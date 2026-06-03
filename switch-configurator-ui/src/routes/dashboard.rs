use askama::Template;
use axum::extract::State;
use axum::response::IntoResponse;

use super::AppState;

#[derive(Debug, Clone)]
pub struct SwitchCard {
    pub id: String,
    pub hostname: String,
    pub model: String,
    pub management_ip: String,
    pub vlan_count: usize,
    pub port_count: usize,
    pub status: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    switches: Vec<SwitchCard>,
}

pub async fn index(State(state): State<AppState>) -> impl IntoResponse {
    let switches = fetch_switch_cards(&state).await;
    DashboardTemplate { switches }
}

async fn fetch_switch_cards(state: &AppState) -> Vec<SwitchCard> {
    // Fetch switches list
    let switches_json = match state.backend.get("/switches").await {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to fetch switches: {}", e);
            return vec![];
        }
    };

    // Fetch status for warnings and last_result
    let status_json = state.backend.get("/api/status").await.ok();

    let switches = switches_json["switches"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    switches
        .iter()
        .map(|sw| {
            let id = sw["id"].as_str().unwrap_or("").to_string();

            // Look up status info for this switch
            let (status, warnings) = status_json
                .as_ref()
                .and_then(|s| s["switches"].as_array())
                .and_then(|arr| arr.iter().find(|s| s["id"].as_str() == Some(&id)))
                .map(|s| {
                    let status = s["last_result"].as_str().map(|s| s.to_string());
                    let warnings = s["warnings"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|w| w.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (status, warnings)
                })
                .unwrap_or((None, vec![]));

            SwitchCard {
                id,
                hostname: sw["hostname"].as_str().unwrap_or("unknown").to_string(),
                model: sw["model"].as_str().unwrap_or("unknown").to_string(),
                management_ip: sw["management_ip"].as_str().unwrap_or("").to_string(),
                vlan_count: sw["vlans"].as_u64().unwrap_or(0) as usize,
                port_count: sw["ports"].as_u64().unwrap_or(0) as usize,
                status,
                warnings,
            }
        })
        .collect()
}
