use crate::api::API_ENDPOINTS;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// Maximum number of errors to track
const MAX_ERRORS: usize = 50;

/// Service-wide status tracking
#[derive(Clone)]
pub struct StatusTracker {
    inner: Arc<RwLock<StatusTrackerInner>>,
}

/// RAII guard returned by `StatusTracker::try_acquire_configuring`. Clears
/// the switch's busy flag when dropped — via a spawned cleanup task, since
/// `Drop` can't `.await` the tracker's lock directly — so every exit path
/// (normal return, early return/`?`, or a panic unwinding through this
/// guard) releases it. See `try_acquire_configuring`'s doc comment for why
/// this replaces manual set/clear call pairs.
pub struct ConfiguringGuard {
    tracker: StatusTracker,
    switch_id: String,
    /// Set by `release()` to skip the spawn-on-drop path when the caller
    /// already released explicitly (avoids a redundant spawned task).
    released: bool,
}

impl ConfiguringGuard {
    /// Release the busy flag now, synchronously, instead of waiting for
    /// `Drop` to spawn a cleanup task. Prefer this when the caller is
    /// already in an `async` context and wants the flag cleared before
    /// doing anything else (e.g. before recording success/failure status
    /// that itself checks busy state).
    pub async fn release(mut self) {
        self.tracker.clear_currently_configuring(&self.switch_id).await;
        self.released = true;
    }
}

impl Drop for ConfiguringGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let tracker = self.tracker.clone();
        let switch_id = std::mem::take(&mut self.switch_id);
        tokio::spawn(async move {
            tracker.clear_currently_configuring(&switch_id).await;
        });
    }
}

struct StatusTrackerInner {
    start_time: Instant,
    config_metadata: Option<ConfigMetadata>,
    switch_status: std::collections::HashMap<String, SwitchStatus>,
    recent_errors: VecDeque<ErrorRecord>,
    config_validation_issues: Vec<String>,
    last_config_error: Option<ConfigLoadError>,
    /// Set of switch IDs currently being configured (allows parallel applies to different switches)
    currently_configuring: HashSet<String>,
    /// Set of switch IDs with pending config reload (file watcher queue, max 1 per switch)
    pending_config_reload: HashSet<String>,
}

/// Metadata about the loaded configuration
#[derive(Debug, Clone, Serialize)]
pub struct ConfigMetadata {
    pub config_file: PathBuf,
    pub config_folders: Vec<PathBuf>,
    pub last_loaded: DateTime<Utc>,
    pub switches_count: usize,
}

/// Status information for a single switch
#[derive(Debug, Clone, Serialize)]
pub struct SwitchStatus {
    pub id: String,
    pub hostname: String,
    pub model: String,
    pub management_ip: String,
    pub connection_type: String,
    pub last_applied: Option<DateTime<Utc>>,
    pub last_result: Option<String>,
    pub apply_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    /// Warnings from the most recent configuration cycle (e.g., model mismatch)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Record of an error that occurred
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub timestamp: DateTime<Utc>,
    pub error_type: String,
    pub message: String,
    pub switch_hostname: Option<String>,
    pub operation: String,
}

/// Complete service status information
#[derive(Debug, Serialize)]
pub struct ServiceStatus {
    pub service: String,
    pub version: String,
    pub status: String,
    pub uptime_seconds: u64,
    /// List of switch IDs currently being configured (empty when idle)
    pub currently_configuring: Vec<String>,
    /// List of switch IDs with pending config reload from file watcher (queued, waiting)
    pub pending_config_reload: Vec<String>,
    pub configuration: ConfigurationStatus,
    pub api: ApiInfo,
    pub switches: Vec<SwitchStatus>,
    pub recent_errors: Vec<ErrorRecord>,
    pub runtime: RuntimeInfo,
}

#[derive(Debug, Serialize)]
pub struct ConfigurationStatus {
    pub loaded: bool,
    pub config_file: Option<PathBuf>,
    pub config_folders: Vec<PathBuf>,
    pub last_loaded: Option<DateTime<Utc>>,
    pub switches_count: usize,
    pub validation_issues: Vec<String>,
    pub last_error: Option<ConfigLoadError>,
}

