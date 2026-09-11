use askama::Template;
use axum::extract::{Path, State};
use axum::response::IntoResponse;

use super::AppState;

#[derive(Debug, Clone)]
pub struct VlanView {
    pub id: u16,
    pub name: String,
    pub ip_display: String,
}

#[derive(Debug, Clone)]
pub struct PortView {
    pub port_id: String,
    pub vlan: u16,
    pub tagged_display: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub poe_enabled: bool,
    pub poe_supported: bool,
    pub speed_display: String,
}

#[derive(Debug, Clone)]
pub struct MirrorView {
    pub session_id: String,
    pub source_ports: Vec<String>,
    pub destination_port: String,
    pub direction_display: String,
}

#[derive(Debug, Clone)]
pub struct SnmpCommunityView {
    pub name: String,
    pub access_display: String,
}

#[derive(Debug, Clone)]
pub struct SnmpTrapReceiverView {
    pub host: String,
    pub community: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct SnmpView {
    pub communities: Vec<SnmpCommunityView>,
    pub trap_receivers: Vec<SnmpTrapReceiverView>,
    pub enabled_traps: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SwitchView {
    pub id: String,
    pub hostname: String,
    pub model: String,
    pub management_ip: String,
    pub vlans: Vec<VlanView>,
    pub ports: Vec<PortView>,
    pub mirrors: Vec<MirrorView>,
    pub snmp: Option<SnmpView>,
}

#[derive(Debug, Clone)]
pub struct ConfigSourceView {
    pub file: String,
    /// Just the file's basename — what the View/Delete overlay routes
    /// (`/overlay/{id}/{filename}/...`) actually key on.
    pub filename: String,
    pub priority: u64,
    pub source_type: String,
    pub is_main: bool,
}

#[derive(Template)]
#[template(path = "switch_detail.html")]
struct SwitchDetailTemplate {
    switch: SwitchView,
    active_tab: String,
    sources: Vec<ConfigSourceView>,
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    detail_with_tab(State(state), Path((id, "overview".to_string()))).await
}

pub async fn detail_with_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
) -> impl IntoResponse {
    let switch = fetch_switch_view(&state, &id).await;
    let sources = if tab == "sources" {
        fetch_config_sources(&state, &id).await
    } else {
        vec![]
    };

    SwitchDetailTemplate {
        switch,
        active_tab: tab,
        sources,
    }
}

/// HTMX tab content — returns just the partial HTML for the tab
pub async fn tab_content(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
) -> impl IntoResponse {
    let switch = fetch_switch_view(&state, &id).await;
    let sources = if tab == "sources" {
        fetch_config_sources(&state, &id).await
    } else {
        vec![]
    };

    // Return just the tab content partial
    match tab.as_str() {
        "vlans" => TabPartialTemplate { switch, tab, sources }.into_response(),
        "ports" => TabPartialTemplate { switch, tab, sources }.into_response(),
        "mirrors" => TabPartialTemplate { switch, tab, sources }.into_response(),
        "snmp" => TabPartialTemplate { switch, tab, sources }.into_response(),
        "sources" => TabPartialTemplate { switch, tab, sources }.into_response(),
        _ => TabPartialTemplate { switch, tab: "overview".to_string(), sources }.into_response(),
    }
}

#[derive(Template)]
#[template(source = r#"{% match tab.as_str() %}
    {% when "vlans" %}{% include "partials/vlan_table.html" %}
    {% when "ports" %}{% include "partials/port_table.html" %}
    {% when "mirrors" %}{% include "partials/mirror_table.html" %}
    {% when "snmp" %}{% include "partials/snmp_panel.html" %}
    {% when "sources" %}{% include "partials/config_sources.html" %}
    {% when _ %}{% include "partials/overview.html" %}
{% endmatch %}"#, ext = "html")]
struct TabPartialTemplate {
    switch: SwitchView,
    tab: String,
    sources: Vec<ConfigSourceView>,
}

async fn fetch_switch_view(state: &AppState, id: &str) -> SwitchView {
    let json = match state.backend.get(&format!("/switches/{}/desired-config", id)).await {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to fetch switch config: {}", e);
            return empty_switch(id);
        }
    };

    parse_switch_view(id, &json)
}

