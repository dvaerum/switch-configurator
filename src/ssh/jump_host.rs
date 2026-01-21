use anyhow::{Context, Result};
use russh::client;
use russh_keys::key;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};
use crate::models::ResolvedJumpHost;

pub struct JumpHostSession {
    session: Arc<tokio::sync::Mutex<russh::client::Handle<Handler>>>,
    info: ResolvedJumpHost,
    // TCP listener for port forwarding (kept alive)
    #[allow(dead_code)]
    listener: Option<Arc<TcpListener>>,
}

struct Handler;

#[async_trait::async_trait]
impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: Implement proper host key verification
        Ok(true)
    }
}

impl JumpHostSession {
    /// Connect to a jump host with authentication fallback
    /// Priority: SSH key -> Password
    pub async fn connect(resolved: ResolvedJumpHost) -> Result<Self> {
        info!(
            "Connecting to jump host {}@{}:{}",
            resolved.username, resolved.hostname, resolved.port
        );

        // Configure with legacy algorithms for compatibility
        let mut config = russh::client::Config::default();
        config.preferred.kex = std::borrow::Cow::Borrowed(&[
            russh::kex::CURVE25519,
            russh::kex::DH_G14_SHA256,
            russh::kex::DH_G14_SHA1,  // Legacy algorithm for older hosts
            russh::kex::DH_G16_SHA512,
        ]);
        // Include legacy ssh-rsa for older hosts that don't support modern algorithms
        config.preferred.key = std::borrow::Cow::Borrowed(&[
            russh_keys::key::ED25519,
            russh_keys::key::ECDSA_SHA2_NISTP256,
            russh_keys::key::ECDSA_SHA2_NISTP521,
            russh_keys::key::RSA_SHA2_256,
            russh_keys::key::RSA_SHA2_512,
            russh_keys::key::SSH_RSA,  // Legacy ssh-rsa
        ]);
        let handler = Handler;

        let mut session = russh::client::connect(
            Arc::new(config),
            (&resolved.hostname[..], resolved.port),
            handler,
        )
        .await
        .with_context(|| {
            format!(
                "Failed to establish connection to jump host {}:{}",
                resolved.hostname, resolved.port
            )
        })?;

        // Try authentication methods in priority order
        let authenticated = Self::authenticate(&mut session, &resolved).await?;

        if !authenticated {
            anyhow::bail!(
                "All authentication methods failed for jump host {}@{}:{}",
                resolved.username,
                resolved.hostname,
                resolved.port
            );
        }

        info!(
            "Successfully authenticated to jump host {}@{}:{}",
            resolved.username, resolved.hostname, resolved.port
        );

        Ok(Self {
            session: Arc::new(tokio::sync::Mutex::new(session)),
            info: resolved,
            listener: None,
        })
    }

    /// Try authentication methods in priority order
    /// Priority: 1. SSH key (if provided), 2. Password (if provided)
    async fn authenticate(
        session: &mut russh::client::Handle<Handler>,
        resolved: &ResolvedJumpHost,
    ) -> Result<bool> {
        let mut attempted_methods = Vec::new();

        // Priority 1: SSH Key Authentication
        if let Some(key_path) = &resolved.ssh_key_path {
            debug!("Attempting SSH key authentication with: {}", key_path);
            attempted_methods.push("ssh-key");

            match Self::try_key_auth(session, &resolved.username, key_path).await {
                Ok(true) => {
                    info!("SSH key authentication succeeded");
                    return Ok(true);
                }
                Ok(false) => {
                    debug!("SSH key authentication rejected");
                }
                Err(e) => {
                    warn!("SSH key authentication error: {}", e);
                }
            }
        }

        // Priority 2: Password Authentication
        if let Some(password) = &resolved.password {
            debug!("Attempting password authentication");
            attempted_methods.push("password");

            match Self::try_password_auth(session, &resolved.username, password).await {
                Ok(true) => {
                    info!("Password authentication succeeded");
                    return Ok(true);
                }
                Ok(false) => {
                    debug!("Password authentication rejected");
                }
                Err(e) => {
                    warn!("Password authentication error: {}", e);
                }
            }
        }

        // No authentication method succeeded
        if attempted_methods.is_empty() {
            anyhow::bail!(
                "No authentication credentials provided for jump host {}@{}:{}",
                resolved.username,
                resolved.hostname,
                resolved.port
            );
        }

        Ok(false)
    }

