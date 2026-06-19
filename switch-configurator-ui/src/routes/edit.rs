use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;
use switch_configurator::models::*;

use super::AppState;

// ============================================================================
// Draft lifecycle
// ============================================================================

#[derive(Deserialize)]
pub struct StartDraftForm {
    #[serde(default)]
    pub tab: Option<String>,
}

pub async fn start_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<StartDraftForm>,
) -> impl IntoResponse {
    let json = match state.backend.get(&format!("/switches/{}/desired-config", id)).await {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to fetch config for draft: {}", e);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    let config: SwitchConfig = match serde_json::from_value(json) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to parse config: {}", e);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    state.drafts.create(id.clone(), config).await;

    let edit_tab = match form.tab.as_deref() {
        Some("ports") => "ports",
        Some("mirrors") => "mirrors",
        Some("snmp") => "snmp",
        _ => "vlans",
    };
    Redirect::to(&format!("/switch/{}/edit/{}", id, edit_tab)).into_response()
}

pub async fn discard_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    state.drafts.discard(&id).await;
    Redirect::to(&format!("/switch/{}", id))
}

// ============================================================================
// VLAN editing
// ============================================================================

#[derive(Debug, Clone)]
pub struct EditableVlan {
    pub id: u16,
    pub name: String,
    pub ip_config: String,
}

#[derive(Template)]
#[template(path = "edit_vlans.html")]
struct EditVlansTemplate {
    switch_id: String,
    hostname: String,
    vlans: Vec<EditableVlan>,
}

pub async fn edit_vlans(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let vlans: Vec<EditableVlan> = draft.edited.vlans.iter().map(|v| {
        EditableVlan {
            id: v.id,
            name: v.name.clone(),
            ip_config: match &v.ip_config {
                VlanIpConfig::None => "none".to_string(),
                VlanIpConfig::Dhcp => "dhcp".to_string(),
                VlanIpConfig::Static { address, netmask } => format!("{}/{}", address, netmask),
            },
        }
    }).collect();

    EditVlansTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        vlans,
    }.into_response()
}

#[derive(Deserialize)]
pub struct UpdateVlanForm {
    pub name: String,
    pub ip_config: String,
}

pub async fn update_vlan(
    State(state): State<AppState>,
    Path((id, vlan_id)): Path<(String, u16)>,
    Form(form): Form<UpdateVlanForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response(),
    };

    if let Some(vlan) = draft.edited.vlans.iter_mut().find(|v| v.id == vlan_id) {
        vlan.name = form.name;
        vlan.ip_config = parse_ip_config(&form.ip_config);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response()
}

#[derive(Deserialize)]
pub struct AddVlanForm {
    pub id: u16,
    pub name: String,
    pub ip_config: String,
}

pub async fn add_vlan(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<AddVlanForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response(),
    };

    if !draft.edited.vlans.iter().any(|v| v.id == form.id) {
        draft.edited.vlans.push(Vlan {
            id: form.id,
            name: form.name,
            description: None,
            ip_config: parse_ip_config(&form.ip_config),
        });
        draft.edited.vlans.sort_by_key(|v| v.id);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response()
}

pub async fn remove_vlan(
    State(state): State<AppState>,
    Path((id, vlan_id)): Path<(String, u16)>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response(),
    };

    draft.edited.vlans.retain(|v| v.id != vlan_id);
    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response()
}

// ============================================================================
// Port editing
// ============================================================================

#[derive(Debug, Clone)]
pub struct EditablePort {
    pub port_id: String,
    pub vlan: u16,
    pub tagged_vlans: String,
    pub description: String,
    pub enabled: bool,
    pub poe_enabled: bool,
    pub speed_duplex: String,
}

#[derive(Template)]
#[template(path = "edit_ports.html")]
struct EditPortsTemplate {
    switch_id: String,
    hostname: String,
    ports: Vec<EditablePort>,
}