/// Structured information about a configuration load error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigLoadError {
    pub timestamp: DateTime<Utc>,
    pub error_type: String,
    pub message: String,
    pub affected_switch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiInfo {
    pub port: u16,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeInfo {
    pub mode: String,
}

impl StatusTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(StatusTrackerInner {
                start_time: Instant::now(),
                config_metadata: None,
                switch_status: std::collections::HashMap::new(),
                recent_errors: VecDeque::with_capacity(MAX_ERRORS),
                config_validation_issues: Vec::new(),
                last_config_error: None,
                currently_configuring: HashSet::new(),
                pending_config_reload: HashSet::new(),
            })),
        }
    }

    /// Mark a switch as currently being configured
    pub async fn set_currently_configuring(&self, switch_id: String) {
        let mut inner = self.inner.write().await;
        inner.currently_configuring.insert(switch_id);
    }

    /// Clear the currently configuring status for a specific switch
    pub async fn clear_currently_configuring(&self, switch_id: &str) {
        let mut inner = self.inner.write().await;
        inner.currently_configuring.remove(switch_id);
    }

    /// Atomically check-and-mark a switch as busy, in a single write-lock
    /// acquisition. Returns `None` if the switch was already busy (matching
    /// `is_switch_busy`'s definition: currently configuring OR a pending
    /// reload queued); returns `Some(guard)` if this call marked it busy.
    ///
    /// This exists because the separate `is_switch_busy().await` check
    /// followed by a later `set_currently_configuring().await` call (the
    /// pattern every operational endpoint used before this) is a
    /// check-then-act race: two concurrent requests can both observe "not
    /// busy" before either sets the flag, both proceed to open a second,
    /// concurrent session on the same exclusive serial device, and one of
    /// them can hang indefinitely on an I/O call with no timeout of its own
    /// (`write_all`/`flush`/`clear_buffer` in `SerialClient` have none —
    /// only the read side, in `wait_for_prompt`, is bounded). A hung task
    /// never reaches its own cleanup code, so the busy flag it set stays
    /// stuck until the whole service is restarted — confirmed as the real
    /// cause of a live incident on IT-02876-sw1 (currently_configuring
    /// stuck for hours, no corresponding "finished processing request" log
    /// line ever appeared for the hung request).
    ///
    /// The returned `ConfiguringGuard` clears the flag on drop (via a
    /// spawned cleanup task, since `Drop` can't `.await`), so *every* exit
    /// path — normal return, early return, and a Rust panic unwinding
    /// through the guard — releases it. This replaces manual
    /// `set_currently_configuring`/`clear_currently_configuring` call pairs,
    /// which require every call site to also handle every panic path by
    /// hand (nothing in Rust does that automatically without an RAII type).
    pub async fn try_acquire_configuring(&self, switch_id: String) -> Option<ConfiguringGuard> {
        let mut inner = self.inner.write().await;
        if inner.currently_configuring.contains(&switch_id) || inner.pending_config_reload.contains(&switch_id) {
            return None;
        }
        inner.currently_configuring.insert(switch_id.clone());
        drop(inner);
        Some(ConfiguringGuard {
            tracker: self.clone(),
            switch_id,
            released: false,
        })
    }

    /// Check if a specific switch is currently being configured
    pub async fn is_currently_configuring(&self, switch_id: &str) -> bool {
        let inner = self.inner.read().await;
        inner.currently_configuring.contains(switch_id)
    }

    /// Queue a pending config reload for a switch (replaces any existing pending reload)
    pub async fn queue_pending_reload(&self, switch_id: String) {
        let mut inner = self.inner.write().await;
        inner.pending_config_reload.insert(switch_id);
    }

    /// Take (remove and return true) a pending reload for a switch if one exists
    pub async fn take_pending_reload(&self, switch_id: &str) -> bool {
        let mut inner = self.inner.write().await;
        inner.pending_config_reload.remove(switch_id)
    }

    /// Check if a switch has a pending config reload queued
    pub async fn has_pending_reload(&self, switch_id: &str) -> bool {
        let inner = self.inner.read().await;
        inner.pending_config_reload.contains(switch_id)
    }

    /// Check if a switch is busy (either being configured or has pending reload)
    pub async fn is_switch_busy(&self, switch_id: &str) -> bool {
        let inner = self.inner.read().await;
        inner.currently_configuring.contains(switch_id) || inner.pending_config_reload.contains(switch_id)
    }

    /// Update configuration metadata
    pub async fn set_config_metadata(&self, metadata: ConfigMetadata) {
        let mut inner = self.inner.write().await;
        inner.config_metadata = Some(metadata);
    }

    /// Get the configuration paths (config file and folders)
    pub async fn get_config_paths(&self) -> Option<(PathBuf, Vec<PathBuf>)> {
        let inner = self.inner.read().await;
        inner.config_metadata.as_ref().map(|m| (m.config_file.clone(), m.config_folders.clone()))
    }

    /// Initialize switch status from configuration
    pub async fn initialize_switches(&self, switches: &[crate::models::SwitchConfig]) {
        let mut inner = self.inner.write().await;
        for switch in switches {
            // These unwraps are safe because configuration validation ensures these fields exist
            let hostname = switch.hostname.as_ref().expect("hostname validated");
            let model = switch.model.as_ref().expect("model validated");
            let management_ip = switch.management_ip.as_ref().expect("management_ip validated");
            let credentials = switch.credentials.as_ref().expect("credentials validated");

            inner.switch_status.insert(
                switch.id.clone(),
                SwitchStatus {
                    id: switch.id.clone(),
                    hostname: hostname.clone(),
                    model: format!("{:?}", model),
                    management_ip: management_ip.clone(),
                    connection_type: format!("{:?}", credentials.connection_type),
                    last_applied: None,
                    last_result: None,
                    apply_count: 0,
                    success_count: 0,
                    failure_count: 0,
                    warnings: Vec::new(),
                },
            );
        }
    }

    /// Record a successful configuration application
    pub async fn record_apply_success(&self, switch_id: &str) {
        let mut inner = self.inner.write().await;
        if let Some(status) = inner.switch_status.get_mut(switch_id) {
            status.last_applied = Some(Utc::now());
            status.last_result = Some("success".to_string());
            status.apply_count += 1;
            status.success_count += 1;
        }
    }

    /// Record a failed configuration application
    pub async fn record_apply_failure(&self, switch_id: &str, error: &str) {
        let mut inner = self.inner.write().await;
        if let Some(status) = inner.switch_status.get_mut(switch_id) {
            status.last_applied = Some(Utc::now());
            status.last_result = Some(format!("failed: {}", error));
            status.apply_count += 1;
            status.failure_count += 1;
        }
    }

    /// Record warnings from a configuration cycle (replaces previous warnings)
    pub async fn record_warnings(&self, switch_id: &str, warnings: Vec<String>) {
        let mut inner = self.inner.write().await;
        if let Some(status) = inner.switch_status.get_mut(switch_id) {
            status.warnings = warnings;
        }
    }

    /// Record an error
    pub async fn record_error(
        &self,
        error_type: String,
        message: String,
        switch_hostname: Option<String>,
        operation: String,
    ) {
        let mut inner = self.inner.write().await;
        let error_record = ErrorRecord {
            timestamp: Utc::now(),
            error_type,
            message,
            switch_hostname,
            operation,
        };

        inner.recent_errors.push_back(error_record);

        // Keep only the most recent MAX_ERRORS
        if inner.recent_errors.len() > MAX_ERRORS {
            inner.recent_errors.pop_front();
        }
    }

    /// Set configuration validation issues
    pub async fn set_validation_issues(&self, issues: Vec<String>) {
        let mut inner = self.inner.write().await;
        inner.config_validation_issues = issues;
    }

    /// Clear configuration validation issues
    pub async fn clear_validation_issues(&self) {
        let mut inner = self.inner.write().await;
        inner.config_validation_issues.clear();
        inner.last_config_error = None;
    }

    /// Set the last configuration load error
    pub async fn set_last_config_error(
        &self,
        error_type: String,
        message: String,
        affected_switch: Option<String>,
    ) {
        let mut inner = self.inner.write().await;
        inner.last_config_error = Some(ConfigLoadError {
            timestamp: Utc::now(),
            error_type,
            message,
            affected_switch,
        });
    }

    /// Get complete service status
    pub async fn get_status(&self, api_port: u16) -> ServiceStatus {
        let inner = self.inner.read().await;
        let uptime = inner.start_time.elapsed();

        let config_status = if let Some(ref metadata) = inner.config_metadata {
            ConfigurationStatus {
                loaded: true,
                config_file: Some(metadata.config_file.clone()),
                config_folders: metadata.config_folders.clone(),
                last_loaded: Some(metadata.last_loaded),
                switches_count: metadata.switches_count,
                validation_issues: inner.config_validation_issues.clone(),
                last_error: inner.last_config_error.clone(),
            }
        } else {
            ConfigurationStatus {
                loaded: false,
                config_file: None,
                config_folders: Vec::new(),
                last_loaded: None,
                switches_count: 0,
                validation_issues: inner.config_validation_issues.clone(),
                last_error: inner.last_config_error.clone(),
            }
        };

        let overall_status = if inner.config_validation_issues.is_empty() {
            "healthy"
        } else {
            "degraded"
        };

        ServiceStatus {
            service: "switch-configurator".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            status: overall_status.to_string(),
            uptime_seconds: uptime.as_secs(),
            currently_configuring: inner.currently_configuring.iter().cloned().collect(),
            pending_config_reload: inner.pending_config_reload.iter().cloned().collect(),
            configuration: config_status,
            api: ApiInfo {
                port: api_port,
                endpoints: API_ENDPOINTS.iter().map(|s| s.to_string()).collect(),
            },
            switches: inner.switch_status.values().cloned().collect(),
            recent_errors: inner.recent_errors.iter().cloned().collect(),
            runtime: RuntimeInfo {
                mode: "service".to_string(),
            },
        }
    }
}

