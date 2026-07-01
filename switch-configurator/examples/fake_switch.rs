//! Minimal fake Aruba-style switch over SSH, for end-to-end testing of features
//! like PoE reset without real hardware. It accepts any password, opens a shell
//! with a `FakeSwitch# ` prompt, echoes each command back with a fresh prompt,
//! and logs every command it receives so tests can assert what was sent.
//!
//! Run: cargo run --example fake_switch -- [port]   (default port 2222)

use std::sync::Arc;

use async_trait::async_trait;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, CryptoVec, Pty};

#[derive(Clone)]
struct FakeSwitch {
    paginate: bool,
}

impl Server for FakeSwitch {
    type Handler = Conn;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Conn {
        Conn {
            paginate: self.paginate,
            pending: Vec::new(),
        }
    }
}

struct Conn {
    /// When true, `show` output is broken into pages with "-- MORE --" prompts
    /// and `no page` is deliberately ignored — reproduces the Aruba 2530 where
    /// pagination stayed on and hung the reader.
    paginate: bool,
    /// Remaining pager pages to emit; each SPACE from the client sends the next.
    pending: Vec<Vec<u8>>,
}

const PROMPT: &[u8] = b"\r\nFakeSwitch# ";
const MORE: &[u8] = b"\r\n-- MORE --, next page: Space, next line: Enter, quit: Control-C";

/// A multi-page fake running-config so `show running-config` triggers several
/// "-- MORE --" prompts (the condition that used to hang the reader).
fn running_config_pages() -> Vec<Vec<u8>> {
    let page = |lines: &[&str]| lines.join("\r\n").into_bytes();
    vec![
        page(&[
            "Running configuration:",
            "",
            "hostname \"IT-04250\"",
            "module 1 type jl826a",
            "vlan 1",
            "   name \"DEFAULT_VLAN\"",
            "   untagged 1-24",
            "   exit",
        ]),
        page(&[
            "vlan 10",
            "   name \"mgmt\"",
            "   tagged 1",
            "   exit",
            "vlan 20",
            "   name \"users\"",
            "   tagged 2",
            "   exit",
        ]),
        page(&[
            "interface 1",
            "   name \"uplink\"",
            "   exit",
            "interface 2",
            "   name \"ap\"",
            "   exit",
            "password manager",
        ]),
    ]
}

#[async_trait]
impl Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, _password: &str) -> Result<Auth, Self::Error> {
        eprintln!("[fake-switch] auth_password user={user} -> ACCEPT");
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        _channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[fake-switch] shell opened");
        session.data(channel, CryptoVec::from_slice(PROMPT));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Mid-pagination: any keypress (the reader sends SPACE) reveals the next page.
        if !self.pending.is_empty() {
            let page = self.pending.remove(0);
            let mut resp = Vec::new();
            resp.extend_from_slice(b"\r\n");
            resp.extend_from_slice(&page);
            resp.extend_from_slice(if self.pending.is_empty() { PROMPT } else { MORE });
            session.data(channel, CryptoVec::from_slice(&resp));
            return Ok(());
        }

        let cmd = String::from_utf8_lossy(data);
        eprintln!("[fake-switch] recv: {:?}", cmd.trim_end());

        // In paginate mode a `show` command emits its output one page at a time,
        // each page (except the last) terminated by a "-- MORE --" prompt. `no page`
        // is intentionally NOT honored, mimicking the switch that caused the hang.
        if self.paginate && cmd.trim_start().starts_with("show ") {
            let mut pages = running_config_pages();
            let first = pages.remove(0);
            let mut resp = Vec::new();
            resp.extend_from_slice(data); // echo
            resp.extend_from_slice(b"\r\n");
            resp.extend_from_slice(&first);
            if pages.is_empty() {
                resp.extend_from_slice(PROMPT);
            } else {
                resp.extend_from_slice(MORE);
                self.pending = pages;
            }
            session.data(channel, CryptoVec::from_slice(&resp));
            return Ok(());
        }

        // Echo the command, then present a fresh prompt so wait_for_prompt() returns.
        let mut resp = Vec::new();
        resp.extend_from_slice(data);
        resp.extend_from_slice(PROMPT);
        session.data(channel, CryptoVec::from_slice(&resp));
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    // Args: [port] [--paginate]. --paginate makes `show` output multi-page with
    // "-- MORE --" prompts (and ignores `no page`) to exercise pager handling.
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args
        .iter()
        .skip(1)
        .find_map(|s| s.parse().ok())
        .unwrap_or(2222);
    let paginate = args.iter().any(|a| a == "--paginate");

    let key = russh_keys::key::KeyPair::generate_ed25519().expect("generate host key");
    let config = russh::server::Config {
        keys: vec![key],
        ..Default::default()
    };
    let mut server = FakeSwitch { paginate };
    eprintln!("[fake-switch] listening on 0.0.0.0:{port} (paginate={paginate})");
    server
        .run_on_address(Arc::new(config), ("0.0.0.0", port))
        .await
        .expect("fake switch server failed");
}
