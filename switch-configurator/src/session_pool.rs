//! Keeps one logged-in vendor session per switch open between operations.
//!
//! A console login costs ~10s over serial (login, privilege check, the
//! vendor's setup commands) against ~4s for the actual work, and operations
//! on a switch are serialised by the queue turn in `crate::status`. So a
//! burst of queued operations shares one login instead of paying it each.
//!
//! The pool is NOT the exclusion mechanism: callers hold the switch's queue
//! turn (`StatusTracker::acquire_configuring_queued`) around checkout and
//! release. A session kept open holds the switch's exclusive console, so it
//! is closed after `idle` with nothing queued, and `close_idle` lets paths
//! that open their own session (apply, get_config) free it first.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::RuntimeConfig;
use crate::models::SwitchConfig;
use crate::vendors::traits::{SwitchVendor, VendorError};

/// How long an unused session stays open (see module docs for why short).
pub const DEFAULT_IDLE_CLOSE: Duration = Duration::from_secs(30);

/// Opens a fresh, connected session. A trait so tests can count opens.
#[async_trait]
pub trait SessionOpener: Send + Sync {
    async fn open(&self, config: &SwitchConfig) -> Result<Box<dyn SwitchVendor>, VendorError>;
}

/// Production opener: the vendor factory plus `connect()`.
pub struct VendorOpener;

#[async_trait]
impl SessionOpener for VendorOpener {
    async fn open(&self, config: &SwitchConfig) -> Result<Box<dyn SwitchVendor>, VendorError> {
        let mut vendor = crate::vendors::create_vendor_with_runtime(
            config,
            &RuntimeConfig::default(),
            config.settings.enforce_port_config,
        )
        .map_err(|e| VendorError::ValidationError(e.to_string()))?;
        vendor.connect().await?;
        Ok(vendor)
    }
}

struct Slot {
    vendor: Box<dyn SwitchVendor>,
    fingerprint: u64,
    /// Identifies this particular keep-open so an idle timer armed for an
    /// earlier release can't close a session that was reused and released
    /// again since.
    token: u64,
}

#[derive(Default)]
struct PoolState {
    slots: HashMap<String, Slot>,
    next_token: u64,
}

#[derive(Clone)]
pub struct SessionPool {
    state: Arc<Mutex<PoolState>>,
    opener: Arc<dyn SessionOpener>,
    idle: Duration,
}

/// A checked-out session. Give it back with `release` (keeps it for reuse
/// when the vendor allows) or `discard` (closes it, e.g. after an error).
/// Dropping it without either just closes the connection with the vendor.
pub struct Session {
    vendor: Option<Box<dyn SwitchVendor>>,
    pool: SessionPool,
    switch_id: String,
    fingerprint: u64,
}

/// Identity of everything a connection was built from. A kept session is
/// only reused when this is unchanged (credentials, address, timeouts,...),
/// so a config reload that touches any of it never reuses a session opened
/// with the old values. Hashed only: the password is never stored or logged.
fn fingerprint(config: &SwitchConfig) -> u64 {
    let mut hasher = DefaultHasher::new();
    config.id.hash(&mut hasher);
    format!("{:?}", config.model).hash(&mut hasher);
    config.management_ip.hash(&mut hasher);
    format!("{:?}", config.credentials).hash(&mut hasher);
    config.settings.ssh_timeout_secs.hash(&mut hasher);
    config.settings.max_retries.hash(&mut hasher);
    config.settings.enforce_port_config.hash(&mut hasher);
    hasher.finish()
}

/// Close a session we're throwing away. A failed disconnect changes nothing
/// for the caller: the session is gone either way and the next checkout
/// opens (and logs in) from scratch.
async fn close(mut vendor: Box<dyn SwitchVendor>, switch_id: &str) {
    if let Err(e) = vendor.disconnect().await {
        debug!("closing session for '{}' failed (ignored): {}", switch_id, e);
    }
}