pub async fn edit_ports(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let mut ports: Vec<EditablePort> = draft.edited.ports.iter().map(|p| port_to_editable(p)).collect();
    ports.sort_by(|a, b| natural_sort(&a.port_id, &b.port_id));

    EditPortsTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        ports,
    }.into_response()
}

#[derive(Deserialize)]
pub struct PortForm {
    pub port_id: String,
    pub vlan: u16,
    #[serde(default)]
    pub tagged_vlans: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: Option<String>,
    #[serde(default)]
    pub poe_enabled: Option<String>,
    #[serde(default)]
    pub speed_duplex: String,
}

pub async fn add_port(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<PortForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response(),
    };

    if !draft.edited.ports.iter().any(|p| p.port_id == form.port_id) {
        draft.edited.ports.push(form_to_port(&form));
        draft.edited.ports.sort_by(|a, b| natural_sort(&a.port_id, &b.port_id));
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response()
}

/// Bulk-save every port row from the single "Save All Ports" form. Replaces the
/// draft's port list in one shot so changes to multiple ports (e.g. toggling PoE
/// on several ports) persist together.
pub async fn update_ports(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response(),
    };

    draft.edited.ports = parse_ports_bulk(pairs, &draft.edited.ports);

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response()
}

pub async fn remove_port(
    State(state): State<AppState>,
    Path((id, port_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response(),
    };

    draft.edited.ports.retain(|p| p.port_id != port_id);
    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response()
}

// ============================================================================
// Mirror editing
// ============================================================================

#[derive(Debug, Clone)]
pub struct EditableMirror {
    pub session_id: String,
    pub source_ports: String,
    pub destination_port: String,
    pub direction: String,
}

#[derive(Template)]
#[template(path = "edit_mirrors.html")]
struct EditMirrorsTemplate {
    switch_id: String,
    hostname: String,
    mirrors: Vec<EditableMirror>,
}

pub async fn edit_mirrors(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let mirrors: Vec<EditableMirror> = draft.edited.port_mirrors.iter().map(|m| {
        EditableMirror {
            session_id: m.session_id.clone(),
            source_ports: m.source_ports.join(", "),
            destination_port: m.destination_port.clone(),
            direction: format!("{:?}", m.direction).to_lowercase(),
        }
    }).collect();

    EditMirrorsTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        mirrors,
    }.into_response()
}

#[derive(Deserialize)]
pub struct MirrorForm {
    pub session_id: String,
    pub source_ports: String,
    pub destination_port: String,
    pub direction: String,
}

pub async fn add_mirror(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<MirrorForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response(),
    };

    if !draft.edited.port_mirrors.iter().any(|m| m.session_id == form.session_id) {
        draft.edited.port_mirrors.push(form_to_mirror(&form));
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response()
}

pub async fn update_mirror(
    State(state): State<AppState>,
    Path((id, session_id)): Path<(String, String)>,
    Form(form): Form<MirrorForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response(),
    };

    if let Some(mirror) = draft.edited.port_mirrors.iter_mut().find(|m| m.session_id == session_id) {
        *mirror = form_to_mirror(&form);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response()
}

