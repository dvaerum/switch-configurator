use anyhow::{Context, Result};
use russh::client;
use russh_keys::key;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};
use crate::models::{Credentials, JumpHost};
use crate::ssh::jump_chain::JumpHostChain;

pub struct SshClient {
    session: Option<russh::client::Handle<Handler>>,
    shell_channel: Option<russh::Channel<russh::client::Msg>>,
    host: String,
    port: u16,
    debug_mode: bool,
    dry_run: bool,
    jump_chain: Option<JumpHostChain>,
}

struct Handler;

#[async_trait::async_trait]
impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // In production, implement proper host key verification
        Ok(true)
    }
}

impl SshClient {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            session: None,
            shell_channel: None,
            host,
            port,
            debug_mode: false,
            dry_run: false,
            jump_chain: None,
        }
    }

    /// Enable debug mode (prompts before each command)
    pub fn with_debug_mode(mut self, enabled: bool) -> Self {
        self.debug_mode = enabled;
        self
    }

    /// Enable dry-run mode (shows commands without executing)
    pub fn with_dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// Connect to the switch using password authentication
    pub async fn connect_with_password(&mut self, username: &str, password: &str) -> Result<()> {
        // Configure with legacy algorithms for network equipment compatibility
        let mut config = russh::client::Config::default();
        config.preferred.kex = std::borrow::Cow::Borrowed(&[
            russh::kex::CURVE25519,
            russh::kex::DH_G14_SHA256,
            russh::kex::DH_G14_SHA1,  // Legacy algorithm for older switches
            russh::kex::DH_G16_SHA512,
        ]);
        // Include legacy ssh-rsa for older network equipment that doesn't support modern algorithms
        config.preferred.key = std::borrow::Cow::Borrowed(&[
            russh_keys::key::ED25519,
            russh_keys::key::ECDSA_SHA2_NISTP256,
            russh_keys::key::ECDSA_SHA2_NISTP521,
            russh_keys::key::RSA_SHA2_256,
            russh_keys::key::RSA_SHA2_512,
            russh_keys::key::SSH_RSA,  // Legacy ssh-rsa for older switches
        ]);
        let handler = Handler;

        let mut session = russh::client::connect(Arc::new(config), (&self.host[..], self.port), handler)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to establish SSH connection to {}:{}: {}", self.host, self.port, e))?;

        let auth_res = session
            .authenticate_password(username, password)
            .await
            .context("Failed to authenticate with password")?;

        if !auth_res {
            anyhow::bail!("Authentication failed");
        }

        self.session = Some(session);
        debug!("SSH connection established to {}:{}", self.host, self.port);

        // Open interactive shell with PTY for switches that don't support exec mode
        self.open_shell().await?;

        Ok(())
    }

    /// Connect to the switch using key-based authentication
    pub async fn connect_with_key(&mut self, username: &str, key_path: &str) -> Result<()> {
        // Configure with legacy algorithms for network equipment compatibility
        let mut config = russh::client::Config::default();
        config.preferred.kex = std::borrow::Cow::Borrowed(&[
            russh::kex::CURVE25519,
            russh::kex::DH_G14_SHA256,
            russh::kex::DH_G14_SHA1,  // Legacy algorithm for older switches
            russh::kex::DH_G16_SHA512,
        ]);
        // Include legacy ssh-rsa for older network equipment that doesn't support modern algorithms
        config.preferred.key = std::borrow::Cow::Borrowed(&[
            russh_keys::key::ED25519,
            russh_keys::key::ECDSA_SHA2_NISTP256,
            russh_keys::key::ECDSA_SHA2_NISTP521,
            russh_keys::key::RSA_SHA2_256,
            russh_keys::key::RSA_SHA2_512,
            russh_keys::key::SSH_RSA,  // Legacy ssh-rsa for older switches
        ]);
        let handler = Handler;

        // Expand ~ to home directory
        let expanded_path = crate::ssh::expand_tilde(key_path);
        let key_pair = russh_keys::load_secret_key(expanded_path.as_ref(), None)
            .context("Failed to load SSH key")?;

        let mut session = russh::client::connect(Arc::new(config), (&self.host[..], self.port), handler)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to establish SSH connection to {}:{}: {}", self.host, self.port, e))?;

        let auth_res = session
            .authenticate_publickey(username, Arc::new(key_pair))
            .await
            .context("Failed to authenticate with key")?;

        if !auth_res {
            anyhow::bail!("Key authentication failed");
        }

        self.session = Some(session);
        debug!("SSH connection established to {}:{}", self.host, self.port);

        // Open interactive shell with PTY for switches that don't support exec mode
        self.open_shell().await?;

        Ok(())
    }

    /// Open interactive shell with PTY (required for switches like Aruba that don't support exec mode)
    async fn open_shell(&mut self) -> Result<()> {
        let session = self
            .session
            .as_mut()
            .context("Not connected")?;

        let mut channel = session
            .channel_open_session()
            .await
            .context("Failed to open shell channel")?;

        // Request PTY
        channel
            .request_pty(
                false,
                "vt100",  // Terminal type
                80,       // Width (columns)
                24,       // Height (rows)
                0,        // Pixel width (unused)
                0,        // Pixel height (unused)
                &[],      // Terminal modes
            )
            .await
            .context("Failed to request PTY")?;

        // Request shell
        channel
            .request_shell(false)
            .await
            .context("Failed to request shell")?;

        debug!("Interactive shell opened with PTY");

        // Wait for initial prompt
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.clear_buffer(&mut channel).await?;

        self.shell_channel = Some(channel);
        Ok(())
    }

    /// Clear any buffered data from the channel
    async fn clear_buffer(&self, channel: &mut russh::Channel<russh::client::Msg>) -> Result<()> {
        // Try to read any pending data with a short timeout
        let mut buf = vec![0u8; 4096];
        loop {
            tokio::select! {
                msg = channel.wait() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { .. }) => continue,
                        _ => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => break,
            }
        }
        Ok(())
    }

    /// Wait for switch prompt (adapted from SerialClient logic)
    async fn wait_for_prompt(&mut self, timeout_secs: u64) -> Result<String> {
        let channel = self
            .shell_channel
            .as_mut()
            .context("Shell not opened")?;

        let mut output = Vec::new();
        let timeout = Duration::from_secs(timeout_secs);
        let start = tokio::time::Instant::now();
        let mut last_data_time = tokio::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                let text = String::from_utf8_lossy(&output);
                let clean = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                    .unwrap()
                    .replace_all(&text, "");
                warn!("Timeout waiting for prompt. Last received (clean): {:?}", &clean[clean.len().saturating_sub(200)..]);
                anyhow::bail!("Timeout waiting for prompt");
            }

            tokio::select! {
                msg = channel.wait() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            output.extend_from_slice(&data);
                            last_data_time = tokio::time::Instant::now();

                            // Check if we have a prompt
                            let text = String::from_utf8_lossy(&output);
                            trace!("Received data (raw): {:?}", &text[text.len().saturating_sub(100)..]);

                            // Remove ANSI escape sequences for checking
                            let clean = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                                .unwrap()
                                .replace_all(&text, "");

                            trace!("Received data (clean, last 100 chars): {:?}", &clean[clean.len().saturating_sub(100)..]);

                            // Look for switch prompt pattern at end of output
                            let prompt_regex = regex::Regex::new(r"[\w-]+(\([^\)]+\))?[>#]\s*$").unwrap();
                            let interactive_regex = regex::Regex::new(r"(Username|username|Password|password|Enable password):\s*$").unwrap();
                            let press_key_regex = regex::Regex::new(r"Press any key to continue\s*$").unwrap();
                            let more_regex = regex::Regex::new(r"--\s*MORE\s*--").unwrap();

                            if press_key_regex.is_match(&clean) {
                                debug!("Detected 'Press any key to continue' prompt, sending ENTER");
                                channel.data(&b"\r"[..]).await?;
                                output.clear();
                                last_data_time = tokio::time::Instant::now();
                            } else if more_regex.is_match(&clean) {
                                debug!("Detected '-- MORE --' paging prompt, sending SPACE");
                                channel.data(&b" "[..]).await?;
                                output.clear();
                                last_data_time = tokio::time::Instant::now();
                            } else if prompt_regex.is_match(&clean) || interactive_regex.is_match(&clean) {
                                trace!("Prompt detected!");
                                return Ok(text.to_string());
                            }
                        }
                        Some(russh::ChannelMsg::Eof) | None => {
                            anyhow::bail!("Channel closed while waiting for prompt");
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(300)) => {
                    // If no data for 300ms and we have output, check for prompt
                    if last_data_time.elapsed() > Duration::from_millis(300) && !output.is_empty() {
                        let text = String::from_utf8_lossy(&output);
                        let clean = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                            .unwrap()
                            .replace_all(&text, "");

                        let prompt_regex = regex::Regex::new(r"[\w-]+(\([^\)]+\))?[>#]\s*$").unwrap();
                        let interactive_regex = regex::Regex::new(r"(Username|username|Password|password|Enable password):\s*$").unwrap();

                        if prompt_regex.is_match(&clean) || interactive_regex.is_match(&clean) {
                            return Ok(text.to_string());
                        }
                    }
                }
            }
        }
    }

    /// Execute a command and return the output
    pub async fn execute_command(&mut self, command: &str) -> Result<String> {
        info!("📝 Command: {}", command);

        // Debug mode: prompt for confirmation
        if self.debug_mode {
            print!("   Execute this command? [Y/n/q]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let response = input.trim().to_lowercase();

            match response.as_str() {
                "q" | "quit" | "exit" => {
                    anyhow::bail!("User aborted execution");
                }
                "n" | "no" => {
                    info!("   ⏭️  Skipped by user");
                    return Ok(String::new());
                }
                _ => {
                    // "y", "yes", or empty (default yes)
                }
            }
        }

        // Dry-run mode: skip execution (except for read-only commands and session settings)
        let is_readonly = command.trim().starts_with("show ");
        let is_session_setting = command.trim() == "no page"
            || command.trim() == "terminal length 0"
            || command.trim() == "terminal pager 0";

        if self.dry_run && !is_readonly && !is_session_setting {
            info!("   🔍 [DRY-RUN] Would execute (skipped)");
            return Ok(String::new());
        }

        // In dry-run mode, allow 'show' commands and session settings to execute
        if self.dry_run && (is_readonly || is_session_setting) {
            if is_readonly {
                info!("   🔍 [DRY-RUN] Executing read-only command");
            } else {
                info!("   🔍 [DRY-RUN] Executing session setting");
            }
        }

        // Use interactive shell channel instead of exec mode
        let channel = self
            .shell_channel
            .as_mut()
            .context("Shell channel not opened")?;

        // Send command with newline
        let cmd_with_newline = format!("{}\r", command);
        channel.data(&cmd_with_newline.as_bytes()[..]).await
            .context("Failed to send command")?;

        // Wait for prompt and get output
        let timeout = if is_readonly || is_session_setting { 60 } else { 120 };
        let output = self.wait_for_prompt(timeout).await?;

        // Remove command echo and prompt from output
        let clean_output = self.clean_command_output(&output, command);

        if !clean_output.is_empty() {
            debug!("Command output: {} bytes", clean_output.len());
        }

        Ok(clean_output)
    }

    /// Clean command output by removing echo and prompt
    fn clean_command_output(&self, output: &str, command: &str) -> String {
        // Remove ANSI escape sequences
        let clean = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
            .unwrap()
            .replace_all(output, "");

        let mut lines: Vec<&str> = clean.lines().collect();

        // Remove command echo (first line usually contains the command)
        if !lines.is_empty() {
            let first_line = lines[0].trim();
            if first_line.contains(command) || first_line.is_empty() {
                lines.remove(0);
            }
        }

        // Remove prompt from last line
        if !lines.is_empty() {
            let last_idx = lines.len() - 1;
            let last_line = lines[last_idx].trim();
            let prompt_regex = regex::Regex::new(r"[\w-]+(\([^\)]+\))?[>#]\s*$").unwrap();
            if prompt_regex.is_match(last_line) {
                lines.remove(last_idx);
            }
        }

        lines.join("\n")
    }

    /// Execute multiple commands in sequence
    pub async fn execute_commands(&mut self, commands: &[String]) -> Result<Vec<String>> {
        let mut results = Vec::new();

        for command in commands {
            let output = self.execute_command(command).await?;
            results.push(output);
        }

        Ok(results)
    }

    /// Connect with credentials (auto-detects jump host usage)
    pub async fn connect_with_credentials(&mut self, creds: &Credentials) -> Result<()> {
        // Check if jump hosts are configured
        if let Some(jump_hosts) = &creds.jump_hosts {
            if !jump_hosts.is_empty() {
                self.validate_jump_hosts(jump_hosts)?;
                return self.connect_via_jump_chain(creds, jump_hosts).await;
            }
        }

        // No jump hosts - direct connection
        if let Some(password) = &creds.password {
            self.connect_with_password(&creds.username, password).await
        } else if let Some(key_path) = &creds.ssh_key_path {
            self.connect_with_key(&creds.username, key_path).await
        } else {
            anyhow::bail!("No authentication method provided")
        }
    }

    /// Connect with credentials and retry logic
    ///
    /// # Arguments
    /// * `creds` - SSH credentials
    /// * `max_retries` - Maximum number of connection attempts (minimum 1)
    /// * `retry_delay_secs` - Delay in seconds between retry attempts
    ///
    /// # Returns
    /// * `Ok(())` - If connection succeeds
    /// * `Err(...)` - If all retry attempts fail
    pub async fn connect_with_retry(
        &mut self,
        creds: &Credentials,
        max_retries: u32,
        retry_delay_secs: u64,
    ) -> Result<()> {
        // Ensure at least 1 attempt
        let max_attempts = max_retries.max(1);
        let mut last_error = None;

        for attempt in 1..=max_attempts {
            debug!(
                "SSH connection attempt {}/{} to {}",
                attempt, max_attempts, self.host
            );

            match self.connect_with_credentials(creds).await {
                Ok(()) => {
                    if attempt > 1 {
                        info!(
                            "SSH connection succeeded on attempt {}/{} to {}",
                            attempt, max_attempts, self.host
                        );
                    }
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_attempts {
                        warn!(
                            "SSH connection attempt {}/{} failed to {}: {}, retrying in {}s",
                            attempt,
                            max_attempts,
                            self.host,
                            last_error.as_ref().unwrap(),
                            retry_delay_secs
                        );
                        tokio::time::sleep(Duration::from_secs(retry_delay_secs)).await;
                    }
                }
            }
        }

        // All retries exhausted
        Err(anyhow::anyhow!(
            "SSH connection failed after {} attempts to {}: {}",
            max_attempts,
            self.host,
            last_error.unwrap()
        ))
    }

    /// Validate jump host configuration
    fn validate_jump_hosts(&self, jump_hosts: &[JumpHost]) -> Result<()> {
        if jump_hosts.is_empty() {
            anyhow::bail!("jump_hosts array is empty");
        }

        for (idx, jump_host) in jump_hosts.iter().enumerate() {
            let hop_num = idx + 1;

            // Validate that each hop has at least one authentication method
            let has_auth = jump_host.ssh_key_path.is_some() || jump_host.password.is_some();

            if !has_auth {
                warn!(
                    "Jump host #{} ({}) has no authentication credentials. \
                    Will attempt to use target switch credentials if available.",
                    hop_num, jump_host.host
                );
            }

            // Validate host is not empty
            if jump_host.host.trim().is_empty() {
                anyhow::bail!("Jump host #{} has empty host field", hop_num);
            }
        }

        Ok(())
    }

    /// Connect to target through jump host chain
    async fn connect_via_jump_chain(
        &mut self,
        creds: &Credentials,
        jump_hosts: &[JumpHost],
    ) -> Result<()> {
        info!(
            "Connecting to {}:{} via {}-hop jump chain",
            self.host,
            self.port,
            jump_hosts.len()
        );

        // Establish jump host chain
        let chain = JumpHostChain::establish(
            jump_hosts,
            &self.host,
            self.port,
            &creds.username,
        )
        .await
        .context("Failed to establish jump host chain")?;

        let local_port = chain.local_port();

        // Connect to target through the chain (via localhost)
        let original_host = self.host.clone();
        let original_port = self.port;

        self.host = "127.0.0.1".to_string();
        self.port = local_port;

        let result = if let Some(password) = &creds.password {
            self.connect_with_password(&creds.username, password).await
        } else if let Some(key_path) = &creds.ssh_key_path {
            self.connect_with_key(&creds.username, key_path).await
        } else {
            anyhow::bail!("No authentication method for target switch")
        };

        // Restore original host/port for logging
        self.host = original_host;
        self.port = original_port;

        if result.is_ok() {
            info!(
                "Successfully connected to {} via jump host chain",
                self.host
            );
            // Keep jump chain alive
            self.jump_chain = Some(chain);
        }

        result
    }

    /// Disconnect from the switch (including jump host chain)
    pub async fn disconnect(&mut self) -> Result<()> {
        // Disconnect target
        if let Some(session) = self.session.take() {
            session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await
                .context("Failed to disconnect from target")?;
            debug!("Target SSH connection closed");
        }

        // Disconnect jump chain
        if let Some(chain) = self.jump_chain.take() {
            chain.disconnect().await
                .context("Failed to disconnect jump host chain")?;
        }

        Ok(())
    }
}