    /// Attempt SSH key authentication
    async fn try_key_auth(
        session: &mut russh::client::Handle<Handler>,
        username: &str,
        key_path: &str,
    ) -> Result<bool> {
        // Expand ~ to home directory
        let expanded_path = crate::ssh::expand_tilde(key_path);
        let key_pair = russh_keys::load_secret_key(expanded_path.as_ref(), None)
            .with_context(|| format!("Failed to load SSH key: {}", key_path))?;

        session
            .authenticate_publickey(username, Arc::new(key_pair))
            .await
            .context("SSH key authentication failed")
    }

    /// Attempt password authentication
    async fn try_password_auth(
        session: &mut russh::client::Handle<Handler>,
        username: &str,
        password: &str,
    ) -> Result<bool> {
        session
            .authenticate_password(username, password)
            .await
            .context("Password authentication failed")
    }

    /// Create a TCP port forward through this jump host
    /// Returns the local port number that forwards to target_host:target_port
    pub async fn create_port_forward(
        &mut self,
        target_host: &str,
        target_port: u16,
    ) -> Result<u16> {
        debug!(
            "Creating port forward through {} to {}:{}",
            self.info.hostname, target_host, target_port
        );

        // Bind TCP listener on random port
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Failed to bind TCP listener for port forward")?;

        let local_addr = listener.local_addr()
            .context("Failed to get local address")?;
        let local_port = local_addr.port();

        // Clone session Arc for background task
        let session = Arc::clone(&self.session);
        let target_host_str = target_host.to_string();
        let target_host_clone = target_host_str.clone();
        let jump_host = self.info.hostname.clone();

        // Keep listener alive by storing it
        let listener_arc = Arc::new(listener);
        let listener_clone = listener_arc.clone();

        // Spawn background task to handle port forwarding
        tokio::spawn(async move {
            // Accept ONE connection (for the target SSH connection)
            match listener_clone.accept().await {
                Ok((mut tcp_stream, _)) => {
                    debug!("Accepted connection on localhost:{}", local_port);

                    // Create SSH channel to target
                    let mut session_guard = session.lock().await;
                    match session_guard.channel_open_direct_tcpip(
                        &target_host_clone,
                        target_port as u32,
                        "127.0.0.1",
                        local_port as u32,
                    ).await {
                        Ok(mut channel) => {
                            debug!("SSH channel created to {}:{}", target_host_clone, target_port);

                            // Forward data bidirectionally
                            let mut buf_tcp = vec![0u8; 8192];
                            let mut buf_ssh = vec![0u8; 8192];

                            loop {
                                tokio::select! {
                                    // TCP -> SSH
                                    result = tcp_stream.read(&mut buf_tcp) => {
                                        match result {
                                            Ok(0) => break, // EOF
                                            Ok(n) => {
                                                if let Err(e) = channel.data(&buf_tcp[..n]).await {
                                                    warn!("Failed to send data through SSH channel: {}", e);
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                warn!("TCP read error: {}", e);
                                                break;
                                            }
                                        }
                                    }
                                    // SSH -> TCP
                                    msg = channel.wait() => {
                                        match msg {
                                            Some(russh::ChannelMsg::Data { data }) => {
                                                if let Err(e) = tcp_stream.write_all(&data).await {
                                                    warn!("TCP write error: {}", e);
                                                    break;
                                                }
                                                // Flush to ensure data is sent immediately
                                                if let Err(e) = tcp_stream.flush().await {
                                                    warn!("TCP flush error: {}", e);
                                                    break;
                                                }
                                            }
                                            Some(russh::ChannelMsg::Eof) | None => break,
                                            _ => {}
                                        }
                                    }
                                }
                            }

                            debug!("Port forward connection closed");
                        }
                        Err(e) => {
                            warn!("Failed to create SSH channel: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to accept connection: {}", e);
                }
            }
        });

        // Store listener to keep it alive
        self.listener = Some(listener_arc);

        info!(
            "Port forward established: localhost:{} -> {}:{} (via {})",
            local_port, target_host_str, target_port, jump_host
        );

        Ok(local_port)
    }

    /// Find an available local port for forwarding
    fn find_free_port() -> Result<u16> {
        use std::net::TcpListener;

        // Bind to port 0 to let OS assign a free port
        let listener = TcpListener::bind("127.0.0.1:0")
            .context("Failed to bind to local port")?;

        let local_addr = listener.local_addr()
            .context("Failed to get local address")?;

        Ok(local_addr.port())
    }

    /// Get jump host info for logging
    pub fn info(&self) -> &ResolvedJumpHost {
        &self.info
    }
}
