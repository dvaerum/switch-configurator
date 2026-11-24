//! `GET /switches/{id}/poe-power`: watts per PoE port, read from the console.
//!
//! SNMP has no per-port watts (docs/learnings/snmp-fortiswitch.md), so this takes
//! the switch's console turn like any other console read and is not part of
//! `/metrics`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::handlers::{queue_timeout_response, QUEUE_MAX_WAIT_SYNC};
use crate::config::ConfigStore;
use crate::models::{PoePortPower, Vendor};

pub async fn get_poe_power(State(store): State<ConfigStore>, Path(id): Path<String>) -> Response {
    let switch_config = {
        let config = store.config.read().await;
        match config.switches.iter().find(|s| s.id == id) {
            Some(cfg) => cfg.clone(),
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": format!("Switch '{}' not found", id)}))).into_response();
            }
        }
    };
    let model = switch_config.model();
    let (command, parse): (&str, fn(&str) -> Vec<PoePortPower>) = match (model.supports_poe(), model.vendor()) {
        (true, Vendor::Fortiswitch) => ("get switch poe inline", crate::vendors::fortiswitch::parse_poe_inline_power),
        (true, Vendor::Aruba) => ("show power-over-ethernet brief", crate::vendors::aruba::parse_poe_brief_power),
        (true, Vendor::Cisco) => ("show power inline", crate::vendors::cisco::parse_poe_inline_power),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Switch model {:?} has no PoE power listing", model)})),
            )
                .into_response();
        }
    };

    let guard = match store.status.acquire_configuring_queued(id.clone(), QUEUE_MAX_WAIT_SYNC).await {
        Some(guard) => guard,
        None => return queue_timeout_response(&id, QUEUE_MAX_WAIT_SYNC),
    };
    let listing = match store.session_pool.checkout(&switch_config).await {
        Err(e) => Err(format!("Connection failed: {}", e)),
        Ok(mut session) => match session.vendor().execute_raw_commands(&[command.to_string()]).await {
            Ok(outputs) => {
                session.release().await;
                Ok(outputs.join("\n"))
            }
            Err(e) => {
                session.discard().await;
                Err(e.to_string())
            }
        },
    };
    guard.release().await;

    let ports = match listing {
        Ok(output) => parse(&output),
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": crate::secrets::scrub(&e)}))).into_response(),
    };
    if ports.is_empty() {
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": "the switch printed no PoE listing"}))).into_response();
    }
    let total: f64 = ports.iter().filter_map(|p| p.draw_watts).sum();
    (
        StatusCode::OK,
        Json(json!({"switch_id": id, "source": "console", "total_draw_watts": total, "ports": ports})),
    )
        .into_response()
}