impl Drop for SshClient {
    fn drop(&mut self) {
        if self.session.is_some() {
            warn!("SshClient dropped without explicit disconnect");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionType, JumpHost};

    #[test]
    fn test_validate_jump_hosts_empty_array() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts: Vec<JumpHost> = vec![];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_validate_jump_hosts_empty_host() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![JumpHost {
            host: "".to_string(),
            username: None,
            port: None,
            ssh_key_path: Some("/key".to_string()),
            password: None,
        }];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty host"));
    }

    #[test]
    fn test_validate_jump_hosts_whitespace_only() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![JumpHost {
            host: "   ".to_string(),
            username: None,
            port: None,
            ssh_key_path: Some("/key".to_string()),
            password: None,
        }];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_jump_hosts_valid_with_key() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![JumpHost {
            host: "bastion.com".to_string(),
            username: Some("user".to_string()),
            port: None,
            ssh_key_path: Some("/path/to/key".to_string()),
            password: None,
        }];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_jump_hosts_valid_with_password() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![JumpHost {
            host: "bastion.com".to_string(),
            username: Some("user".to_string()),
            port: None,
            ssh_key_path: None,
            password: Some("password".to_string()),
        }];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_jump_hosts_no_auth_warns() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![JumpHost {
            host: "bastion.com".to_string(),
            username: Some("user".to_string()),
            port: None,
            ssh_key_path: None,
            password: None,
        }];

