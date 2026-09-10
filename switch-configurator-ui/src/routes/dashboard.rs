use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};

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

#[derive(Debug, Clone)]
pub struct OverlayFileInfo {
    pub filename: String,
    pub full_path: String,
}

/// One config file that contributed to a switch's (failed) merge. `is_main`
/// marks the main config specifically, so the dashboard can link it to a
/// read-only view — it's never offered for edit/delete like a genuine
/// overlay, but you still need to be able to see what it declares, since the
/// ambiguous-VLAN-name error names it directly when relevant.
#[derive(Debug, Clone)]
pub struct ConfigSourceView {
    pub path: String,
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub struct ValidationFailureView {
    pub switch_id: String,
    pub hostname: String,
    pub error: String,
    pub config_sources: Vec<ConfigSourceView>,
    pub overlay_files: Vec<OverlayFileInfo>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    switches: Vec<SwitchCard>,
    validation_failures: Vec<ValidationFailureView>,
}

pub async fn index(State(state): State<AppState>) -> impl IntoResponse {
    let (switches, validation_failures) = fetch_dashboard_data(&state).await;
    DashboardTemplate { switches, validation_failures }
}

async fn fetch_dashboard_data(state: &AppState) -> (Vec<SwitchCard>, Vec<ValidationFailureView>) {
    let switches = fetch_switch_cards(state).await;

    let status_json = state.backend.get("/api/status").await.ok();
    let failures = status_json.as_ref().map(build_validation_failures).unwrap_or_default();

    (switches, failures)
}

/// Turn `/api/status`'s raw JSON into per-switch validation-failure views,
/// splitting each failure's `config_sources` into the main config (never
/// offered for view/delete here — deleting it removes the switch's identity
/// fields entirely, not just an overlay) and genuine overlay files.
fn build_validation_failures(status_json: &serde_json::Value) -> Vec<ValidationFailureView> {
    let main_config = status_json["configuration"]["config_file"].as_str().unwrap_or("").to_string();

    status_json["validation_failures"].as_array()
        .map(|arr| arr.iter().map(|f| {
            let raw_sources: Vec<String> = f["config_sources"].as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            let overlay_files: Vec<OverlayFileInfo> = raw_sources.iter()
                .filter(|src| *src != &main_config)
                .filter(|src| src.ends_with(".yaml") || src.ends_with(".yml"))
                .filter_map(|src| {
                    std::path::Path::new(src)
                        .file_name()
                        .map(|name| OverlayFileInfo {
                            filename: name.to_string_lossy().to_string(),
                            full_path: src.clone(),
                        })
                })
                .collect();

            let config_sources: Vec<ConfigSourceView> = raw_sources.iter()
                .map(|src| ConfigSourceView {
                    path: src.clone(),
                    is_main: src == &main_config,
                })
                .collect();

            ValidationFailureView {
                switch_id: f["switch_id"].as_str().unwrap_or("").to_string(),
                hostname: f["hostname"].as_str().unwrap_or("unknown").to_string(),
                error: f["error"].as_str().unwrap_or("").to_string(),
                config_sources,
                overlay_files,
            }
        }).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_validation_failures_excludes_main_config_from_overlay_files() {
        // Regression test for a bug found while investigating a live incident:
        // the main config file was showing up alongside genuine overlays with
        // View/Delete buttons, because this function read config_file from
        // the wrong JSON path and the exclusion filter silently never matched.
        let status_json = serde_json::json!({
            "configuration": {
                "config_file": "/etc/main.yaml"
            },
            "validation_failures": [{
                "switch_id": "IT-90297",
                "hostname": "IT-90297",
                "error": "ambiguous VLAN name",
                "config_sources": ["/etc/main.yaml", "/etc/switch-configurator/overlay.yaml"]
            }]
        });

        let failures = build_validation_failures(&status_json);
        assert_eq!(failures.len(), 1);

        let overlay_filenames: Vec<&str> = failures[0].overlay_files.iter()
            .map(|f| f.filename.as_str())
            .collect();

        assert_eq!(
            overlay_filenames, vec!["overlay.yaml"],
            "main config should never be offered as a deletable overlay file"
        );
    }

    #[test]
    fn test_build_validation_failures_marks_main_config_source() {
        // The main config isn't offered for delete, but it should still be
        // viewable (read-only) — the dashboard needs to know which of
        // config_sources is the main one to link it to that read-only view.
        let status_json = serde_json::json!({
            "configuration": {
                "config_file": "/etc/main.yaml"
            },
            "validation_failures": [{
                "switch_id": "IT-90297",
                "hostname": "IT-90297",
                "error": "ambiguous VLAN name",
                "config_sources": ["/etc/main.yaml", "/etc/switch-configurator/overlay.yaml"]
            }]
        });

        let failures = build_validation_failures(&status_json);
        let sources = &failures[0].config_sources;
        assert_eq!(sources.len(), 2);

        let main = sources.iter().find(|s| s.path == "/etc/main.yaml").unwrap();
        assert!(main.is_main, "main config source should be marked is_main");

        let overlay = sources.iter().find(|s| s.path == "/etc/switch-configurator/overlay.yaml").unwrap();
        assert!(!overlay.is_main, "overlay source should not be marked is_main");
    }
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

    // Switch IDs currently being applied (rendered as "Configuring")
    let configuring: std::collections::HashSet<String> = status_json
        .as_ref()
        .and_then(|s| s["currently_configuring"].as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

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
                    // Normalize the raw last_result into the badge states the template
                    // matches. `record_apply_failure` stores "failed: <error>", so match
                    // on a prefix rather than the exact string, and let an in-progress
                    // apply win over a stale last_result.
                    let status = if configuring.contains(&id) {
                        Some("configuring".to_string())
                    } else {
                        match s["last_result"].as_str() {
                            Some(r) if r.starts_with("success") => Some("success".to_string()),
                            Some(r) if r.starts_with("failed") => Some("failed".to_string()),
                            _ => None,
                        }
                    };
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

#[derive(Template)]
#[template(path = "overlay_view.html")]
struct OverlayViewTemplate {
    switch_id: String,
    filename: String,
    content: String,
    error: Option<String>,
}

/// Read-only raw dump of an overlay file — useful to double-check exactly
/// what's on disk, but resolving a broken switch happens through the
/// structured VLAN/port editor (`edit.rs`'s draft flow, seeded from
/// `/switches/{id}/merge-preview`), not by editing this raw text.
pub async fn view_overlay(
    State(state): State<AppState>,
    Path((switch_id, filename)): Path<(String, String)>,
) -> impl IntoResponse {
    let path = format!("/switches/{}/overlay/{}", switch_id, filename);
    let (content, error) = match state.backend.get_text(&path).await {
        Ok(text) => (text, None),
        Err(e) => (String::new(), Some(format!("Failed to load overlay: {}", e))),
    };

    OverlayViewTemplate {
        switch_id,
        filename,
        content,
        error,
    }
}

#[derive(Template)]
#[template(path = "main_config_view.html")]
struct MainConfigViewTemplate {
    content: String,
    error: Option<String>,
}

/// Read-only view of the main config — deliberately no edit or delete action
/// here. It carries a switch's identity fields, not a disposable overlay, but
/// the ambiguous-VLAN-name error can name it as one side of a collision, so
/// it needs to be inspectable from the dashboard even though it's never
/// editable from here.
pub async fn view_main_config(State(state): State<AppState>) -> impl IntoResponse {
    match state.backend.get_text("/config/main-file").await {
        Ok(content) => MainConfigViewTemplate { content, error: None },
        Err(e) => MainConfigViewTemplate { content: String::new(), error: Some(format!("Failed to load main config: {}", e)) },
    }
}

pub async fn delete_overlay(
    State(state): State<AppState>,
    Path((switch_id, filename)): Path<(String, String)>,
) -> impl IntoResponse {
    let path = format!("/switches/{}/overlay/{}", switch_id, filename);
    match state.backend.delete(&path).await {
        Ok((status, _)) if status < 300 => {
            tracing::info!("Deleted overlay {} for {}", filename, switch_id);
        }
        Ok((status, body)) => {
            tracing::error!("Failed to delete overlay: {} {:?}", status, body);
        }
        Err(e) => {
            tracing::error!("Failed to delete overlay: {}", e);
        }
    }

    Redirect::to("/")
}
