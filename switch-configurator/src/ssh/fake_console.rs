//! A scripted FortiOS-style console on the far end of a pty pair, so the real
//! `SerialClient` login/command/probe code can be tested without hardware.
//! Real I/O underneath: tests that log in take a few seconds of the client's
//! own settle delays.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialStream;

use super::serial::SerialClient;

pub const PROMPT: &str = "S108FPTV21002683 # ";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Sitting at a command prompt.
    LoggedIn,
    /// Sitting at `login:`; username then password get the session back.
    LoggedOut,
    /// Like `LoggedOut`, but the next prompt says the session timed out.
    Terminated,
    /// Never answers (hung console / locked line).
    Silent,
}

#[derive(Default)]
struct Shared {
    mode: Option<Mode>,
    awaiting_password: bool,
    logins: usize,
    commands: Vec<String>,
    /// (command prefix, canned output) pairs; first match wins.
    outputs: Vec<(String, String)>,
}

/// Handle to the scripted peer; the peer task stops when this is dropped
/// along with the client's end of the pty.
pub struct FakeConsole {
    shared: Arc<Mutex<Shared>>,
}

impl FakeConsole {
    /// A client attached to a fake console that starts in `mode`.
    pub fn spawn(mode: Mode) -> (SerialClient, FakeConsole) {
        let (client_end, mut console_end) = SerialStream::pair().expect("pty pair");
        let shared = Arc::new(Mutex::new(Shared { mode: Some(mode), ..Default::default() }));

        let peer = shared.clone();
        tokio::spawn(async move {
            let mut line = String::new();
            let mut buf = [0u8; 256];
            loop {
                let n = match console_end.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                for &byte in &buf[..n] {
                    // Ctrl-C aborts the current line like Enter on an empty one.
                    let finished = byte == b'\r' || byte == b'\n' || byte == 0x03;
                    if !finished {
                        line.push(byte as char);
                        continue;
                    }
                    let reply = respond(&peer, std::mem::take(&mut line));
                    if !reply.is_empty() && console_end.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
        });

        (SerialClient::from_stream("fake-console".to_string(), client_end), FakeConsole { shared })
    }

    pub fn set_mode(&self, mode: Mode) {
        let mut shared = self.shared.lock().unwrap();
        shared.mode = Some(mode);
        shared.awaiting_password = false;
    }

    /// Answer commands starting with `prefix` with `output` instead of `OK`.
    pub fn set_output(&self, prefix: &str, output: &str) {
        self.shared.lock().unwrap().outputs.push((prefix.to_string(), output.to_string()));
    }

    /// Successful logins so far.
    pub fn logins(&self) -> usize {
        self.shared.lock().unwrap().logins
    }

    /// Commands received while logged in, in order.
    pub fn commands(&self) -> Vec<String> {
        self.shared.lock().unwrap().commands.clone()
    }
}

fn respond(shared: &Arc<Mutex<Shared>>, line: String) -> String {
    let mut s = shared.lock().unwrap();
    match s.mode.unwrap_or(Mode::Silent) {
        Mode::Silent => String::new(),
        Mode::LoggedIn => {
            if line.trim().is_empty() {
                format!("\r\n{PROMPT}")
            } else {
                s.commands.push(line.clone());
                let output = s
                    .outputs
                    .iter()
                    .find(|(prefix, _)| line.starts_with(prefix.as_str()))
                    .map_or("OK".to_string(), |(_, out)| out.clone());
                format!("{line}\r\n{output}\r\n{PROMPT}")
            }
        }
        mode @ (Mode::LoggedOut | Mode::Terminated) => {
            if s.awaiting_password {
                s.awaiting_password = false;
                s.logins += 1;
                s.mode = Some(Mode::LoggedIn);
                format!("\r\n{PROMPT}")
            } else if line.trim().is_empty() {
                let banner = if mode == Mode::Terminated { "Session Terminated, login timed out\r\n" } else { "" };
                format!("\r\n{banner}S108FPTV21002683 login: ")
            } else {
                s.awaiting_password = true;
                "\r\nPassword: ".to_string()
            }
        }
    }
}

/// Collects everything the `tracing` macros emit while it is installed.
#[derive(Clone, Default)]
pub(crate) struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuffer {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}


/// Install a log collector for the current thread: everything logged by the
/// `tracing` macros until the returned guard drops lands in the buffer.
pub(crate) fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_login_code_logs_in_against_the_fake_console() {
        let (mut client, console) = FakeConsole::spawn(Mode::LoggedOut);
        client.login("admin", "pw").await.expect("login against fake console");
        assert_eq!(console.logins(), 1);
    }

    #[tokio::test]
    async fn probe_accepts_a_logged_in_console() {
        let (mut client, _console) = FakeConsole::spawn(Mode::LoggedIn);
        client.probe_session().await.expect("logged-in console is alive");
    }

    #[tokio::test]
    async fn probe_rejects_a_logged_out_console() {
        let (mut client, _console) = FakeConsole::spawn(Mode::LoggedOut);
        let err = client.probe_session().await.unwrap_err().to_string();
        assert!(err.contains("login"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn probe_rejects_a_terminated_session() {
        let (mut client, _console) = FakeConsole::spawn(Mode::Terminated);
        let err = client.probe_session().await.unwrap_err().to_string();
        assert!(err.contains("login") || err.contains("Terminated"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn probe_rejects_a_silent_console() {
        let (mut client, _console) = FakeConsole::spawn(Mode::Silent);
        assert!(client.probe_session().await.is_err());
    }

    #[tokio::test]
    async fn probe_does_not_run_commands_on_the_switch() {
        let (mut client, console) = FakeConsole::spawn(Mode::LoggedIn);
        client.probe_session().await.unwrap();
        assert!(console.commands().is_empty(), "probe must only poke the prompt");
    }

    #[tokio::test]
    async fn command_log_masks_snmp_secrets() {
        let (logs, _installed) = capture_logs();

        let (mut client, console) = FakeConsole::spawn(Mode::LoggedIn);
        client.execute_command("snmp-server community \"s3cret-community\" unrestricted").await.unwrap();
        client.execute_command("config system snmp community").await.unwrap();
        client.execute_command("set name \"s3cret-forti\"").await.unwrap();
        client.execute_command("end").await.unwrap();

        // The switch still receives the real command...
        assert!(console.commands().iter().any(|c| c.contains("s3cret-community")));
        assert!(console.commands().iter().any(|c| c.contains("s3cret-forti")));
        // ...but nothing in the logs shows it.
        let text = logs.text();
        assert!(text.contains("Command"), "expected command logging, got: {text}");
        assert!(!text.contains("s3cret-community"), "community leaked into logs:\n{text}");
        assert!(!text.contains("s3cret-forti"), "FortiOS community leaked into logs:\n{text}");
    }

    #[tokio::test]
    async fn trace_logs_withhold_show_output_that_may_hold_secrets() {
        let (logs, _installed) = capture_logs();
        let (mut client, console) = FakeConsole::spawn(Mode::LoggedIn);
        console.set_output(
            "show system snmp community",
            "config system snmp community\r\n    edit 100\r\n        set name \"s3cret-readback\"\r\n    next\r\nend",
        );

        let output = client.execute_command("show system snmp community").await.unwrap();

        assert!(output.contains("s3cret-readback"), "callers still get the real output");
        let text = logs.text();
        assert!(text.contains("show system snmp community"), "the command itself stays visible:\n{text}");
        assert!(!text.contains("s3cret-readback"), "read-back leaked into trace logs:\n{text}");
    }
}