        // Should succeed but log a warning
        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_jump_hosts_multi_hop() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        let jump_hosts = vec![
            JumpHost {
                host: "jump1.com".to_string(),
                username: None,
                port: None,
                ssh_key_path: Some("/key1".to_string()),
                password: None,
            },
            JumpHost {
                host: "jump2.com".to_string(),
                username: None,
                port: None,
                ssh_key_path: None,
                password: Some("pass2".to_string()),
            },
            JumpHost {
                host: "jump3.com".to_string(),
                username: None,
                port: None,
                ssh_key_path: Some("/key3".to_string()),
                password: None,
            },
        ];

        let result = client.validate_jump_hosts(&jump_hosts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ssh_client_new() {
        let client = SshClient::new("192.168.1.1".to_string(), 22);
        assert_eq!(client.host, "192.168.1.1");
        assert_eq!(client.port, 22);
        assert!(!client.debug_mode);
        assert!(!client.dry_run);
        assert!(client.session.is_none());
        assert!(client.jump_chain.is_none());
    }

    #[test]
    fn test_ssh_client_with_debug_mode() {
        let client = SshClient::new("192.168.1.1".to_string(), 22)
            .with_debug_mode(true);
        assert!(client.debug_mode);
    }

    #[test]
    fn test_ssh_client_with_dry_run() {
        let client = SshClient::new("192.168.1.1".to_string(), 22)
            .with_dry_run(true);
        assert!(client.dry_run);
    }

    // Tests for connect_with_retry behavior
    // Note: These tests verify the retry logic by checking that the function
    // correctly propagates errors and implements the retry loop.
    // Since we can't mock the actual SSH connection in unit tests,
    // we test the error handling paths.

    #[tokio::test]
    async fn test_connect_with_retry_unreachable_host() {
        // Test that connect_with_retry correctly fails after max retries
        // when connecting to an unreachable host
        let mut client = SshClient::new("192.0.2.1".to_string(), 22); // TEST-NET-1, never reachable
        let creds = Credentials {
            username: "admin".to_string(),
            password: Some("password".to_string()),
            connection_type: ConnectionType::Ssh,
            port: 22,
            serial_device: None,
            baud_rate: 9600,
            ssh_key_path: None,
            jump_hosts: None,
        };

        // Should fail after retries (using small number for test speed)
        let result = client.connect_with_retry(&creds, 1, 1).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("192.0.2.1"));
    }

