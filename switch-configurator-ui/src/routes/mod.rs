pub mod dashboard;
pub mod diff;
pub mod edit;
pub mod events;
pub mod save;
pub mod switch;

use crate::proxy::BackendClient;
use crate::state::DraftStore;
use axum::{routing::{get, post}, Router};
use axum::http::header;

#[derive(Clone)]
pub struct AppState {
    pub backend: BackendClient,
    pub drafts: DraftStore,
}

async fn static_css() -> ([(header::HeaderName, &'static str); 1], &'static str) {
    ([(header::CONTENT_TYPE, "text/css")],
     include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/style.css")))
}

async fn static_htmx() -> ([(header::HeaderName, &'static str); 1], &'static [u8]) {
    ([(header::CONTENT_TYPE, "application/javascript")],
     include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/htmx.min.js")))
}

async fn static_htmx_sse() -> ([(header::HeaderName, &'static str); 1], &'static [u8]) {
    ([(header::CONTENT_TYPE, "application/javascript")],
     include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/htmx-sse.js")))
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard::index))
        .route("/health", get(health))
        .route("/switch/:id", get(switch::detail))
        .route("/switch/:id/:tab", get(switch::detail_with_tab))
        .route("/switch/:id/tab/:tab", get(switch::tab_content))
        // Edit views
        .route("/switch/:id/edit/vlans", get(edit::edit_vlans))
        .route("/switch/:id/edit/ports", get(edit::edit_ports))
        .route("/switch/:id/edit/mirrors", get(edit::edit_mirrors))
        .route("/switch/:id/edit/snmp", get(edit::edit_snmp))
        // Draft lifecycle
        .route("/draft/:id/start", post(edit::start_draft))
        .route("/draft/:id/discard", get(edit::discard_draft))
        // VLAN CRUD
        .route("/draft/:id/vlan/add", post(edit::add_vlan))
        .route("/draft/:id/vlan/:vlan_id", post(edit::update_vlan))
        .route("/draft/:id/vlan/:vlan_id/remove", get(edit::remove_vlan))
        // Port CRUD
        .route("/draft/:id/ports", post(edit::update_ports))
        .route("/draft/:id/port/add", post(edit::add_port))
        .route("/draft/:id/port/:port_id/remove", get(edit::remove_port))
        // Mirror CRUD
        .route("/draft/:id/mirror/add", post(edit::add_mirror))
        .route("/draft/:id/mirror/:session_id", post(edit::update_mirror))
        .route("/draft/:id/mirror/:session_id/remove", get(edit::remove_mirror))
        // SNMP CRUD
        .route("/draft/:id/snmp/community/add", post(edit::add_snmp_community))
        .route("/draft/:id/snmp/community/:name/remove", get(edit::remove_snmp_community))
        .route("/draft/:id/snmp/trap-receiver/add", post(edit::add_snmp_trap_receiver))
        .route("/draft/:id/snmp/trap-receiver/:host/remove", get(edit::remove_snmp_trap_receiver))
        .route("/draft/:id/snmp/traps", post(edit::update_snmp_traps))
        .route("/preview/:id", post(diff::preview))
        .route("/preview/:id/commands", get(diff::commands))
        .route("/preview/:id/yaml", get(diff::yaml_diff))
        .route("/save/:id", get(save::save_dialog))
        .route("/save/:id/confirm", post(save::save_overlay))
        // PoE reset
        .route("/switch/:id/poe-reset/:port_id", post(switch::poe_reset))
        // Overlay management (broken config)
        .route("/overlay/:switch_id/:filename/view", get(dashboard::view_overlay))
        .route("/overlay/:switch_id/:filename/delete", post(dashboard::delete_overlay))
        .route("/events", get(events::sse_proxy))
        .route("/static/style.css", get(static_css))
        .route("/static/htmx.min.js", get(static_htmx))
        .route("/static/htmx-sse.js", get(static_htmx_sse))
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