impl SessionPool {
    pub fn new(opener: Arc<dyn SessionOpener>, idle: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(PoolState::default())),
            opener,
            idle,
        }
    }

    /// Production pool: real vendor sessions, closed after `DEFAULT_IDLE_CLOSE`.
    pub fn with_vendor_sessions() -> Self {
        Self::new(Arc::new(VendorOpener), DEFAULT_IDLE_CLOSE)
    }

    /// Hand out a connected session: the kept one if it is for the same
    /// connection settings and still alive, otherwise a fresh one. The
    /// caller must hold the switch's queue turn.
    pub async fn checkout(&self, config: &SwitchConfig) -> Result<Session, VendorError> {
        let fingerprint = fingerprint(config);
        let kept = self.state.lock().await.slots.remove(&config.id);

        if let Some(mut slot) = kept {
            if slot.fingerprint == fingerprint {
                match slot.vendor.resume_session().await {
                    Ok(()) => {
                        debug!("reusing open session for '{}'", config.id);
                        return Ok(self.session(config, slot.vendor, fingerprint));
                    }
                    Err(e) => debug!("kept session for '{}' not reusable ({}), opening a new one", config.id, e),
                }
            }
            close(slot.vendor, &config.id).await;
        }

        let vendor = self.opener.open(config).await?;
        Ok(self.session(config, vendor, fingerprint))
    }

    /// Close this switch's kept session, if there is one. For paths that
    /// open their own connection (apply, get_config): the console is
    /// exclusive, so a kept session would make theirs fail. A session that
    /// is currently checked out is not kept, so it is never touched.
    pub async fn close_idle(&self, switch_id: &str) {
        let kept = self.state.lock().await.slots.remove(switch_id);
        if let Some(slot) = kept {
            close(slot.vendor, switch_id).await;
        }
    }

    fn session(&self, config: &SwitchConfig, vendor: Box<dyn SwitchVendor>, fingerprint: u64) -> Session {
        Session {
            vendor: Some(vendor),
            pool: self.clone(),
            switch_id: config.id.clone(),
            fingerprint,
        }
    }

    async fn close_if_token(&self, switch_id: &str, token: u64) {
        let expired = {
            let mut state = self.state.lock().await;
            match state.slots.get(switch_id) {
                Some(slot) if slot.token == token => state.slots.remove(switch_id),
                _ => None,
            }
        };
        if let Some(slot) = expired {
            debug!("closing idle session for '{}'", switch_id);
            close(slot.vendor, switch_id).await;
        }
    }
}

impl Session {
    pub fn vendor(&mut self) -> &mut dyn SwitchVendor {
        self.vendor.as_mut().expect("vendor present until release/discard").as_mut()
    }