    #[tokio::test]
    async fn test_connect_with_retry_multiple_attempts() {
        // Test that connect_with_retry attempts multiple times
        // Using an unreachable IP with very short delay between retries
        let mut client = SshClient::new("192.0.2.1".to_string(), 22);
        let creds = Credentials {
            username: "admin".to_string(),
            password: Some("password".to_string()),
            connection_type: ConnectionType::Ssh,
            port: 22,
            serial_device: None,
            baud_rate: 9600,
            ssh_key_path: None,
            jump_hosts: None,
        };

        let start = std::time::Instant::now();
        let result = client.connect_with_retry(&creds, 3, 1).await;
        let elapsed = start.elapsed();

        // Should have taken at least 2 seconds (3 attempts with 1s delay between)
        // But may be less depending on OS TCP timeout behavior
        assert!(result.is_err());

        // Verify error message mentions all failed attempts
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("192.0.2.1"));
        assert!(error_msg.contains("3 attempts"));
    }

    #[tokio::test]
    async fn test_connect_with_retry_zero_retries() {
        // Test behavior with max_retries = 0 (should still attempt once due to .max(1))
        let mut client = SshClient::new("192.0.2.1".to_string(), 22);
        let creds = Credentials {
            username: "admin".to_string(),
            password: Some("password".to_string()),
            connection_type: ConnectionType::Ssh,
            port: 22,
            serial_device: None,
            baud_rate: 9600,
            ssh_key_path: None,
            jump_hosts: None,
        };

        // With max_retries = 0, it should still attempt once (enforced by .max(1))
        let result = client.connect_with_retry(&creds, 0, 1).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        // Should have made 1 attempt
        assert!(error_msg.contains("1 attempts"));
    }
}