fn parse_switch_view(id: &str, json: &serde_json::Value) -> SwitchView {
    use switch_configurator::models::SwitchModel;
    let model: Option<SwitchModel> = json["model"]
        .as_str()
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok());

    let mut vlans: Vec<VlanView> = json["vlans"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|v| VlanView {
                    id: v["id"].as_u64().unwrap_or(0) as u16,
                    name: v["name"].as_str().unwrap_or("").to_string(),
                    ip_display: match v["ip_config"].as_str() {
                        Some("dhcp") => "DHCP".to_string(),
                        Some("none") | None => "-".to_string(),
                        Some(other) => other.to_string(),
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    vlans.sort_by_key(|v| v.id);

    let mut ports: Vec<PortView> = json["ports"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|p| {
                    let port_id = p["port_id"].as_str().unwrap_or("").to_string();
                    let poe_supported = model
                        .as_ref()
                        .map(|m| m.port_supports_poe(&port_id))
                        .unwrap_or(false);
                    PortView {
                        port_id,
                        vlan: p["vlan"].as_u64().unwrap_or(1) as u16,
                        tagged_display: p["tagged_vlans"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_u64())
                                    .map(|v| v.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                        description: p["description"].as_str().map(|s| s.to_string()),
                        enabled: p["enabled"].as_bool().unwrap_or(false),
                        poe_enabled: p["poe_enabled"].as_bool().unwrap_or(false),
                        poe_supported,
                        speed_display: p["speed_duplex"].as_str().unwrap_or("auto").to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ports.sort_by(|a, b| natural_port_sort(&a.port_id, &b.port_id));

    let mirrors = json["port_mirrors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|m| MirrorView {
                    session_id: m["session_id"].as_str().unwrap_or("").to_string(),
                    source_ports: m["source_ports"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default(),
                    destination_port: m["destination_port"].as_str().unwrap_or("").to_string(),
                    direction_display: m["direction"].as_str().unwrap_or("both").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let snmp = json.get("snmp").and_then(|s| {
        if s.is_null() {
            return None;
        }
        Some(SnmpView {
            communities: s["communities"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| SnmpCommunityView {
                            name: c["name"].as_str().unwrap_or("").to_string(),
                            access_display: c["access"].as_str().unwrap_or("").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            trap_receivers: s["trap_receivers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| SnmpTrapReceiverView {
                            host: r["host"].as_str().unwrap_or("").to_string(),
                            community: r["community"].as_str().unwrap_or("").to_string(),
                            version: r["version"].as_str().unwrap_or("").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            enabled_traps: s["enabled_traps"]
                .as_array()
                .map(|a| a.iter().filter_map(|t| t.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
        })
    });

    SwitchView {
        id: id.to_string(),
        hostname: json["hostname"].as_str().unwrap_or(id).to_string(),
        model: json["model"].as_str().unwrap_or("unknown").to_string(),
        management_ip: json["management_ip"].as_str().unwrap_or("").to_string(),
        vlans,
        ports,
        mirrors,
        snmp,
    }
}

/// Build one Config Sources row from the backend's raw fields. `filename`
/// (the basename) is what the existing overlay View/Delete routes
/// (`/overlay/{id}/{filename}/...`) key on — they take a bare filename, not
/// a full path.
fn build_config_source_view(file: &str, priority: u64, source_type: &str) -> ConfigSourceView {
    let filename = std::path::Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());

    ConfigSourceView {
        file: file.to_string(),
        filename,
        priority,
        source_type: source_type.to_string(),
        is_main: source_type == "main",
    }
}

async fn fetch_config_sources(state: &AppState, id: &str) -> Vec<ConfigSourceView> {
    match state.backend.get(&format!("/switches/{}/config-sources", id)).await {
        Ok(json) => {
            json["sources"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|s| build_config_source_view(
                            s["file"].as_str().unwrap_or(""),
                            s["priority"].as_u64().unwrap_or(0),
                            s["source_type"].as_str().unwrap_or(""),
                        ))
                        .collect()
                })
                .unwrap_or_default()
        }
        Err(e) => {
            tracing::error!("Failed to fetch config sources: {}", e);
            vec![]
        }
    }
}

/// Parse a config file's YAML content and build a read-only `SwitchView`
/// for every switch it declares — so a raw overlay or main-config file can
/// be shown with the same structured tables the Edit flow already uses,
/// instead of only a raw text dump. Each `SwitchView` reflects exactly what
/// *this file* declares (identity fields it doesn't set stay blank/unknown
/// — this is not the merged switch).
pub fn parse_switch_views_from_yaml(yaml_content: &str) -> Vec<SwitchView> {
    let parsed: switch_configurator::config::AppConfigFile = match serde_yaml::from_str(yaml_content) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    parsed.config.switches.iter()
        .filter_map(|switch| {
            let json = serde_json::to_value(switch).ok()?;
            Some(parse_switch_view(&switch.id, &json))
        })
        .collect()
}

/// Same as `parse_switch_views_from_yaml`, scoped to one switch id — used
/// by the overlay view (which is always for a specific switch). `None` if
/// the file doesn't parse or doesn't declare this switch.
pub fn parse_switch_view_from_yaml(switch_id: &str, yaml_content: &str) -> Option<SwitchView> {
    parse_switch_views_from_yaml(yaml_content)
        .into_iter()
        .find(|v| v.id == switch_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERLAY_YAML: &str = r#"
merge_priority: 200
switches:
  - id: IT-90297
    hostname: IT-90297
    vlans:
      - id: 101
        name: philips-ap-z1
        description: null
        ip_config: none
    ports:
      - port_id: "1"
        vlan: 101
        tagged_vlans: []
        description: RTX3481 - Zone 1
        enabled: true
        poe_enabled: true
        mac_notify: false
        speed_duplex: auto
"#;

    #[test]
    fn test_parse_switch_view_from_yaml_extracts_declared_switch() {
        let view = parse_switch_view_from_yaml("IT-90297", OVERLAY_YAML).expect("should parse");
        assert_eq!(view.vlans.len(), 1);
        assert_eq!(view.vlans[0].name, "philips-ap-z1");
        assert_eq!(view.ports.len(), 1);
        assert_eq!(view.ports[0].port_id, "1");
    }

    #[test]
    fn test_parse_switch_view_from_yaml_none_for_unrelated_switch() {
        assert!(parse_switch_view_from_yaml("some-other-switch", OVERLAY_YAML).is_none());
    }

    #[test]
    fn test_parse_switch_view_from_yaml_none_on_invalid_yaml() {
        assert!(parse_switch_view_from_yaml("IT-90297", "not: [valid, yaml: structure").is_none());
    }

    #[test]
    fn test_parse_switch_views_from_yaml_handles_multiple_switches() {
        let yaml = r#"
merge_priority: 10
switches:
  - id: sw-a
    hostname: sw-a
    vlans: []
    ports: []
  - id: sw-b
    hostname: sw-b
    vlans: []
    ports: []
"#;
        let views = parse_switch_views_from_yaml(yaml);
        let ids: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["sw-a", "sw-b"]);
    }

    #[test]
    fn test_build_config_source_view_extracts_filename_and_is_main() {
        let view = build_config_source_view("/etc/switch-configurator/Shadow-Switch-1.yaml", 200, "folder");
        assert_eq!(view.filename, "Shadow-Switch-1.yaml");
        assert!(!view.is_main);
    }

    #[test]
    fn test_build_config_source_view_marks_main_source() {
        // Nix-store paths have no separate directory component for the hash
        // prefix — the whole thing is the on-disk filename.
        let view = build_config_source_view("/nix/store/1c35q4l9-switch-config.yaml", 10, "main");
        assert_eq!(view.filename, "1c35q4l9-switch-config.yaml");
        assert!(view.is_main);
    }
}

fn empty_switch(id: &str) -> SwitchView {
    SwitchView {
        id: id.to_string(),
        hostname: id.to_string(),
        model: "unknown".to_string(),
        management_ip: "".to_string(),
        vlans: vec![],
        ports: vec![],
        mirrors: vec![],
        snmp: None,
    }
}

/// Natural sort for port IDs — handles pure numbers ("1", "24"),
/// slash-separated ("1/0/1"), and prefixed ("GigabitEthernet1/0/1").
fn natural_port_sort(a: &str, b: &str) -> std::cmp::Ordering {
    // Extract trailing numeric segments for comparison
    let a_nums = extract_port_numbers(a);
    let b_nums = extract_port_numbers(b);
    a_nums.cmp(&b_nums)
}

fn extract_port_numbers(port_id: &str) -> Vec<u32> {
    port_id
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(u32::MAX))
        .collect()
}

/// Kicks off a PoE reset on the backend (returns 202) and forwards the status.
/// Live progress is streamed separately over SSE (/events) and rendered by the
/// poeReset() client script — this endpoint only starts the operation.
pub async fn poe_reset(
    State(state): State<AppState>,
    Path((id, port_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let path = format!("/switches/{}/poe-reset/{}", id, port_id);
    match state.backend.post(&path, &serde_json::json!({})).await {
        Ok((status, body)) => {
            let code = axum::http::StatusCode::from_u16(status)
                .unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
            (code, axum::Json(body)).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
