use askama::Template;
use axum::extract::{Path, State};
use axum::response::IntoResponse;

use super::AppState;

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub category: String,
    pub description: String,
    pub change_type: String, // "add", "remove", "update"
}

#[derive(Template)]
#[template(path = "partials/diff_preview.html")]
struct DiffPreviewTemplate {
    switch_id: String,
    has_changes: bool,
    entries: Vec<DiffEntry>,
}

#[derive(Template)]
#[template(path = "partials/command_preview.html")]
struct CommandPreviewTemplate {
    switch_id: String,
    vlan_commands: Vec<String>,
    port_commands: Vec<String>,
    mirror_commands: Vec<String>,
    snmp_commands: Vec<String>,
    reset_commands: Vec<String>,
}

#[derive(Template)]
#[template(path = "partials/yaml_diff.html")]
struct YamlDiffTemplate {
    switch_id: String,
    original_yaml: String,
    edited_yaml: String,
}

pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => {
            return DiffPreviewTemplate {
                switch_id: id,
                has_changes: false,
                entries: vec![],
            }.into_response();
        }
    };

    // Compute diff by posting to backend preview-diff with a synthetic current state from original
    let current_state = serde_json::json!({
        "current_state": {
            "vlans": draft.original.vlans,
            "ports": draft.original.ports,
            "port_mirrors": draft.original.port_mirrors,
            "snmp": draft.original.snmp,
            "warnings": []
        }
    });

    // First temporarily update the backend's desired config with the draft
    let _ = state.backend.post(
        &format!("/switches/{}/desired-config", id),
        &serde_json::to_value(&draft.edited).unwrap_or_default(),
    ).await;

    let result = state.backend.post(
        &format!("/switches/{}/preview-diff", id),
        &current_state,
    ).await;

    // Restore original config
    let _ = state.backend.post(
        &format!("/switches/{}/desired-config", id),
        &serde_json::to_value(&draft.original).unwrap_or_default(),
    ).await;

    let (has_changes, entries) = match result {
        Ok((_, json)) => {
            let has_changes = json["has_changes"].as_bool().unwrap_or(false);
            let mut entries = Vec::new();

            // Parse diff into structured entries
            if let Some(diff) = json.get("diff") {
                add_diff_entries(&mut entries, diff, "vlans_to_add", "VLAN", "add");
                add_diff_entries(&mut entries, diff, "vlans_to_remove", "VLAN", "remove");
                add_diff_entries(&mut entries, diff, "vlans_to_update", "VLAN", "update");
                add_diff_entries(&mut entries, diff, "ports_to_configure", "Port", "update");
                add_diff_entries(&mut entries, diff, "ports_to_reset", "Port", "remove");
                add_diff_entries(&mut entries, diff, "mirrors_to_add", "Mirror", "add");
                add_diff_entries(&mut entries, diff, "mirrors_to_remove", "Mirror", "remove");
                add_diff_entries(&mut entries, diff, "mirrors_to_update", "Mirror", "update");
            }

            (has_changes, entries)
        }
        Err(e) => {
            tracing::error!("Failed to compute diff: {}", e);
            (false, vec![DiffEntry {
                category: "Error".to_string(),
                description: format!("Failed to compute diff: {}", e),
                change_type: "error".to_string(),
            }])
        }
    };

    DiffPreviewTemplate {
        switch_id: id,
        has_changes,
        entries,
    }.into_response()
}

pub async fn commands(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => {
            return CommandPreviewTemplate {
                switch_id: id,
                vlan_commands: vec![],
                port_commands: vec![],
                mirror_commands: vec![],
                snmp_commands: vec![],
                reset_commands: vec![],
            }.into_response();
        }
    };

    let current_state = serde_json::json!({
        "current_state": {
            "vlans": draft.original.vlans,
            "ports": draft.original.ports,
            "port_mirrors": draft.original.port_mirrors,
            "snmp": draft.original.snmp,
            "warnings": []
        }
    });

    let result = state.backend.post(
        &format!("/switches/{}/preview-diff", id),
        &current_state,
    ).await;

    let (vlan_cmds, port_cmds, mirror_cmds, snmp_cmds, reset_cmds) = match result {
        Ok((_, json)) => {
            let cmds = &json["commands"];
            (
                json_str_array(cmds, "vlan_commands"),
                json_str_array(cmds, "port_commands"),
                json_str_array(cmds, "mirror_commands"),
                json_str_array(cmds, "snmp_commands"),
                json_str_array(cmds, "reset_commands"),
            )
        }
        Err(_) => (vec![], vec![], vec![], vec![], vec![]),
    };

    CommandPreviewTemplate {
        switch_id: id,
        vlan_commands: vlan_cmds,
        port_commands: port_cmds,
        mirror_commands: mirror_cmds,
        snmp_commands: snmp_cmds,
        reset_commands: reset_cmds,
    }.into_response()
}

pub async fn yaml_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => {
            return YamlDiffTemplate {
                switch_id: id,
                original_yaml: String::new(),
                edited_yaml: String::new(),
            }.into_response();
        }
    };

    let original_yaml = serde_yaml::to_string(&draft.original).unwrap_or_default();
    let edited_yaml = serde_yaml::to_string(&draft.edited).unwrap_or_default();

    YamlDiffTemplate {
        switch_id: id,
        original_yaml,
        edited_yaml,
    }.into_response()
}

fn add_diff_entries(entries: &mut Vec<DiffEntry>, diff: &serde_json::Value, field: &str, category: &str, change_type: &str) {
    if let Some(arr) = diff[field].as_array() {
        for item in arr {
            let desc = if let Some(id) = item.as_u64() {
                format!("{} {}", category, id)
            } else if let Some(id) = item.as_str() {
                format!("{} {}", category, id)
            } else if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let id = item.get("id").and_then(|i| i.as_u64()).map(|i| i.to_string())
                    .or_else(|| item.get("port_id").and_then(|p| p.as_str()).map(|s| s.to_string()))
                    .or_else(|| item.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .unwrap_or_default();
                format!("{} {} ({})", category, id, name)
            } else if let Some(port_id) = item.get("port_id").and_then(|p| p.as_str()) {
                format!("{} {}", category, port_id)
            } else {
                format!("{}", category)
            };
            entries.push(DiffEntry {
                category: category.to_string(),
                description: desc,
                change_type: change_type.to_string(),
            });
        }
    }
}

fn json_str_array(val: &serde_json::Value, field: &str) -> Vec<String> {
    val[field].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}