pub async fn remove_mirror(
    State(state): State<AppState>,
    Path((id, session_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response(),
    };

    draft.edited.port_mirrors.retain(|m| m.session_id != session_id);
    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/mirrors", id)).into_response()
}

// ============================================================================
// SNMP editing
// ============================================================================

#[derive(Template)]
#[template(path = "edit_snmp.html")]
struct EditSnmpTemplate {
    switch_id: String,
    hostname: String,
    communities: Vec<EditableSnmpCommunity>,
    trap_receivers: Vec<EditableSnmpTrapReceiver>,
    enabled_traps: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EditableSnmpCommunity {
    pub name: String,
    pub access: String,
}

#[derive(Debug, Clone)]
pub struct EditableSnmpTrapReceiver {
    pub host: String,
    pub community: String,
    pub version: String,
}

pub async fn edit_snmp(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let snmp = draft.edited.snmp.as_ref();

    EditSnmpTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        communities: snmp.map(|s| s.communities.iter().map(|c| EditableSnmpCommunity {
            name: c.name.clone(),
            access: format!("{:?}", c.access).to_lowercase(),
        }).collect()).unwrap_or_default(),
        trap_receivers: snmp.map(|s| s.trap_receivers.iter().map(|r| EditableSnmpTrapReceiver {
            host: r.host.clone(),
            community: r.community.clone(),
            version: r.version.clone().unwrap_or_default(),
        }).collect()).unwrap_or_default(),
        enabled_traps: snmp.map(|s| s.enabled_traps.iter().map(|t| format!("{:?}", t).to_lowercase()).collect()).unwrap_or_default(),
    }.into_response()
}

#[derive(Deserialize)]
pub struct SnmpCommunityForm {
    pub name: String,
    pub access: String,
}

pub async fn add_snmp_community(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<SnmpCommunityForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response(),
    };

    let snmp = draft.edited.snmp.get_or_insert_with(|| SnmpConfig {
        communities: vec![],
        trap_receivers: vec![],
        enabled_traps: vec![],
    });

    if !snmp.communities.iter().any(|c| c.name == form.name) {
        snmp.communities.push(SnmpCommunity {
            name: form.name,
            access: parse_snmp_access(&form.access),
        });
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response()
}

pub async fn remove_snmp_community(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response(),
    };

    if let Some(snmp) = &mut draft.edited.snmp {
        snmp.communities.retain(|c| c.name != name);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response()
}

#[derive(Deserialize)]
pub struct SnmpTrapReceiverForm {
    pub host: String,
    pub community: String,
    pub version: String,
}

pub async fn add_snmp_trap_receiver(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<SnmpTrapReceiverForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response(),
    };

    let snmp = draft.edited.snmp.get_or_insert_with(|| SnmpConfig {
        communities: vec![],
        trap_receivers: vec![],
        enabled_traps: vec![],
    });

    snmp.trap_receivers.push(SnmpTrapReceiver {
        host: form.host,
        community: form.community,
        version: Some(form.version),
    });

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response()
}

pub async fn remove_snmp_trap_receiver(
    State(state): State<AppState>,
    Path((id, host)): Path<(String, String)>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response(),
    };

    if let Some(snmp) = &mut draft.edited.snmp {
        snmp.trap_receivers.retain(|r| r.host != host);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response()
}

#[derive(Deserialize)]
pub struct SnmpTrapsForm {
    #[serde(default)]
    pub mac_notify: Option<String>,
    #[serde(default)]
    pub link_change: Option<String>,
}

pub async fn update_snmp_traps(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<SnmpTrapsForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response(),
    };

    let snmp = draft.edited.snmp.get_or_insert_with(|| SnmpConfig {
        communities: vec![],
        trap_receivers: vec![],
        enabled_traps: vec![],
    });

    snmp.enabled_traps.clear();
    if form.mac_notify.is_some() {
        snmp.enabled_traps.push(TrapType::MacNotify);
    }
    if form.link_change.is_some() {
        snmp.enabled_traps.push(TrapType::LinkChange);
    }

    state.drafts.update(&id, draft.edited).await;
    Redirect::to(&format!("/switch/{}/edit/snmp", id)).into_response()
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_ip_config(s: &str) -> VlanIpConfig {
    match s {
        "dhcp" => VlanIpConfig::Dhcp,
        "none" | "" => VlanIpConfig::None,
        _ => VlanIpConfig::None,
    }
}

fn parse_snmp_access(s: &str) -> SnmpAccess {
    match s {
        "manager" => SnmpAccess::Manager,
        _ => SnmpAccess::Operator,
    }
}

fn parse_speed_duplex(s: &str) -> SpeedDuplex {
    match s {
        "10-half" => SpeedDuplex::TenHalf,
        "10-full" => SpeedDuplex::TenFull,
        "100-half" => SpeedDuplex::HundredHalf,
        "100-full" => SpeedDuplex::HundredFull,
        "1000-full" => SpeedDuplex::ThousandFull,
        "10g-full" => SpeedDuplex::TenGFull,
        _ => SpeedDuplex::Auto,
    }
}

