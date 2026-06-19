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

pub async fn update_port(
    State(state): State<AppState>,
    Path((id, port_id)): Path<(String, String)>,
    Form(form): Form<PortForm>,
) -> impl IntoResponse {
    let mut draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}/edit/ports", id)).into_response(),
    };

    if let Some(port) = draft.edited.ports.iter_mut().find(|p| p.port_id == port_id) {
        *port = form_to_port(&form);
    }

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
