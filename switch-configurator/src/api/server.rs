use crate::config::{AppConfig, ConfigStore};
use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};
use std::net::SocketAddr;
use std::path::Path;
use tower_http::trace::TraceLayer;
use tracing::info;

use super::handlers;

/// API endpoint definitions - single source of truth
/// Used by both the router and /api/status response
pub const API_ENDPOINTS: &[&str] = &[
    "GET /health",
    "GET /api/status",
    "GET /switches",
    "POST /switches/:id/apply",
    "POST /switches/:id/reload",
    "GET /switches/:id/config",
    "GET /switches/:id/desired-config",
    "PUT /switches/:id/desired-config",
    "PATCH /switches/:id/desired-config",
    "DELETE /switches/:id/desired-config",
    "POST /config/reload",
];

pub async fn start(store: ConfigStore, socket_path: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let app = create_router(store.clone());

    if let Some(ref path) = socket_path {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let uds = tokio::net::UnixListener::bind(path)?;
        info!("API server listening on unix socket: {}", path.display());
        let uds_app = app.clone();
        tokio::spawn(async move {
            serve_unix_socket(uds, uds_app).await;
        });
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], store.api_port));
    info!("Starting API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

pub async fn serve_unix_socket(listener: tokio::net::UnixListener, app: Router) {
    use hyper::body::Incoming;
    use hyper_util::rt::TokioIo;
    use tower::Service;

    let mut make_service = app.into_make_service();

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Unix socket accept error: {}", e);
                continue;
            }
        };

        let tower_service = unwrap_infallible(make_service.call(&stream).await);

        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let hyper_service = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                tower_service.clone().call(req)
            });

            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, hyper_service)
                .await
            {
                if !e.is_incomplete_message() {
                    tracing::error!("Unix socket connection error: {}", e);
                }
            }
        });
    }
}

fn unwrap_infallible<T>(result: Result<T, std::convert::Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => match err {},
    }
}

pub fn create_router(store: ConfigStore) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/api/status", get(handlers::status))
        .route("/switches", get(handlers::list_switches))
        .route("/switches/:id/apply", post(handlers::apply_config))
        .route("/switches/:id/reload", post(handlers::reload_switch_config))
        // GET retrieves running config from switch hardware via SSH
        .route("/switches/:id/config", get(handlers::get_config))
        // PUT creates/replaces in-memory config, PATCH updates, DELETE removes
        .route(
            "/switches/:id/desired-config",
            get(handlers::get_desired_config)
                .put(handlers::set_switch_config)
                .patch(handlers::patch_switch_config)
                .delete(handlers::delete_switch_config),
        )
        .route("/config/reload", post(handlers::reload_config))
        .layer(TraceLayer::new_for_http())
        .with_state(store)
}