fn parse_port_mode(s: &str) -> PortMode {
    match s {
        "trunk" => PortMode::Trunk,
        _ => PortMode::Access,
    }
}

fn parse_tagged_vlans(s: &str) -> Vec<u16> {
    s.split(',')
        .filter_map(|v| v.trim().parse::<u16>().ok())
        .collect()
}

fn form_to_port(form: &PortForm) -> Port {
    let tagged = parse_tagged_vlans(&form.tagged_vlans);
    Port {
        port_id: form.port_id.clone(),
        mode: if tagged.is_empty() { PortMode::Access } else { PortMode::Trunk },
        vlan: form.vlan,
        tagged_vlans: tagged,
        description: if form.description.is_empty() { None } else { Some(form.description.clone()) },
        enabled: form.enabled.is_some(),
        poe_enabled: form.poe_enabled.is_some(),
        mac_notify: false,
        speed_duplex: parse_speed_duplex(&form.speed_duplex),
    }
}

/// Parse a flat list of form pairs (from the single "Save All Ports" form) into
/// a full set of ports. Field names are indexed per row, e.g. `port_id.0`,
/// `vlan.0`, `poe_enabled.0`. Unchecked checkboxes are absent, so presence of the
/// key means the box was ticked. `mac_notify` isn't an editable field, so it is
/// preserved from the matching existing port by id.
fn parse_ports_bulk(pairs: Vec<(String, String)>, existing: &[Port]) -> Vec<Port> {
    use std::collections::{BTreeMap, HashMap};

    let mut rows: BTreeMap<usize, HashMap<String, String>> = BTreeMap::new();
    for (key, val) in pairs {
        if let Some((field, idx)) = key.rsplit_once('.') {
            if let Ok(i) = idx.parse::<usize>() {
                rows.entry(i).or_default().insert(field.to_string(), val);
            }
        }
    }

    let mut ports = Vec::new();
    for fields in rows.into_values() {
        let port_id = match fields.get("port_id") {
            Some(p) if !p.trim().is_empty() => p.clone(),
            _ => continue,
        };
        let tagged = parse_tagged_vlans(fields.get("tagged_vlans").map(String::as_str).unwrap_or(""));
        let vlan = fields.get("vlan").and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
        let description = fields.get("description").cloned().unwrap_or_default();
        let mac_notify = existing
            .iter()
            .find(|p| p.port_id == port_id)
            .map(|p| p.mac_notify)
            .unwrap_or(false);

        ports.push(Port {
            port_id,
            mode: if tagged.is_empty() { PortMode::Access } else { PortMode::Trunk },
            vlan,
            tagged_vlans: tagged,
            description: if description.is_empty() { None } else { Some(description) },
            enabled: fields.contains_key("enabled"),
            poe_enabled: fields.contains_key("poe_enabled"),
            mac_notify,
            speed_duplex: parse_speed_duplex(fields.get("speed_duplex").map(String::as_str).unwrap_or("auto")),
        });
    }

    ports.sort_by(|a, b| natural_sort(&a.port_id, &b.port_id));
    ports
}

fn form_to_mirror(form: &MirrorForm) -> PortMirror {
    PortMirror {
        session_id: form.session_id.clone(),
        source_ports: form.source_ports.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        destination_port: form.destination_port.clone(),
        direction: match form.direction.as_str() {
            "rx" => MirrorDirection::Rx,
            "tx" => MirrorDirection::Tx,
            _ => MirrorDirection::Both,
        },
    }
}

fn port_to_editable(p: &Port) -> EditablePort {
    EditablePort {
        port_id: p.port_id.clone(),
        vlan: p.vlan,
        tagged_vlans: p.tagged_vlans.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
        description: p.description.clone().unwrap_or_default(),
        enabled: p.enabled,
        poe_enabled: p.poe_enabled,
        speed_duplex: format!("{:?}", p.speed_duplex).to_lowercase(),
    }
}

