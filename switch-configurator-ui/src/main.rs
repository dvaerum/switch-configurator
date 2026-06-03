mod config;
mod proxy;
mod routes;
mod state;

use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{info, Level};

#[derive(Parser, Debug)]
#[command(author, version, about = "Web UI for switch-configurator")]
struct Args {
    /// Backend unix socket path
    #[arg(long)]
    backend_socket: Option<PathBuf>,

    /// Backend TCP URL (alternative to socket)
    #[arg(long, default_value = "http://localhost:4002")]
    backend_url: String,

    /// Listen address for the UI server
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: SocketAddr,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: Level,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    let backend = if let Some(socket) = &args.backend_socket {
        info!("Connecting to backend via unix socket: {}", socket.display());
        config::BackendTransport::UnixSocket(socket.clone())
    } else {
        info!("Connecting to backend via TCP: {}", args.backend_url);
        config::BackendTransport::Tcp(args.backend_url.clone())
    };

    let backend_client = proxy::BackendClient::new(backend);
    let draft_store = state::DraftStore::new();

    let app_state = routes::AppState {
        backend: backend_client,
        drafts: draft_store,
    };

    let app = routes::create_router(app_state);

    info!("Starting UI server on {}", args.listen);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