impl Default for StatusTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_try_acquire_configuring_succeeds_when_free() {
        let tracker = StatusTracker::new();
        let guard = tracker.try_acquire_configuring("sw1".to_string()).await;
        assert!(guard.is_some());
        assert!(tracker.is_currently_configuring("sw1").await);
    }

    #[tokio::test]
    async fn test_try_acquire_configuring_fails_when_already_configuring() {
        let tracker = StatusTracker::new();
        let _guard1 = tracker.try_acquire_configuring("sw1".to_string()).await;
        let guard2 = tracker.try_acquire_configuring("sw1".to_string()).await;
        assert!(guard2.is_none(), "second acquire must fail while the first guard is still held");
    }

    #[tokio::test]
    async fn test_try_acquire_configuring_fails_when_pending_reload_queued() {
        let tracker = StatusTracker::new();
        tracker.queue_pending_reload("sw1".to_string()).await;
        let guard = tracker.try_acquire_configuring("sw1".to_string()).await;
        assert!(guard.is_none(), "must not acquire while a pending reload is queued");
    }

    #[tokio::test]
    async fn test_try_acquire_configuring_independent_per_switch() {
        let tracker = StatusTracker::new();
        let _guard1 = tracker.try_acquire_configuring("sw1".to_string()).await;
        let guard2 = tracker.try_acquire_configuring("sw2".to_string()).await;
        assert!(guard2.is_some(), "a different switch id must not be blocked");
    }

    #[tokio::test]
    async fn test_guard_release_clears_flag_immediately() {
        let tracker = StatusTracker::new();
        let guard = tracker.try_acquire_configuring("sw1".to_string()).await.unwrap();
        guard.release().await;
        assert!(!tracker.is_currently_configuring("sw1").await);
    }

    #[tokio::test]
    async fn test_guard_drop_clears_flag_eventually() {
        let tracker = StatusTracker::new();
        {
            let _guard = tracker.try_acquire_configuring("sw1".to_string()).await;
            assert!(tracker.is_currently_configuring("sw1").await);
            // _guard drops here, spawning a cleanup task.
        }
        // Drop's cleanup runs in a spawned task, not synchronously on scope
        // exit -- poll briefly instead of asserting immediately.
        for _ in 0..50 {
            if !tracker.is_currently_configuring("sw1").await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("guard drop never cleared the busy flag within 500ms");
    }

    #[tokio::test]
    async fn test_reacquire_after_release_succeeds() {
        let tracker = StatusTracker::new();
        let guard = tracker.try_acquire_configuring("sw1".to_string()).await.unwrap();
        guard.release().await;
        let guard2 = tracker.try_acquire_configuring("sw1".to_string()).await;
        assert!(guard2.is_some(), "must be able to re-acquire after release");
    }

    /// Regression test for the actual race this guard closes: with the old
    /// separate is_switch_busy()-then-set_currently_configuring() pattern,
    /// two concurrent callers could both observe "not busy" and both
    /// proceed. try_acquire_configuring folds the check and the set into
    /// one write-lock acquisition, so exactly one of any number of
    /// concurrent callers can ever succeed for the same switch id.
    #[tokio::test]
    async fn test_concurrent_acquire_only_one_winner() {
        let tracker = StatusTracker::new();
        let mut handles = Vec::new();
        for _ in 0..20 {
            let tracker = tracker.clone();
            handles.push(tokio::spawn(async move {
                tracker.try_acquire_configuring("sw1".to_string()).await.is_some()
            }));
        }
        let mut successes = 0;
        for h in handles {
            if h.await.unwrap() {
                successes += 1;
            }
        }
        assert_eq!(successes, 1, "exactly one concurrent acquire should have won");
    }
}