fn natural_sort(a: &str, b: &str) -> std::cmp::Ordering {
    let a_nums: Vec<u32> = a
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(u32::MAX))
        .collect();
    let b_nums: Vec<u32> = b
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(u32::MAX))
        .collect();
    a_nums.cmp(&b_nums)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn test_parse_ports_bulk_multiple_poe_enabled() {
        // Both rows have poe_enabled checkbox present (checked) -> both true.
        // This is the regression: previously only one row's save persisted.
        let pairs = vec![
            pair("port_id.0", "1"),
            pair("vlan.0", "10"),
            pair("tagged_vlans.0", ""),
            pair("description.0", "port one"),
            pair("enabled.0", "on"),
            pair("poe_enabled.0", "on"),
            pair("speed_duplex.0", "auto"),
            pair("port_id.1", "2"),
            pair("vlan.1", "10"),
            pair("tagged_vlans.1", ""),
            pair("description.1", "port two"),
            pair("enabled.1", "on"),
            pair("poe_enabled.1", "on"),
            pair("speed_duplex.1", "auto"),
        ];
        let ports = parse_ports_bulk(pairs, &[]);
        assert_eq!(ports.len(), 2);
        assert!(ports.iter().find(|p| p.port_id == "1").unwrap().poe_enabled);
        assert!(ports.iter().find(|p| p.port_id == "2").unwrap().poe_enabled);
    }

    #[test]
    fn test_parse_ports_bulk_unchecked_poe_is_false() {
        // Unchecked checkboxes are simply absent from the form submission.
        // Row 0 keeps poe on, row 1 has no poe_enabled key -> false.
        let pairs = vec![
            pair("port_id.0", "1"),
            pair("vlan.0", "10"),
            pair("enabled.0", "on"),
            pair("poe_enabled.0", "on"),
            pair("speed_duplex.0", "auto"),
            pair("port_id.1", "2"),
            pair("vlan.1", "10"),
            pair("enabled.1", "on"),
            pair("speed_duplex.1", "auto"),
        ];
        let ports = parse_ports_bulk(pairs, &[]);
        assert!(ports.iter().find(|p| p.port_id == "1").unwrap().poe_enabled);
        assert!(!ports.iter().find(|p| p.port_id == "2").unwrap().poe_enabled);
    }

    #[test]
    fn test_parse_ports_bulk_preserves_mac_notify() {
        // mac_notify isn't an editable field; bulk save must not silently drop it.
        let existing = vec![Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            tagged_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: true,
            speed_duplex: SpeedDuplex::Auto,
        }];
        let pairs = vec![
            pair("port_id.0", "1"),
            pair("vlan.0", "10"),
            pair("enabled.0", "on"),
            pair("speed_duplex.0", "auto"),
        ];
        let ports = parse_ports_bulk(pairs, &existing);
        assert!(ports[0].mac_notify, "mac_notify should be preserved");
    }

    #[test]
    fn test_parse_ports_bulk_sorts_and_parses_fields() {
        let pairs = vec![
            pair("port_id.0", "10"),
            pair("vlan.0", "5"),
            pair("tagged_vlans.0", "20,30"),
            pair("enabled.0", "on"),
            pair("speed_duplex.0", "1000-full"),
            pair("port_id.1", "2"),
            pair("vlan.1", "1"),
            pair("speed_duplex.1", "auto"),
        ];
        let ports = parse_ports_bulk(pairs, &[]);
        // natural sort: "2" before "10"
        assert_eq!(ports[0].port_id, "2");
        assert_eq!(ports[1].port_id, "10");
        let p10 = &ports[1];
        assert_eq!(p10.vlan, 5);
        assert_eq!(p10.tagged_vlans, vec![20, 30]);
        assert_eq!(p10.mode, PortMode::Trunk);
        assert_eq!(p10.speed_duplex, SpeedDuplex::ThousandFull);
        // port 2 had no enabled checkbox -> disabled
        assert!(!ports[0].enabled);
    }
}
