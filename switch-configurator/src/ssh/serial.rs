use anyhow::{Context, Result};
use std::io::{self, Write as StdWrite};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialPortBuilderExt;
use tracing::{debug, info, trace, warn};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

pub struct SerialClient {
    port: Option<tokio_serial::SerialStream>,
    device: String,
    baud_rate: u32,
    debug_mode: bool,
    dry_run: bool,
    /// When true, all commands execute regardless of dry_run (used during login/enable auth)
    auth_mode: bool,
}

impl SerialClient {
    pub fn new(device: String, baud_rate: u32) -> Self {
        Self {
            port: None,
            device,
            baud_rate,
            debug_mode: false,
            dry_run: false,
            auth_mode: false,
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

    /// Enable auth mode: all commands execute regardless of dry-run.
    /// Used during login and enable authentication where credentials must
    /// be sent as interactive responses to prompts.
    pub fn set_auth_mode(&mut self, enabled: bool) {
        self.auth_mode = enabled;
    }

    /// Connect to the serial device
    pub async fn connect(&mut self) -> Result<()> {
        debug!("Opening serial device: {} at {} baud", self.device, self.baud_rate);

        // Check if device exists before attempting to open
        let device_path = std::path::Path::new(&self.device);
        if !device_path.exists() {
            let available_devices = self.list_available_serial_devices();
            anyhow::bail!(
                "Serial device does not exist: {}\n\
                 \n\
                 Possible causes:\n\
                 - Device not connected to the system\n\
                 - Incorrect device path in configuration\n\
                 - Device not recognized by the operating system\n\
                 \n\
                 Available serial devices:\n\
                 {}",
                self.device,
                if available_devices.is_empty() {
                    "  <none found>".to_string()
                } else {
                    available_devices.iter().map(|d| format!("  - {}", d)).collect::<Vec<_>>().join("\n")
                }
            );
        }

        // Check if device is already locked by another process
        self.check_device_lock()?;

        let port = tokio_serial::new(&self.device, self.baud_rate)
            .timeout(Duration::from_secs(10))
            .open_native_async()
            .with_context(|| {
                format!(
                    "Failed to open serial device: {}. \
                     This may be a permissions issue. \
                     Try running with sudo or add your user to the 'dialout' group (Linux) or 'uucp' group (BSD).",
                    self.device
                )
            })?;

        self.port = Some(port);
        debug!("Serial connection established to {}", self.device);

        // Give the device a moment to stabilize
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Clear any initial data
        self.clear_buffer().await?;

        Ok(())
    }

    /// Connect with retry logic
    ///
    /// # Arguments
    /// * `max_retries` - Maximum number of connection attempts (including first attempt)
    /// * `retry_delay_secs` - Delay in seconds between retry attempts
    ///
    /// # Returns
    /// * `Ok(())` - If connection succeeds
    /// * `Err(...)` - If all retry attempts fail
    pub async fn connect_with_retry(&mut self, max_retries: u32, retry_delay_secs: u64) -> Result<()> {
        let mut last_error = None;
        let max_retries = max_retries.max(1);

        for attempt in 1..=max_retries {
            debug!(
                "Serial connection attempt {}/{} to {}",
                attempt, max_retries, self.device
            );

            match self.connect().await {
                Ok(()) => {
                    if attempt > 1 {
                        info!(
                            "Serial connection succeeded on attempt {}/{} to {}",
                            attempt, max_retries, self.device
                        );
                    }
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        warn!(
                            "Serial connection attempt {}/{} failed to {}: {}, retrying in {}s",
                            attempt,
                            max_retries,
                            self.device,
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
            "Serial connection failed after {} attempts to {}: {}",
            max_retries,
            self.device,
            last_error.unwrap()
        ))
    }

    /// List available serial devices on the system
    fn list_available_serial_devices(&self) -> Vec<String> {
        let mut devices = Vec::new();

        // Check common serial device paths on Linux/Unix
        let search_paths = vec![
            "/dev/ttyUSB",  // USB serial adapters
            "/dev/ttyACM",  // ACM devices (Arduino, etc.)
            "/dev/ttyS",    // Built-in serial ports
            "/dev/cu.",     // macOS serial devices
            "/dev/serial/by-id/",  // Linux by-id symlinks
        ];

        for base_path in search_paths {
            if base_path.ends_with('/') {
                // It's a directory, list its contents
                if let Ok(entries) = std::fs::read_dir(base_path) {
                    for entry in entries.flatten() {
                        if let Ok(path) = entry.path().canonicalize() {
                            if let Some(path_str) = path.to_str() {
                                devices.push(path_str.to_string());
                            }
                        }
                    }
                }
            } else {
                // It's a prefix pattern, glob for matches
                for i in 0..10 {
                    let device = format!("{}{}", base_path, i);
                    if std::path::Path::new(&device).exists() {
                        devices.push(device);
                    }
                }
            }
        }

        devices.sort();
        devices.dedup();
        devices
    }

    /// Clear the input buffer by draining all pending data
    ///
    /// Reads in a loop until no more data arrives within the timeout period.
    /// This prevents leftover data from a previous command from contaminating
    /// the next command's output.
    async fn clear_buffer(&mut self) -> Result<()> {
        if let Some(port) = &mut self.port {
            let mut temp_buf = [0u8; 4096];
            let mut total_cleared = 0usize;
            loop {
                tokio::select! {
                    result = port.read(&mut temp_buf) => {
                        match result {
                            Ok(n) if n > 0 => {
                                trace!("Serial clear_buffer: discarding {} bytes: {:?}", n, String::from_utf8_lossy(&temp_buf[..n]));
                                total_cleared += n;
                                // Keep reading - there might be more data
                                continue;
                            }
                            _ => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        break;
                    }
                }
            }
            if total_cleared > 0 {
                debug!("Cleared {} bytes from serial buffer", total_cleared);
            }
        }
        Ok(())
    }

    /// Send login credentials
    pub async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        info!("Logging in as user: {}", username);

        // NOTE: Login is always performed, even in dry-run mode.
        // Serial connections require authentication before any commands work,
        // including read-only "show" commands needed for state parsing.

        // Send Ctrl-C + Enter to break out of any stuck state (e.g., switch left
        // in config mode from a previous session that timed out, or a "-- MORE --"
        // prompt). Ctrl-C aborts the current command/mode on most switch CLIs.
        self.send_raw("\x03").await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.send_raw("\r").await?;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Try to determine current state
        let mut state_check = self.check_current_state(Duration::from_secs(2)).await?;

        debug!("Current state check: {}", state_check);

        // Check for Cisco initial configuration dialog prompt
        if state_check.contains("initial configuration dialog") {
            debug!("Cisco initial configuration dialog detected, declining...");
            self.send_raw("no\r").await?;
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Wait for the next prompt (usually a login or command prompt)
            state_check = self.check_current_state(Duration::from_secs(2)).await?;
            debug!("State after declining initial config: {}", state_check);
        }

        if state_check.contains("login:") || state_check.contains("Username:") {
            // Need to login
            debug!("Login prompt detected, logging in...");

            // Send username
            self.send_raw(&format!("{}\r", username)).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Send password (password prompt should appear)
            self.send_raw(&format!("{}\r", password)).await?;

            // Wait for command prompt after login
            tokio::time::sleep(Duration::from_secs(2)).await;
        } else if state_check.contains('#') || state_check.contains('>') {
            // Already at a prompt — but check if we're in config mode and need to exit
            let ansi_stripped = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                .unwrap()
                .replace_all(&state_check, "");
            if ansi_stripped.contains("(config") {
                debug!("In config mode, sending 'end' to return to privileged exec mode");
                self.send_raw("end\r").await?;
                tokio::time::sleep(Duration::from_secs(1)).await;
                // Drain the response
                let _ = self.check_current_state(Duration::from_secs(1)).await;
            } else {
                debug!("Already at command prompt, skipping login");
            }
        } else {
            // Unknown state - this can happen on Aruba switches when the previous user logged out,
            // or when the switch is stuck in config mode / "-- MORE --" / "Press any key" state.
            // Also handles "Session Terminated, login timed out" where a stale session expired.
            let mut logged_in = false;

            for attempt in 1..=3 {
                debug!("Unknown state, recovery attempt {}/3...", attempt);

                if attempt == 1 {
                    // Ctrl-C + Enter to break out of stuck state
                    self.send_raw("\x03").await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    self.send_raw("\r").await?;
                    tokio::time::sleep(Duration::from_millis(800)).await;
                } else if attempt == 2 {
                    // Space (for "-- MORE --") + Ctrl-C + Enter
                    self.send_raw(" ").await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    self.send_raw("\x03").await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    self.send_raw("\r").await?;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    // Final attempt: longer wait then Enter (for post-banner login prompts)
                    self.send_raw("\r").await?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }

                state_check = self.check_current_state(Duration::from_secs(3)).await?;
                debug!("State check after attempt {}: {}", attempt, state_check);

                // Check if "Session Terminated" appeared — the switch dropped a stale session.
                // After this, it usually shows the login banner then eventually a login prompt.
                // We need to clear this output and wait for the actual login prompt.
                let ansi_stripped = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                    .unwrap()
                    .replace_all(&state_check, "");
                if ansi_stripped.contains("Session Terminated") || ansi_stripped.contains("session terminated") {
                    warn!("Stale serial session detected ('Session Terminated'), waiting for fresh login prompt...");
                    // Clear residual banner text, send Enter to trigger fresh login prompt
                    self.clear_buffer().await?;
                    self.send_raw("\r").await?;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    state_check = self.check_current_state(Duration::from_secs(5)).await?;
                    debug!("State after session-terminated recovery: {}", state_check);
                }

                if state_check.contains("login:") || state_check.contains("Username:") {
                    debug!("Login prompt appeared on attempt {}, logging in...", attempt);
                    self.send_raw(&format!("{}\r", username)).await?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.send_raw(&format!("{}\r", password)).await?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    logged_in = true;
                    break;
                } else if state_check.contains('#') || state_check.contains('>') {
                    debug!("Command prompt appeared on attempt {}, already logged in", attempt);
                    logged_in = true;
                    break;
                }
            }

            if !logged_in {
                anyhow::bail!(
                    "Failed to detect login prompt or command prompt after 3 recovery attempts. \
                     Last state: {:?}. The serial session may be stuck — try power-cycling the \
                     console connection or rebooting the switch.",
                    state_check.chars().take(200).collect::<String>()
                );
            }
        }

        debug!("Login/prompt detection successful");

        // Check if we need to enter privileged exec mode (Cisco switches)
        self.enter_privileged_mode().await?;

        // Verify the connection is actually working by checking for a response
        self.verify_connectivity().await?;

        Ok(())
    }

    /// Enter privileged exec mode if we're in user mode (Cisco switches)
    async fn enter_privileged_mode(&mut self) -> Result<()> {
        debug!("Checking privilege level...");

        // Send newline to get current prompt
        self.send_raw("\r").await?;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let prompt_raw = self.check_current_state(Duration::from_secs(2)).await?;
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
        let prompt_clean = ansi_regex.replace_all(&prompt_raw, "");
        let prompt_trimmed = prompt_clean.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();

        // Check if we're in user mode (prompt ends with >)
        // Examples: "Switch>", "Router>", "hostname>"
        if prompt_trimmed.ends_with('>') {
            debug!("In user mode (>), entering privileged exec mode...");

            self.send_raw("enable\r").await?;
            tokio::time::sleep(Duration::from_millis(500)).await;

            let response = self.check_current_state(Duration::from_secs(2)).await?;

            // Check if enable password is required
            if response.contains("Password:") {
                debug!("Enable password required, but no enable password configured");
                debug!("Attempting to continue without enable password...");
                // Try pressing enter (no enable password configured)
                self.send_raw("\r").await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            // Verify we're now in privileged mode
            self.send_raw("\r").await?;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let final_prompt = self.check_current_state(Duration::from_secs(2)).await?;
            let final_prompt_trimmed = final_prompt.trim();

            if final_prompt_trimmed.ends_with('#') {
                debug!("Successfully entered privileged exec mode (#)");
            } else if final_prompt_trimmed.ends_with('>') {
                warn!("Still in user mode after enable command. Enable password may be required.");
                warn!("Add 'enable_secret' to switch credentials if commands fail.");
            } else {
                debug!("Privilege level unclear, but continuing anyway");
            }
        } else if prompt_trimmed.ends_with('#') {
            debug!("Already in privileged exec mode (#)");
        } else {
            debug!("Unknown prompt format: '{}', assuming OK", prompt_trimmed);
        }

        Ok(())
    }

    /// Verify serial connectivity by checking for prompt response
    /// This helps detect locked devices or unresponsive switches early
    async fn verify_connectivity(&mut self) -> Result<()> {
        debug!("Verifying serial device connectivity...");

        // Send a newline and wait for prompt response
        self.send_raw("\r").await?;

        // Try to get a response within 3 seconds
        let response = self.check_current_state(Duration::from_secs(3)).await?;

        // Check if we got any meaningful response (should have a prompt)
        let clean_response = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
            .unwrap()
            .replace_all(&response, "");

        let clean_trimmed = clean_response.trim();

        if clean_trimmed.is_empty() {
            anyhow::bail!(
                "Serial device not responding. \
                 Possible causes: (1) Device locked by another process (e.g., picocom, minicom, screen), \
                 (2) Switch powered off or not connected, (3) Wrong baud rate (configured: {})",
                self.baud_rate
            );
        }

        // Verify we have a command prompt (# or >), not just stale banner or "Password:" text
        if !clean_trimmed.contains('#') && !clean_trimmed.contains('>') {
            // Check if we're stuck at a login/password prompt — session wasn't established
            if clean_trimmed.contains("Password:") || clean_trimmed.contains("login:")
                || clean_trimmed.contains("Username:") {
                anyhow::bail!(
                    "Serial session not authenticated. Got a login/password prompt instead of a \
                     command prompt. The login sequence may have failed."
                );
            }
            warn!(
                "Serial connectivity check: no command prompt detected in response. \
                 Response: {:?}",
                &clean_trimmed[..clean_trimmed.len().min(200)]
            );
        }

        debug!("Serial connectivity verified - device is responding");
        Ok(())
    }

    /// Check current state by reading available data
    async fn check_current_state(&mut self, timeout: Duration) -> Result<String> {
        if let Some(port) = &mut self.port {
            let mut accumulated = String::new();
            let mut buf = [0u8; 1024];
            let start = tokio::time::Instant::now();
            let mut last_data_time = tokio::time::Instant::now();

            loop {
                if start.elapsed() > timeout {
                    break;
                }

                tokio::select! {
                    result = port.read(&mut buf) => {
                        match result {
                            Ok(n) if n > 0 => {
                                let data = String::from_utf8_lossy(&buf[..n]);
                                trace!("Serial RX (check_state): {} bytes: {:?}", n, data);
                                accumulated.push_str(&data);
                                last_data_time = tokio::time::Instant::now();
                            }
                            Ok(_) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Only return early if we have data that looks like a prompt
                        // (login:, Username:, Password:, #, >) or enough idle time has
                        // passed. This prevents returning too early when the switch sends
                        // \r\n first, then the actual prompt after a delay.
                        if !accumulated.is_empty() && last_data_time.elapsed() > Duration::from_millis(500) {
                            break;
                        }
                        // Also break early if we already have a recognizable prompt
                        if accumulated.contains("login:") || accumulated.contains("Username:")
                            || accumulated.contains("Password:") || accumulated.contains('#')
                            || accumulated.contains('>') {
                            break;
                        }
                    }
                }
            }

            trace!("Serial check_state result: {:?}", accumulated);
            Ok(accumulated)
        } else {
            anyhow::bail!("Not connected")
        }
    }

    /// Send raw data to the serial port
    async fn send_raw(&mut self, data: &str) -> Result<()> {
        if let Some(port) = &mut self.port {
            // Use \r for line endings (Aruba switches expect carriage return)
            let data_with_cr = if data.ends_with('\n') {
                data.replace('\n', "\r")
            } else if !data.ends_with('\r') && !data.is_empty() {
                format!("{}\r", data)
            } else {
                data.to_string()
            };

            trace!("Serial TX (raw): {:?}", data_with_cr);
            AsyncWriteExt::write_all(port, data_with_cr.as_bytes()).await?;
            AsyncWriteExt::flush(port).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        } else {
            anyhow::bail!("Not connected")
        }
    }

    /// Wait for command prompt to appear in the serial output.
    ///
    /// This method reads data from the serial port until it detects a switch
    /// command prompt (e.g., `hostname#` or `hostname>`). It handles:
    /// - ANSI escape sequence stripping
    /// - "Press any key to continue" prompts (auto-dismisses)
    /// - "-- MORE --" paging prompts (auto-continues)
    /// - Interactive prompts (Username/Password)
    /// - Cisco IOS log messages that appear after the prompt
    ///
    /// To avoid false-positive prompt detection (which can cause truncated output),
    /// this method uses a **confirmation wait**: after detecting what looks like a
    /// prompt, it waits briefly to see if more data arrives. If it does, the match
    /// was a false positive (e.g., a line in the running config that looks like a
    /// prompt) and we continue reading.
    async fn wait_for_prompt(&mut self, timeout_secs: u64) -> Result<String> {
        if let Some(port) = &mut self.port {
            let mut output = Vec::new();
            let mut buf = [0u8; 4096];
            let timeout = Duration::from_secs(timeout_secs);
            let start = tokio::time::Instant::now();
            let mut last_data_time = tokio::time::Instant::now();
            // Track when we first detected a potential prompt match.
            // We use this for a "confirmation wait" to avoid false positives.
            let mut prompt_detected_at: Option<tokio::time::Instant> = None;

            // Pre-compile regexes outside the loop for efficiency
            let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
            // Prompt: hostname with at least 2 chars, optional (context), then # or >
            // Must be the entire line (anchored with ^ and $)
            let prompt_regex = regex::Regex::new(r"^[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$").unwrap();
            let interactive_regex = regex::Regex::new(
                r"(Username|username|Password|password|Enable password):\s*$"
            ).unwrap();
            let press_key_regex = regex::Regex::new(r"Press any key to continue\s*$").unwrap();
            let more_regex = regex::Regex::new(r"--\s*MORE\s*--").unwrap();

            // How long to wait after a prompt match to confirm no more data is coming.
            // Serial connections can have gaps between data bursts, so we need to be
            // patient. 500ms is enough for serial at 9600+ baud.
            let prompt_confirm_delay = Duration::from_millis(500);
            // How long after the last data before we consider the output "settled"
            // and check for a prompt in the idle-timeout path.
            let idle_settle_time = Duration::from_millis(500);

            loop {
                if start.elapsed() > timeout {
                    let text = String::from_utf8_lossy(&output);
                    let clean = ansi_regex.replace_all(&text, "");
                    warn!(
                        "Timeout waiting for prompt after {}s. Output length: {} bytes. Last received (clean): {:?}",
                        timeout_secs,
                        output.len(),
                        &clean[clean.len().saturating_sub(200)..]
                    );
                    anyhow::bail!("Timeout waiting for prompt");
                }

                // If we previously detected a prompt, check if the confirmation delay has passed
                // without any new data arriving.
                if let Some(detected_at) = prompt_detected_at {
                    if last_data_time <= detected_at && detected_at.elapsed() >= prompt_confirm_delay {
                        // No new data arrived since we detected the prompt - it's confirmed
                        let text = String::from_utf8_lossy(&output);
                        trace!("Prompt confirmed after {}ms settle time", prompt_confirm_delay.as_millis());
                        return Ok(text.to_string());
                    }
                }

                tokio::select! {
                    result = port.read(&mut buf) => {
                        match result {
                            Ok(n) if n > 0 => {
                                // Log raw bytes at trace level for serial debugging
                                trace!(
                                    "Serial RX: {} bytes: {:?}",
                                    n,
                                    String::from_utf8_lossy(&buf[..n])
                                );

                                output.extend_from_slice(&buf[..n]);
                                last_data_time = tokio::time::Instant::now();

                                // New data arrived - any previous prompt detection was a false positive
                                if prompt_detected_at.is_some() {
                                    trace!("New data arrived after prompt detection - was a false positive, continuing to read");
                                    prompt_detected_at = None;
                                }

                                let text = String::from_utf8_lossy(&output);
                                trace!("Accumulated: {} bytes (total: {}), last 100 chars: {:?}", n, output.len(), &text[text.len().saturating_sub(100)..]);

                                // Remove ANSI escape sequences for checking
                                let clean = ansi_regex.replace_all(&text, "");

                                // Handle special prompts that need immediate action
                                if press_key_regex.is_match(&clean) {
                                    debug!("Detected 'Press any key to continue' prompt, sending ENTER");
                                    AsyncWriteExt::write_all(port, b"\r").await?;
                                    AsyncWriteExt::flush(port).await?;
                                    output.clear();
                                    last_data_time = tokio::time::Instant::now();
                                    prompt_detected_at = None;
                                    continue;
                                }

                                if more_regex.is_match(&clean) {
                                    debug!("Detected '-- MORE --' paging prompt, sending SPACE to continue");
                                    AsyncWriteExt::write_all(port, b" ").await?;
                                    AsyncWriteExt::flush(port).await?;
                                    // Remove the answered pager prompt so the stale
                                    // marker can't re-match forever (page content kept).
                                    crate::ssh::strip_pager_prompt(&mut output);
                                    last_data_time = tokio::time::Instant::now();
                                    prompt_detected_at = None;
                                    continue;
                                }

                                // Check for interactive prompts (these should return immediately)
                                if interactive_regex.is_match(&clean) {
                                    trace!("Interactive prompt detected (Username/Password)!");
                                    return Ok(text.to_string());
                                }

                                // Check for command prompt on the last non-empty line.
                                // We only check the LAST non-empty line to avoid matching
                                // prompt-like patterns within config output (e.g., hostname
                                // references in SNMP or banner text).
                                let last_nonempty_line = clean.lines()
                                    .rev()
                                    .find(|line| !line.trim().is_empty())
                                    .unwrap_or("")
                                    .trim();

                                let has_prompt = prompt_regex.is_match(last_nonempty_line);

                                // Also handle Cisco: check last 3 lines in case log messages
                                // appeared after the prompt. But only use strict line matching.
                                let has_prompt_in_recent = if !has_prompt {
                                    clean.lines().rev().take(3).any(|line| {
                                        prompt_regex.is_match(line.trim())
                                    })
                                } else {
                                    false
                                };

                                // Also check for prompt at end of last line even if not at
                                // start. This handles command echo + prompt on the same line
                                // without a newline, e.g. "no pageIT-04269# ".
                                // Safe because the confirmation wait will reject false positives.
                                let has_prompt_at_end = if !has_prompt && !has_prompt_in_recent {
                                    let end_prompt_regex = regex::Regex::new(
                                        r"[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$"
                                    ).unwrap();
                                    end_prompt_regex.is_match(last_nonempty_line)
                                } else {
                                    false
                                };

                                if has_prompt || has_prompt_in_recent || has_prompt_at_end {
                                    // Don't return immediately! Start the confirmation timer.
                                    // This prevents false positives when a line in the config
                                    // output happens to match the prompt pattern.
                                    trace!(
                                        "Potential prompt detected: {:?} (starting {}ms confirmation wait)",
                                        last_nonempty_line,
                                        prompt_confirm_delay.as_millis()
                                    );
                                    prompt_detected_at = Some(tokio::time::Instant::now());
                                }
                            }
                            Ok(_) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        // Periodic check for confirmation timeout and idle-settle prompts
                        if let Some(detected_at) = prompt_detected_at {
                            if last_data_time <= detected_at && detected_at.elapsed() >= prompt_confirm_delay {
                                let text = String::from_utf8_lossy(&output);
                                trace!("Prompt confirmed via idle check after {}ms", prompt_confirm_delay.as_millis());
                                return Ok(text.to_string());
                            }
                        }

                        // Idle-settle path: if no data for idle_settle_time and we have output,
                        // do a final prompt check. This handles cases where the prompt was
                        // split across reads or arrived without triggering the main path.
                        if prompt_detected_at.is_none()
                            && last_data_time.elapsed() > idle_settle_time
                            && !output.is_empty()
                        {
                            let text = String::from_utf8_lossy(&output);
                            let clean = ansi_regex.replace_all(&text, "");

                            // Handle special prompts in idle path
                            if press_key_regex.is_match(&clean) {
                                debug!("Detected 'Press any key to continue' prompt (idle path), sending ENTER");
                                AsyncWriteExt::write_all(port, b"\r").await?;
                                AsyncWriteExt::flush(port).await?;
                                output.clear();
                                last_data_time = tokio::time::Instant::now();
                                continue;
                            }

                            if more_regex.is_match(&clean) {
                                debug!("Detected '-- MORE --' paging prompt (idle path), sending SPACE");
                                AsyncWriteExt::write_all(port, b" ").await?;
                                AsyncWriteExt::flush(port).await?;
                                // Remove the answered pager prompt so the stale
                                // marker can't re-match forever (page content kept).
                                crate::ssh::strip_pager_prompt(&mut output);
                                last_data_time = tokio::time::Instant::now();
                                continue;
                            }

                            let last_nonempty_line = clean.lines()
                                .rev()
                                .find(|line| !line.trim().is_empty())
                                .unwrap_or("")
                                .trim();

                            // Check for prompt: strict line match, recent lines, or end-of-line
                            let end_prompt_regex = regex::Regex::new(
                                r"[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$"
                            ).unwrap();
                            let has_prompt = prompt_regex.is_match(last_nonempty_line)
                                || interactive_regex.is_match(&clean)
                                || end_prompt_regex.is_match(last_nonempty_line)
                                || clean.lines().rev().take(3).any(|l| prompt_regex.is_match(l.trim()));

                            if has_prompt {
                                trace!("Prompt detected in idle-settle path: {:?}", last_nonempty_line);
                                return Ok(text.to_string());
                            }
                        }
                    }
                }
            }
        }

        anyhow::bail!("Not connected")
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

        // Dry-run mode: skip execution (except for read-only commands, session settings,
        // and anything during auth_mode which handles login/enable credentials)
        let is_readonly = command.trim().starts_with("show ")
            || command.trim().starts_with("get ");  // FortiSwitch read commands
        let is_session_setting = command.trim() == "no page"
            || command.trim() == "terminal length 0"
            || command.trim() == "terminal pager 0"
            || command.trim() == "enable"
            || command.trim() == "end";  // FortiSwitch: exit config mode

        if self.dry_run && !self.auth_mode && !is_readonly && !is_session_setting {
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

        // Clear any pending data
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.clear_buffer().await?;

        if let Some(port) = &mut self.port {
            // Send the command with \r (not \r\n)
            let cmd = format!("{}\r", command);
            trace!("Serial TX: {:?}", cmd);
            AsyncWriteExt::write_all(port, cmd.as_bytes()).await?;
            AsyncWriteExt::flush(port).await?;

            // Wait for prompt - use longer timeout for "show" commands that return lots of data.
            // "show running-config" can be very large on serial connections (10KB+ at 9600 baud).
            let timeout = if command.starts_with("show running") {
                60
            } else if command.starts_with("show") {
                30
            } else {
                10
            };
            let output = self.wait_for_prompt(timeout).await?;

            // Check for errors in output
            // Note: "Invalid input: enable" when already in privileged mode is benign
            let has_error = (output.contains("Invalid input") && !output.contains("Invalid input: enable"))
                || output.contains("Error:")
                || output.contains("Unknown command");

            if has_error {
                warn!("Command may have failed: {}", command);
                debug!("Output contains error indication: {}", output.trim());
            }

            if !output.is_empty() {
                debug!("Command output: {} bytes, {} lines", output.len(), output.lines().count());
                trace!("Output (first 200 chars): {}", &output.chars().take(200).collect::<String>());
            }

            Ok(output)
        } else {
            anyhow::bail!("Not connected. Call connect first")
        }
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

    /// Check if the device file is locked by another process
    #[cfg(unix)]
    fn check_device_lock(&self) -> Result<()> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        // Try to open the device with non-blocking flag to test for locks
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(&self.device)
        {
            Ok(file) => {
                // Try to acquire an exclusive lock (non-blocking)
                let fd = file.as_raw_fd();
                let lock_result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

                if lock_result != 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock {
                        anyhow::bail!(
                            "Serial device {} is locked by another process (e.g., picocom, minicom, screen). \
                             Please close the other program and try again.",
                            self.device
                        );
                    }
                }

                // Release the lock
                unsafe { libc::flock(fd, libc::LOCK_UN) };
                Ok(())
            }
            Err(e) => {
                // Device couldn't be opened - check if it's a lock issue
                if e.kind() == io::ErrorKind::PermissionDenied {
                    anyhow::bail!(
                        "Permission denied accessing {}. Ensure you have access rights (dialout group membership).",
                        self.device
                    );
                }
                // Other errors will be caught when tokio_serial tries to open
                Ok(())
            }
        }
    }

    #[cfg(not(unix))]
    fn check_device_lock(&self) -> Result<()> {
        // Lock checking not implemented for non-Unix systems
        Ok(())
    }

    /// Disconnect from the serial device
    pub async fn disconnect(&mut self) -> Result<()> {
        if let Some(_port) = self.port.take() {
            debug!("Serial connection closed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;
    use tokio;

    // ============================================================================
    // Prompt Detection Tests (Critical for Recent Changes)
    // ============================================================================

    #[test]
    fn test_press_any_key_prompt_detection() {
        // Test the regex pattern used to detect Aruba's welcome screen prompt
        let press_key_regex = regex::Regex::new(r"Press any key to continue\s*$").unwrap();

        // Should match various formats
        assert!(press_key_regex.is_match("Press any key to continue"));
        assert!(press_key_regex.is_match("Press any key to continue "));
        assert!(press_key_regex.is_match("Press any key to continue  "));
        assert!(press_key_regex.is_match("some preamble text\nPress any key to continue"));
        assert!(press_key_regex.is_match("Multi\nline\ntext\nPress any key to continue"));

        // Should NOT match if there's text after
        assert!(!press_key_regex.is_match("Press any key to continue and then more"));
        assert!(!press_key_regex.is_match("Press any key"));
        assert!(!press_key_regex.is_match("press any key to continue")); // Case sensitive
    }

    #[test]
    fn test_more_paging_prompt_detection() {
        // Test the regex pattern used to detect "-- MORE --" paging prompts
        let more_regex = regex::Regex::new(r"--\s*MORE\s*--").unwrap();

        // Should match various spacing formats
        assert!(more_regex.is_match("-- MORE --"));
        assert!(more_regex.is_match("--MORE--"));
        assert!(more_regex.is_match("--  MORE  --"));
        assert!(more_regex.is_match("-- MORE --"));
        assert!(more_regex.is_match("some text\n-- MORE --\nmore text"));
        assert!(more_regex.is_match("output\n-- MORE --, next page: Space"));

        // Should not match incomplete patterns
        assert!(!more_regex.is_match("MORE"));
        assert!(!more_regex.is_match("- MORE -"));
        assert!(!more_regex.is_match("-- more --")); // Case sensitive
    }

    #[test]
    fn test_switch_prompt_regex() {
        // Test the regex pattern used to detect switch command prompts
        // Pattern requires at least 2 word chars to avoid false positives from single chars.
        // Uses ^ and $ to match individual lines (not entire accumulated output).
        // In actual code, lines are trimmed before matching.
        let prompt_regex = regex::Regex::new(r"^[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$").unwrap();

        // Valid prompts that should match (on a single line, after trimming)
        assert!(prompt_regex.is_match("switch#"));
        assert!(prompt_regex.is_match("switch>"));
        assert!(prompt_regex.is_match("router#"));
        assert!(prompt_regex.is_match("hostname>"));
        assert!(prompt_regex.is_match("switch(config)#"));
        assert!(prompt_regex.is_match("switch(vlan-42)#"));
        assert!(prompt_regex.is_match("switch(config-if)#"));
        assert!(prompt_regex.is_match("hostname-with-dashes#"));
        assert!(prompt_regex.is_match("test-switch#"));
        assert!(prompt_regex.is_match("test_switch#"));
        assert!(prompt_regex.is_match("switch123#"));
        assert!(prompt_regex.is_match("switch# ")); // Trailing space
        assert!(prompt_regex.is_match("IT-04269#")); // Real production hostname

        // Leading/trailing whitespace: In actual code, lines are trimmed first
        assert!(prompt_regex.is_match("  switch#  ".trim()));

        // Should NOT match standalone # or > (could appear in output/comments)
        assert!(!prompt_regex.is_match("#"));
        assert!(!prompt_regex.is_match(">"));
        assert!(!prompt_regex.is_match("just text #"));
        assert!(!prompt_regex.is_match("# comment"));

        // Should NOT match single-char hostnames (too likely to be false positives)
        assert!(!prompt_regex.is_match("s#"));
        assert!(!prompt_regex.is_match("r>"));

        // Should not match if there's text after the prompt (on same line)
        assert!(!prompt_regex.is_match("switch# show running"));

        // Should not match if there's text before the prompt (on same line)
        // Note: multiline strings won't match because ^ requires start of string
        assert!(!prompt_regex.is_match("output line\nswitch#"));

        // Test line-by-line detection logic (as used in actual code)
        // This is the key scenario for Cisco: prompt on one line, log message on another
        let multiline_output = "description Management Port\nSwitch(config-if)#\n*Nov 26 08:07:29.324: %SYS-5-CONFIG_I: Configured from console by console";
        let has_prompt = multiline_output.lines().any(|line| {
            let trimmed = line.trim();
            prompt_regex.is_match(trimmed)
        });
        assert!(has_prompt, "Should detect prompt on second line despite log message on third line");
    }

    #[test]
    fn test_interactive_prompt_detection() {
        // Test the regex pattern for login/password prompts
        let interactive_regex = regex::Regex::new(
            r"(Username|username|Password|password|Enable password):\s*$"
        ).unwrap();

        // Valid interactive prompts
        assert!(interactive_regex.is_match("Username: "));
        assert!(interactive_regex.is_match("username: "));
        assert!(interactive_regex.is_match("Password: "));
        assert!(interactive_regex.is_match("password: "));
        assert!(interactive_regex.is_match("Enable password: "));
        assert!(interactive_regex.is_match("output\nUsername: "));

        // Edge case: "Enter username:" WILL match because it ends with "username:"
        // This is acceptable behavior - we want to catch username prompts regardless
        // of preamble text
        assert!(interactive_regex.is_match("Enter username: "));

        // Should not match incomplete formats (no colon and space at end)
        assert!(!interactive_regex.is_match("Username"));
        assert!(!interactive_regex.is_match("Password"));

        // Should not match if there's text after the prompt
        assert!(!interactive_regex.is_match("Username: admin"));
        assert!(!interactive_regex.is_match("Password: followed by text"));
    }

    #[test]
    fn test_ansi_escape_sequence_removal() {
        // Test that ANSI escape sequences are properly removed before prompt matching
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();

        // Common ANSI sequences from switch output
        let test_cases = vec![
            ("\x1b[24;40Hswitch# \x1b[?25h", "switch# "),
            ("\x1b[?6l\x1b[1;24rswitch#", "switch#"),
            ("\x1b[2Jswitch#", "switch#"),
            ("\x1b[1;1Houtput\x1b[2Kswitch#", "outputswitch#"),
            ("no escapes here", "no escapes here"),
        ];

        for (input, expected) in test_cases {
            let clean = ansi_regex.replace_all(input, "");
            assert_eq!(clean, expected, "Failed to clean: {:?}", input);
        }
    }

    // ============================================================================
    // False Positive Prompt Detection Tests
    // These test scenarios that previously caused truncated command output.
    // ============================================================================

    #[test]
    fn test_prompt_regex_does_not_match_config_lines() {
        // The prompt regex should NOT match lines that appear in running-config output
        // but look similar to prompts. This was the root cause of the serial output
        // truncation bug: lines within running-config were matching the prompt pattern.
        let prompt_regex = regex::Regex::new(r"^[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$").unwrap();

        // These are lines from real Aruba running-config that should NOT match
        assert!(!prompt_regex.is_match("hostname \"IT-04269\""));
        assert!(!prompt_regex.is_match("   name \"management\""));
        assert!(!prompt_regex.is_match("   untagged 1-24"));
        assert!(!prompt_regex.is_match("   tagged 1-24"));
        assert!(!prompt_regex.is_match("   ip address 192.168.1.1 255.255.255.0"));
        assert!(!prompt_regex.is_match("snmp-server host 10.0.0.1 community \"public\""));
        assert!(!prompt_regex.is_match("   no power-over-ethernet"));
        assert!(!prompt_regex.is_match("interface 1"));
        assert!(!prompt_regex.is_match("vlan 42"));
        assert!(!prompt_regex.is_match("   exit"));
        assert!(!prompt_regex.is_match("; J9855A Configuration Editor"));
        assert!(!prompt_regex.is_match("Running configuration:"));
        assert!(!prompt_regex.is_match("mirror 1 port 22"));

        // SNMP community strings that might contain # or >
        assert!(!prompt_regex.is_match("snmp-server community \"ro_community#1\" operator"));

        // Comments with # should not match
        assert!(!prompt_regex.is_match("# This is a comment"));
        assert!(!prompt_regex.is_match("; comment with # symbol"));
    }

    #[test]
    fn test_prompt_regex_matches_real_switch_prompts() {
        // Ensure the regex still correctly matches real switch prompts
        let prompt_regex = regex::Regex::new(r"^[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$").unwrap();

        // Real Aruba switch prompts
        assert!(prompt_regex.is_match("IT-04269#"));
        assert!(prompt_regex.is_match("IT-04269# "));
        assert!(prompt_regex.is_match("HP-2530-8G-PoEP#"));
        assert!(prompt_regex.is_match("HP-2530-8G-PoEP(config)#"));
        assert!(prompt_regex.is_match("HP-2530-8G-PoEP(vlan-42)#"));

        // Real Cisco switch prompts
        assert!(prompt_regex.is_match("Switch#"));
        assert!(prompt_regex.is_match("Switch>"));
        assert!(prompt_regex.is_match("Switch(config)#"));
        assert!(prompt_regex.is_match("Switch(config-if)#"));
        assert!(prompt_regex.is_match("Catalyst9300#"));

        // FortiSwitch prompts
        assert!(prompt_regex.is_match("S108FPTV21002683#"));
    }

    #[test]
    fn test_last_nonempty_line_extraction() {
        // Test the last-non-empty-line logic used for prompt detection.
        // This avoids matching prompts that appear mid-output.

        // Simulate running-config output with prompt at the end
        let output = "Running configuration:\n\nhostname \"IT-04269\"\nvlan 42\n   name \"test\"\n   exit\nIT-04269# \n";
        let last_nonempty = output.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        assert_eq!(last_nonempty, "IT-04269#");

        // Simulate output where the "prompt" is mid-output (should NOT be the last line)
        let output_mid = "show running-config\n\nIT-04269# show running\nhostname \"IT-04269\"\nvlan 42\n   name \"test\"\n   exit\n";
        let last_nonempty_mid = output_mid.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        // The last non-empty line is "   exit", not "IT-04269# show running"
        assert_eq!(last_nonempty_mid, "exit");

        let prompt_regex = regex::Regex::new(r"^[\w-]{2,}\s*(\([^\)]+\))?\s*[>#]\s*$").unwrap();
        assert!(!prompt_regex.is_match(last_nonempty_mid), "Should NOT match 'exit' as a prompt");
    }

    // ============================================================================
    // Dry-Run Mode and Command Classification Tests
    // ============================================================================

    #[test]
    fn test_session_setting_classification() {
        // Helper function to classify commands (extracted from execute_command logic)
        fn is_session_setting(cmd: &str) -> bool {
            cmd.trim() == "no page"
                || cmd.trim() == "terminal length 0"
                || cmd.trim() == "terminal pager 0"
        }

        // These should be classified as session settings
        assert!(is_session_setting("no page"));
        assert!(is_session_setting("terminal length 0"));
        assert!(is_session_setting("terminal pager 0"));
        assert!(is_session_setting("  no page  ")); // With whitespace

        // These should NOT be classified as session settings
        assert!(!is_session_setting("configure terminal"));
        assert!(!is_session_setting("interface 1"));
        assert!(!is_session_setting("no shutdown"));
        assert!(!is_session_setting("show running-config"));
        assert!(!is_session_setting("write memory"));
    }

    #[test]
    fn test_readonly_command_classification() {
        // Helper function to classify commands
        fn is_readonly(cmd: &str) -> bool {
            cmd.trim().starts_with("show ")
        }

        // These should be classified as read-only
        assert!(is_readonly("show running-config"));
        assert!(is_readonly("show vlan"));
        assert!(is_readonly("show interfaces"));
        assert!(is_readonly("show version"));
        assert!(is_readonly("  show running-config  ")); // With whitespace

        // These should NOT be classified as read-only
        assert!(!is_readonly("configure terminal"));
        assert!(!is_readonly("interface 1"));
        assert!(!is_readonly("no shutdown"));
        assert!(!is_readonly("showing")); // Starts with 'show' but not 'show '
    }

    // ============================================================================
    // Device Lock Tests (Existing)
    // ============================================================================

    #[test]
    #[cfg(unix)]
    fn test_check_device_lock_on_nonexistent_device() {
        // Test that checking a non-existent device doesn't panic
        let client = SerialClient::new("/dev/nonexistent_serial_device_12345".to_string(), 9600);

        // Should not error - the check will be performed when actual connection is attempted
        let result = client.check_device_lock();

        // Non-existent devices return Ok - the actual error will occur during tokio_serial open
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_check_device_lock_with_regular_file() {
        // Create a temporary file
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test_file");
        let mut file = File::create(&file_path).unwrap();
        file.write_all(b"test data").unwrap();
        drop(file);

        // Test lock detection with unlocked file
        let client = SerialClient::new(file_path.to_str().unwrap().to_string(), 9600);
        let result = client.check_device_lock();

        // Should succeed - file exists and is not locked
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn test_check_device_lock_with_locked_file() {
        use std::os::unix::io::AsRawFd;

        // Create a temporary file
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("locked_test_file");
        let file = File::create(&file_path).unwrap();

        // Acquire an exclusive lock on the file
        let fd = file.as_raw_fd();
        let lock_result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(lock_result, 0, "Failed to lock test file");

        // Now try to check the lock with SerialClient
        let client = SerialClient::new(file_path.to_str().unwrap().to_string(), 9600);
        let result = client.check_device_lock();

        // Should fail with lock error
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("locked by another process"),
            "Expected lock error message, got: {}",
            error_msg
        );

        // Clean up: release the lock
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        drop(file);
    }

    #[test]
    #[cfg(unix)]
    fn test_check_device_lock_nonexistent_path() {
        // Test with a path in a directory that doesn't exist
        let client = SerialClient::new("/nonexistent/directory/device".to_string(), 9600);
        let result = client.check_device_lock();

        // Should return Ok - path errors are handled during actual connection attempt
        // The check_device_lock only catches lock issues
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(not(unix))]
    fn test_check_device_lock_on_non_unix() {
        // On non-Unix systems, the check should always succeed
        let client = SerialClient::new("COM1".to_string(), 9600);
        let result = client.check_device_lock();

        assert!(result.is_ok());
    }

    #[test]
    fn test_serial_client_creation() {
        let client = SerialClient::new("/dev/ttyUSB0".to_string(), 115200);

        assert_eq!(client.device, "/dev/ttyUSB0");
        assert_eq!(client.baud_rate, 115200);
        assert!(client.port.is_none());
    }

    #[test]
    fn test_serial_client_with_debug_mode() {
        let client = SerialClient::new("/dev/ttyUSB0".to_string(), 9600)
            .with_debug_mode(true);

        assert_eq!(client.debug_mode, true);
    }

    #[test]
    fn test_serial_client_with_dry_run() {
        let client = SerialClient::new("/dev/ttyUSB0".to_string(), 9600)
            .with_dry_run(true);

        assert_eq!(client.dry_run, true);
    }

    // Tests for connect_with_retry behavior

    #[tokio::test]
    async fn test_serial_connect_with_retry_nonexistent_device() {
        // Test that serial connect_with_retry correctly fails after max retries
        // when connecting to a non-existent device
        let mut client = SerialClient::new("/dev/ttyNOTEXIST".to_string(), 9600);

        // Should fail after retries
        let result = client.connect_with_retry(1, 1).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("/dev/ttyNOTEXIST"));
    }

    #[tokio::test]
    async fn test_serial_connect_with_retry_multiple_attempts() {
        // Test that serial connect_with_retry attempts multiple times
        let mut client = SerialClient::new("/dev/ttyNOTEXIST".to_string(), 9600);

        let start = std::time::Instant::now();
        let result = client.connect_with_retry(3, 1).await;
        let _elapsed = start.elapsed();

        // Should have made 3 attempts
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("/dev/ttyNOTEXIST"));
        assert!(error_msg.contains("3 attempts"));
    }

    #[tokio::test]
    async fn test_serial_connect_with_retry_zero_retries() {
        // Test behavior with max_retries = 0 (should still attempt once due to .max(1))
        let mut client = SerialClient::new("/dev/ttyNOTEXIST".to_string(), 9600);

        // With max_retries = 0, it should still attempt once (enforced by .max(1))
        let result = client.connect_with_retry(0, 1).await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        // Should have made 1 attempt
        assert!(error_msg.contains("1 attempts"));
    }

    // ============================================================================
    // Additional Unit Tests
    // ============================================================================

    #[test]
    fn test_ctrl_c_in_login_sequence() {
        // Verify that the Ctrl-C character (\x03) is a valid byte that can be
        // sent via serial. The login() method sends \x03 to break out of stuck
        // states (config mode, "-- MORE --" prompts, etc.).
        let ctrl_c = "\x03";
        let bytes = ctrl_c.as_bytes();

        assert_eq!(bytes.len(), 1, "Ctrl-C should be a single byte");
        assert_eq!(bytes[0], 0x03, "Ctrl-C should be byte 0x03");

        // Verify it can be embedded in a larger string (as done in login flow)
        let with_cr = format!("{}\r", ctrl_c);
        assert_eq!(with_cr.as_bytes(), &[0x03, 0x0D]);

        // Verify the raw string used in send_raw calls
        let raw = "\x03";
        assert_eq!(raw.len(), 1);
        assert_eq!(raw.as_bytes()[0], 3u8);
    }

    #[test]
    fn test_config_mode_detection() {
        // Test the ANSI-stripped config mode detection logic used in login().
        // When the switch is in config mode, we need to send "end" to return
        // to privileged exec mode. The check uses contains("(config") on
        // ANSI-stripped output.
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();

        // Helper: strip ANSI and check for config mode, matching the login() logic
        let is_config_mode = |input: &str| -> bool {
            let stripped = ansi_regex.replace_all(input, "");
            stripped.contains("(config")
        };

        // Should detect config mode
        assert!(
            is_config_mode("hostname(config)#"),
            "Should detect basic config mode"
        );
        assert!(
            is_config_mode("hostname(config-if)#"),
            "Should detect interface config mode"
        );
        assert!(
            is_config_mode("hostname(config-vlan)#"),
            "Should detect VLAN config mode"
        );
        assert!(
            is_config_mode("\x1b[24;1Hhostname(config)# \x1b[?25h"),
            "Should detect config mode with ANSI escape sequences"
        );

        // Should NOT detect config mode for these
        assert!(
            !is_config_mode("hostname#"),
            "Plain privileged exec prompt should NOT be config mode"
        );
        assert!(
            !is_config_mode("hostname>"),
            "User mode prompt should NOT be config mode"
        );

        // The login() code checks for "(config" not "(config)", so sub-modes
        // like (config-if) also match. Verify parenthetical contexts that do
        // NOT start with "config" are not matched.
        assert!(
            !is_config_mode("hostname(vlan-42)#"),
            "VLAN context (not config sub-mode) should NOT match config mode"
        );
    }

    #[test]
    fn test_session_setting_and_auth_command_classification() {
        // Verify the dry-run allow-list logic from execute_command().
        // Mirrors the exact classification used in lines 795-801.
        fn is_session_setting(cmd: &str) -> bool {
            cmd.trim() == "no page"
                || cmd.trim() == "terminal length 0"
                || cmd.trim() == "terminal pager 0"
                || cmd.trim() == "enable"
                || cmd.trim() == "end"
        }

        fn is_readonly(cmd: &str) -> bool {
            cmd.trim().starts_with("show ") || cmd.trim().starts_with("get ")
        }

        // Session settings: allowed during dry-run
        assert!(is_session_setting("no page"), "'no page' is a session setting");
        assert!(is_session_setting("terminal length 0"), "'terminal length 0' is a session setting");
        assert!(is_session_setting("terminal pager 0"), "'terminal pager 0' is a session setting");
        assert!(is_session_setting("enable"), "'enable' is a session setting");
        assert!(is_session_setting("end"), "'end' is a session setting");

        // Read-only commands: allowed during dry-run
        assert!(is_readonly("show running-config"), "'show running-config' is readonly");
        assert!(is_readonly("show version"), "'show version' is readonly");
        assert!(is_readonly("get system status"), "'get system status' is readonly (FortiSwitch)");

        // Configuration commands: should NOT be in either category
        assert!(
            !is_session_setting("configure terminal") && !is_readonly("configure terminal"),
            "'configure terminal' is neither session setting nor readonly"
        );
        assert!(
            !is_session_setting("vlan 42") && !is_readonly("vlan 42"),
            "'vlan 42' is neither session setting nor readonly"
        );
        assert!(
            !is_session_setting("write memory") && !is_readonly("write memory"),
            "'write memory' is neither session setting nor readonly"
        );
    }

    #[test]
    fn test_auth_mode_bypasses_dry_run() {
        // Verify the auth_mode flag logic: when auth_mode is true, even
        // non-readonly/non-session commands should be allowed through the
        // dry-run gate. This simulates the enable credential flow where
        // passwords must be sent as interactive responses.
        //
        // The gate condition from execute_command():
        //   if self.dry_run && !self.auth_mode && !is_readonly && !is_session_setting { SKIP }

        fn would_skip_in_dry_run(dry_run: bool, auth_mode: bool, cmd: &str) -> bool {
            let is_readonly = cmd.trim().starts_with("show ")
                || cmd.trim().starts_with("get ");
            let is_session_setting = cmd.trim() == "no page"
                || cmd.trim() == "terminal length 0"
                || cmd.trim() == "terminal pager 0"
                || cmd.trim() == "enable"
                || cmd.trim() == "end";

            dry_run && !auth_mode && !is_readonly && !is_session_setting
        }

        // With dry_run=true and auth_mode=false, config commands are skipped
        assert!(
            would_skip_in_dry_run(true, false, "vlan 42"),
            "Config commands should be skipped in dry-run without auth_mode"
        );
        assert!(
            would_skip_in_dry_run(true, false, "configure terminal"),
            "'configure terminal' should be skipped in dry-run without auth_mode"
        );
        assert!(
            would_skip_in_dry_run(true, false, "write memory"),
            "'write memory' should be skipped in dry-run without auth_mode"
        );

        // With dry_run=true and auth_mode=true, nothing is skipped
        assert!(
            !would_skip_in_dry_run(true, true, "vlan 42"),
            "Config commands should NOT be skipped when auth_mode is true"
        );
        assert!(
            !would_skip_in_dry_run(true, true, "configure terminal"),
            "'configure terminal' should NOT be skipped when auth_mode is true"
        );
        assert!(
            !would_skip_in_dry_run(true, true, "some-password-string"),
            "Password strings should NOT be skipped when auth_mode is true"
        );

        // Read-only and session settings are never skipped in dry-run
        assert!(
            !would_skip_in_dry_run(true, false, "show running-config"),
            "Read-only commands should NOT be skipped in dry-run"
        );
        assert!(
            !would_skip_in_dry_run(true, false, "no page"),
            "Session settings should NOT be skipped in dry-run"
        );
        assert!(
            !would_skip_in_dry_run(true, false, "enable"),
            "'enable' should NOT be skipped in dry-run (it's a session setting)"
        );

        // With dry_run=false, nothing is ever skipped
        assert!(
            !would_skip_in_dry_run(false, false, "vlan 42"),
            "Nothing should be skipped when dry_run is false"
        );
        assert!(
            !would_skip_in_dry_run(false, false, "write memory"),
            "Nothing should be skipped when dry_run is false"
        );

        // Also verify set_auth_mode works on the struct
        let mut client = SerialClient::new("/dev/null".to_string(), 9600)
            .with_dry_run(true);
        assert!(!client.auth_mode, "auth_mode should default to false");
        client.set_auth_mode(true);
        assert!(client.auth_mode, "auth_mode should be true after set_auth_mode(true)");
        client.set_auth_mode(false);
        assert!(!client.auth_mode, "auth_mode should be false after set_auth_mode(false)");
    }

    // ============================================================================
    // Session Terminated Recovery Tests
    // ============================================================================

    #[test]
    fn test_session_terminated_detection() {
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();

        // Real output from an Aruba switch with a stale session
        let raw_output = "\x1b[2J\x1b[?7h\x1b[1;23r\x1b[?6l\x1b[1;1H\x1b[?25l\x1b[1;1HHP J9773A 2530-24G-PoEP Switch\nSoftware revision YA.16.11.0016\n\n (C) Copyright 2024 Hewlett Packard Enterprise\n\n\x1b[10;5HSession Terminated, login timed out.\n\x1b[?25h\x1b[19;1H";
        let clean = ansi_regex.replace_all(raw_output, "");
        assert!(
            clean.contains("Session Terminated"),
            "Should detect 'Session Terminated' in ANSI-stripped output: {:?}",
            clean
        );
    }

    #[test]
    fn test_enter_privileged_mode_ansi_stripping() {
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();

        // Simulated ANSI-laden prompt response
        let raw_prompt = "\x1b[24;1H\x1b[?25lShadow-Switch-1# \x1b[?25h\x1b[24;19H";
        let clean = ansi_regex.replace_all(raw_prompt, "");
        let last_line = clean.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();

        assert!(
            last_line.ends_with('#'),
            "Should detect privileged prompt after ANSI stripping: {:?}",
            last_line
        );
    }

    #[test]
    fn test_verify_connectivity_detects_password_prompt() {
        // When verify_connectivity gets a "Password:" prompt, it means
        // the session wasn't authenticated — this should be caught
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
        let response = "\x1b[24;1HPassword: \x1b[?25h\x1b[24;11H";
        let clean = ansi_regex.replace_all(response, "");
        let clean_trimmed = clean.trim();

        assert!(
            clean_trimmed.contains("Password:"),
            "Should detect Password: prompt in response"
        );
        assert!(
            !clean_trimmed.contains('#') && !clean_trimmed.contains('>'),
            "Should NOT contain a command prompt"
        );
    }
}

impl Drop for SerialClient {
    fn drop(&mut self) {
        if self.port.is_some() {
            warn!("SerialClient dropped without explicit disconnect");
        }
    }
}
