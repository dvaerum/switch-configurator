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
struct FakeSwitch;

impl Server for FakeSwitch {
    type Handler = Conn;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Conn {
        Conn
    }
}

struct Conn;

const PROMPT: &[u8] = b"\r\nFakeSwitch# ";

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
        let cmd = String::from_utf8_lossy(data);
        eprintln!("[fake-switch] recv: {:?}", cmd.trim_end());
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
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2222);

    let key = russh_keys::key::KeyPair::generate_ed25519().expect("generate host key");
    let config = russh::server::Config {
        keys: vec![key],
        ..Default::default()
    };
    let mut server = FakeSwitch;
    eprintln!("[fake-switch] listening on 0.0.0.0:{port}");
    server
        .run_on_address(Arc::new(config), ("0.0.0.0", port))
        .await
        .expect("fake switch server failed");
}
