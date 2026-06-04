use askama::Template;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use switch_configurator::models::*;

use super::AppState;

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub category: String,
    pub description: String,
    pub change_type: String,
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

    // Compute diff locally: original config as "current state", edited as "desired"
    let current_state = SwitchState {
        vlans: draft.original.vlans.clone(),
        ports: draft.original.ports.clone(),
        port_mirrors: draft.original.port_mirrors.clone(),
        snmp: draft.original.snmp.clone(),
        management_vlan: draft.original.management_vlan,
        warnings: vec![],
    };

    let diff = switch_configurator::diff::compute_diff(
        &current_state,
        &draft.edited,
        false,
    );

    let mut entries = Vec::new();

    // VLANs
    for vlan in &diff.vlans_to_add {
        entries.push(DiffEntry {
            category: "VLAN".to_string(),
            description: format!("Add VLAN {} ({})", vlan.id, vlan.name),
            change_type: "add".to_string(),
        });
    }
    for vlan_id in &diff.vlans_to_remove {
        entries.push(DiffEntry {
            category: "VLAN".to_string(),
            description: format!("Remove VLAN {}", vlan_id),
            change_type: "remove".to_string(),
        });
    }
    for vlan in &diff.vlans_to_update {
        entries.push(DiffEntry {
            category: "VLAN".to_string(),
            description: format!("Update VLAN {} ({})", vlan.id, vlan.name),
            change_type: "update".to_string(),
        });
    }

    // Ports
    for port in &diff.ports_to_configure {
        entries.push(DiffEntry {
            category: "Port".to_string(),
            description: format!("Configure port {} (VLAN {}{})",
                port.port_id, port.vlan,
                if !port.tagged_vlans.is_empty() {
                    format!(", tagged: {:?}", port.tagged_vlans)
                } else { String::new() }),
            change_type: "update".to_string(),
        });
    }
    for port_id in &diff.ports_to_reset {
        entries.push(DiffEntry {
            category: "Port".to_string(),
            description: format!("Reset port {} to default", port_id),
            change_type: "remove".to_string(),
        });
    }

    // Mirrors
    for mirror in &diff.mirrors_to_add {
        entries.push(DiffEntry {
            category: "Mirror".to_string(),
            description: format!("Add mirror session {} (src: {:?} → dst: {})",
                mirror.session_id, mirror.source_ports, mirror.destination_port),
            change_type: "add".to_string(),
        });
    }
    for session_id in &diff.mirrors_to_remove {
        entries.push(DiffEntry {
            category: "Mirror".to_string(),
            description: format!("Remove mirror session {}", session_id),
            change_type: "remove".to_string(),
        });
    }
    for mirror in &diff.mirrors_to_update {
        entries.push(DiffEntry {
            category: "Mirror".to_string(),
            description: format!("Update mirror session {}", mirror.session_id),
            change_type: "update".to_string(),
        });
    }

    // SNMP
    if let Some(snmp_diff) = &diff.snmp_diff {
        if snmp_diff.has_changes() {
            for c in &snmp_diff.communities_to_add {
                entries.push(DiffEntry {
                    category: "SNMP".to_string(),
                    description: format!("Add community '{}'", c.name),
                    change_type: "add".to_string(),
                });
            }
            for name in &snmp_diff.communities_to_remove {
                entries.push(DiffEntry {
                    category: "SNMP".to_string(),
                    description: format!("Remove community '{}'", name),
                    change_type: "remove".to_string(),
                });
            }
            for trap in &snmp_diff.traps_to_enable {
                entries.push(DiffEntry {
                    category: "SNMP".to_string(),
                    description: format!("Enable trap {:?}", trap),
                    change_type: "add".to_string(),
                });
            }
            for trap in &snmp_diff.traps_to_disable {
                entries.push(DiffEntry {
                    category: "SNMP".to_string(),
                    description: format!("Disable trap {:?}", trap),
                    change_type: "remove".to_string(),
                });
            }
        }
    }

    DiffPreviewTemplate {
        switch_id: id,
        has_changes: !entries.is_empty(),
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

    // Use backend preview-diff with the original as current_state
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
        Err(e) => {
            tracing::error!("Failed to get command preview: {}", e);
            (vec![format!("Error: {}", e)], vec![], vec![], vec![], vec![])
        }
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

fn json_str_array(val: &serde_json::Value, field: &str) -> Vec<String> {
    val[field].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}