    /// Done with it, and it is healthy: keep it open for the next operation
    /// (closed after the pool's idle timeout), or close it right away if the
    /// vendor can't safely resume a kept session.
    pub async fn release(mut self) {
        let Some(vendor) = self.vendor.take() else { return };
        if !vendor.supports_session_reuse() {
            close(vendor, &self.switch_id).await;
            return;
        }

        let (token, replaced) = {
            let mut state = self.pool.state.lock().await;
            let token = state.next_token;
            state.next_token += 1;
            let replaced = state.slots.insert(
                self.switch_id.clone(),
                Slot { vendor, fingerprint: self.fingerprint, token },
            );
            (token, replaced)
        };
        if let Some(old) = replaced {
            close(old.vendor, &self.switch_id).await;
        }

        let pool = self.pool.clone();
        let switch_id = self.switch_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(pool.idle).await;
            pool.close_if_token(&switch_id, token).await;
        });
    }

    /// The session is suspect (an operation failed or timed out): close it
    /// so the next checkout starts clean.
    pub async fn discard(mut self) {
        if let Some(vendor) = self.vendor.take() {
            close(vendor, &self.switch_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionType, Credentials, SwitchModel};
    use crate::vendors::tests::MockVendor;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Counts opens/disconnects; each opened mock vendor reports `supports`
    /// for reuse and answers `resume_session` with the shared `resume_ok`.
    struct CountingOpener {
        opens: Arc<AtomicUsize>,
        disconnects: Arc<AtomicUsize>,
        resumes: Arc<AtomicUsize>,
        resume_ok: Arc<AtomicBool>,
        supports: bool,
    }

    impl CountingOpener {
        fn new(supports: bool) -> Self {
            Self {
                opens: Arc::new(AtomicUsize::new(0)),
                disconnects: Arc::new(AtomicUsize::new(0)),
                resumes: Arc::new(AtomicUsize::new(0)),
                resume_ok: Arc::new(AtomicBool::new(true)),
                supports,
            }
        }
    }

    #[async_trait]
    impl SessionOpener for CountingOpener {
        async fn open(&self, _config: &SwitchConfig) -> Result<Box<dyn SwitchVendor>, VendorError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            let mut vendor = MockVendor::new();
            let supports = self.supports;
            vendor.expect_supports_session_reuse().returning(move || supports);
            let resumes = self.resumes.clone();
            let resume_ok = self.resume_ok.clone();
            vendor.expect_resume_session().returning(move || {
                resumes.fetch_add(1, Ordering::SeqCst);
                if resume_ok.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(VendorError::SshError("session went stale".to_string()))
                }
            });
            let disconnects = self.disconnects.clone();
            vendor.expect_disconnect().returning(move || {
                disconnects.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            Ok(Box::new(vendor))
        }
    }

    fn switch(id: &str, password: &str) -> SwitchConfig {
        SwitchConfig {
            id: id.to_string(),
            hostname: Some("test".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.0.2.10".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some(password.to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Serial,
                serial_device: Some("/dev/null".to_string()),
                baud_rate: 115200,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        }
    }

    fn pool(opener: &Arc<CountingOpener>, idle: Duration) -> SessionPool {
        SessionPool::new(opener.clone(), idle)
    }

    #[tokio::test]
    async fn burst_of_operations_shares_one_open_session() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));
        let cfg = switch("sw1", "pw");

        for _ in 0..3 {
            let session = pool.checkout(&cfg).await.unwrap();
            session.release().await;
        }

        assert_eq!(opener.opens.load(Ordering::SeqCst), 1, "only the first checkout opens");
        assert_eq!(opener.resumes.load(Ordering::SeqCst), 2, "later checkouts verify the kept session");
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_session_is_replaced_not_reused() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().release().await;
        opener.resume_ok.store(false, Ordering::SeqCst);
        pool.checkout(&cfg).await.unwrap().release().await;

        assert_eq!(opener.opens.load(Ordering::SeqCst), 2, "stale session forces a fresh open");
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1, "the stale one is closed, not leaked");
    }

    #[tokio::test]
    async fn discarded_session_is_closed_and_not_reused() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().discard().await;
        pool.checkout(&cfg).await.unwrap().release().await;

        assert_eq!(opener.opens.load(Ordering::SeqCst), 2);
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn session_closes_after_idle_timeout() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_millis(100));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().release().await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1, "idle session must release the console");

        pool.checkout(&cfg).await.unwrap().release().await;
        assert_eq!(opener.opens.load(Ordering::SeqCst), 2, "after idle close the next checkout opens fresh");
    }

    #[tokio::test]
    async fn new_checkout_cancels_pending_idle_close() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_millis(300));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().release().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        // Re-checked-out and released again: the first timer (due at ~300ms)
        // must not close this newer session.
        pool.checkout(&cfg).await.unwrap().release().await;
        tokio::time::sleep(Duration::from_millis(250)).await; // past the first timer, before the second
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 0, "stale timer closed a live session");
    }

    #[tokio::test]
    async fn changed_credentials_do_not_reuse_old_session() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));

        pool.checkout(&switch("sw1", "old")).await.unwrap().release().await;
        pool.checkout(&switch("sw1", "new")).await.unwrap().release().await;

        assert_eq!(opener.opens.load(Ordering::SeqCst), 2);
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1, "old-credential session is closed");
    }

    #[tokio::test]
    async fn vendors_without_reuse_support_are_closed_on_release() {
        let opener = Arc::new(CountingOpener::new(false));
        let pool = pool(&opener, Duration::from_secs(30));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().release().await;
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1, "console must not stay held");
        pool.checkout(&cfg).await.unwrap().release().await;
        assert_eq!(opener.opens.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn close_idle_frees_a_kept_session_and_ignores_checked_out_ones() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));
        let cfg = switch("sw1", "pw");

        pool.checkout(&cfg).await.unwrap().release().await;
        pool.close_idle("sw1").await;
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1);

        let in_use = pool.checkout(&cfg).await.unwrap();
        pool.close_idle("sw1").await; // nothing kept: must not touch the one in use
        assert_eq!(opener.disconnects.load(Ordering::SeqCst), 1);
        in_use.release().await;
    }

    #[tokio::test]
    async fn sessions_are_per_switch() {
        let opener = Arc::new(CountingOpener::new(true));
        let pool = pool(&opener, Duration::from_secs(30));

        pool.checkout(&switch("sw1", "pw")).await.unwrap().release().await;
        pool.checkout(&switch("sw2", "pw")).await.unwrap().release().await;

        assert_eq!(opener.opens.load(Ordering::SeqCst), 2);
        assert_eq!(opener.resumes.load(Ordering::SeqCst), 0);
    }
}
