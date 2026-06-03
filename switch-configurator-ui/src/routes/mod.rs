pub mod dashboard;

use crate::proxy::BackendClient;
use crate::state::DraftStore;
use axum::{routing::get, Router};
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    pub backend: BackendClient,
    pub drafts: DraftStore,
}

pub fn create_router(state: AppState) -> Router {
    let static_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static");
    Router::new()
        .route("/", get(dashboard::index))
        .route("/health", get(health))
        .nest_service("/static", ServeDir::new(static_dir))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            backend: BackendClient::new(crate::config::BackendTransport::Tcp("http://localhost:9999".to_string())),
            drafts: DraftStore::new(),
        }
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = create_router(test_state());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_static_htmx_served() {
        let app = create_router(test_state());
        let req = Request::builder()
            .uri("/static/htmx.min.js")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "htmx.min.js should be served from static/");
    }
}
