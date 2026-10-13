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
                format!("{line}\r\nOK\r\n{PROMPT}")
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
}
