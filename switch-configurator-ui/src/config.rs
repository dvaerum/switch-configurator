use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum BackendTransport {
    UnixSocket(PathBuf),
    Tcp(String),
}
