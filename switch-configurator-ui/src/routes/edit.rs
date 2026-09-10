use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
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

/// Pull the merged config and per-row source attribution out of a
/// `/switches/{id}/merge-preview` response. Used both for an ordinary
/// healthy switch (empty source maps — nothing to attribute) and a switch
/// that failed validation (both colliding rows present, each tagged with
/// the file it came from) — one endpoint, one parse, so the draft flow
/// doesn't need to know in advance which kind of switch it's editing.
fn parse_merge_preview(
    json: &serde_json::Value,
) -> Option<(SwitchConfig, std::collections::HashMap<u16, String>, std::collections::HashMap<String, String>)> {
    let config: SwitchConfig = serde_json::from_value(json["config"].clone()).ok()?;

    let vlan_sources = json["vlan_sources"].as_object()
        .map(|m| m.iter().filter_map(|(k, v)| {
            Some((k.parse::<u16>().ok()?, v.as_str()?.to_string()))
        }).collect())
        .unwrap_or_default();

    let port_sources = json["port_sources"].as_object()
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();

    Some((config, vlan_sources, port_sources))
}

pub async fn start_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<StartDraftForm>,
) -> impl IntoResponse {
    let json = match state.backend.get(&format!("/switches/{}/merge-preview", id)).await {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to fetch config for draft: {}", e);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    let (config, vlan_sources, port_sources) = match parse_merge_preview(&json) {
        Some(parsed) => parsed,
        None => {
            tracing::error!("Failed to parse merge preview for {}", id);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    state.drafts.create_with_sources(id.clone(), config, vlan_sources, port_sources).await;

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
    /// `true` for a row that came from the main config — never editable
    /// here, same boundary the overlay View/Delete pages already enforce.
    pub read_only: bool,
    /// `Some("from overlay.yaml")` for a row attributed to a specific
    /// overlay file; `None` for an ordinary healthy-switch draft (nothing
    /// to attribute) or for a main-config row (labeled via `read_only`
    /// instead, to avoid saying the same thing two different ways).
    pub source_label: Option<String>,
    /// Set when another row (a different id) shares this row's name — the
    /// exact condition the merge-time ambiguous-name check rejects. Points
    /// at the other row so both sides of a collision are visible together,
    /// not just in the banner text above the table.
    pub collision_note: Option<String>,
}

#[derive(Template)]
#[template(path = "edit_vlans.html")]
struct EditVlansTemplate {
    switch_id: String,
    hostname: String,
    vlans: Vec<EditableVlan>,
}

/// Build the editable VLAN rows for one switch, given its merged vlans, the
/// per-id source-file attribution (empty for an ordinary healthy switch),
/// and the main config's own path (to tell "from the main config" apart
/// from "from an overlay").
fn build_editable_vlans(
    vlans: &[Vlan],
    vlan_sources: &std::collections::HashMap<u16, String>,
    main_config_path: &str,
) -> Vec<EditableVlan> {
    vlans.iter().map(|v| {
        let source = vlan_sources.get(&v.id);
        let read_only = is_main_config_source(source, main_config_path);
        let source_label = source.filter(|_| !read_only).and_then(|path| {
            std::path::Path::new(path).file_name().map(|n| format!("from {}", n.to_string_lossy()))
        });

        let collision = vlans.iter()
            .filter(|other| other.id != v.id && other.name == v.name)
            .next();
        let collision_note = collision.map(|other| {
            let other_source = vlan_sources.get(&other.id);
            let other_is_main = is_main_config_source(other_source, main_config_path);
            let other_label = if other_is_main {
                "main config".to_string()
            } else {
                other_source
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "another source".to_string())
            };
            format!("also used by id {} ({})", other.id, other_label)
        });

        EditableVlan {
            id: v.id,
            name: v.name.clone(),
            ip_config: match &v.ip_config {
                VlanIpConfig::None => "none".to_string(),
                VlanIpConfig::Dhcp => "dhcp".to_string(),
                VlanIpConfig::Static { address, netmask } => format!("{}/{}", address, netmask),
            },
            read_only,
            source_label,
            collision_note,
        }
    }).collect()
}

pub async fn edit_vlans(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let main_config_path = main_config_path(&state).await;
    let vlans = build_editable_vlans(&draft.edited.vlans, &draft.vlan_sources, &main_config_path);

    EditVlansTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        vlans,
    }.into_response()
}

/// The main config's own path, so a draft row attributed to it can be told
/// apart from a row attributed to a genuine (editable) overlay. Empty when
/// unknown — never equals a real source path, so every row reads as an
/// editable overlay row rather than being mistaken for read-only.
pub(crate) async fn main_config_path(state: &AppState) -> String {
    state.backend.get("/api/status").await.ok()
        .and_then(|s| s["configuration"]["config_file"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Whether a draft row (by its recorded source path, if any) came from the
/// main config — and so must never be mutated from here, no matter what a
/// form submission claims. `None` (no attribution recorded — an ordinary
/// healthy-switch draft) is never read-only.
pub(crate) fn is_main_config_source(source: Option<&String>, main_config_path: &str) -> bool {
    source.map(|s| s == main_config_path).unwrap_or(false)
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

    // Never mutate a row attributed to the main config, regardless of what
    // the form claims — the same boundary View/Delete already enforce for
    // overlay files, checked server-side rather than trusted from a
    // (spoofable) disabled form control.
    let main_config = main_config_path(&state).await;
    if is_main_config_source(draft.vlan_sources.get(&vlan_id), &main_config) {
        return Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response();
    }

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

    let main_config = main_config_path(&state).await;
    if is_main_config_source(draft.vlan_sources.get(&vlan_id), &main_config) {
        return Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response();
    }

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
    pub tagged_vlan_choices: Vec<TaggedVlanChoice>,
    pub description: String,
    pub enabled: bool,
    pub poe_enabled: bool,
    pub speed_duplex: String,
}

/// A VLAN as offered to the untagged VLAN picker — named by id, so the UI
/// selects (and, on save, persists) VLANs by name rather than a hand-typed
/// numeric id.
#[derive(Debug, Clone)]
pub struct VlanOption {
    pub id: u16,
    pub name: String,
}

/// A VLAN as offered to one port's tagged-VLAN multi-select, with whether
/// it's currently selected precomputed — Askama 0.12 has no `contains`/`in`
/// expression, so membership is resolved here rather than in the template.
#[derive(Debug, Clone)]
pub struct TaggedVlanChoice {
    pub id: u16,
    pub name: String,
    pub selected: bool,
}

fn vlan_options(vlans: &[Vlan]) -> Vec<VlanOption> {
    vlans.iter().map(|v| VlanOption { id: v.id, name: v.name.clone() }).collect()
}

fn tagged_vlan_choices(vlans: &[VlanOption], tagged: &[u16]) -> Vec<TaggedVlanChoice> {
    vlans.iter().map(|v| TaggedVlanChoice {
        id: v.id,
        name: v.name.clone(),
        selected: tagged.contains(&v.id),
    }).collect()
}

#[derive(Template)]
#[template(path = "edit_ports.html")]
struct EditPortsTemplate {
    switch_id: String,
    hostname: String,
    ports: Vec<EditablePort>,
    vlans: Vec<VlanOption>,
}

pub async fn edit_ports(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let vlans = vlan_options(&draft.edited.vlans);
    let mut ports: Vec<EditablePort> = draft.edited.ports.iter().map(|p| port_to_editable(p, &vlans)).collect();
    ports.sort_by(|a, b| natural_sort(&a.port_id, &b.port_id));

    EditPortsTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        ports,
        vlans,
    }.into_response()
}

#[derive(Deserialize)]
pub struct PortForm {
    pub port_id: String,
    pub vlan: u16,
    #[serde(default)]
    pub tagged_vlans: Vec<u16>,
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

fn form_to_port(form: &PortForm) -> Port {
    let tagged = form.tagged_vlans.clone();
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
        vlan_name: None,
        tagged_vlan_refs: vec![],
    }
}

/// Parse a flat list of form pairs (from the single "Save All Ports" form) into
/// a full set of ports. Field names are indexed per row, e.g. `port_id.0`,
/// `vlan.0`, `poe_enabled.0`. Unchecked checkboxes are absent, so presence of the
/// key means the box was ticked. `mac_notify` isn't an editable field, so it is
/// preserved from the matching existing port by id.
fn parse_ports_bulk(pairs: Vec<(String, String)>, existing: &[Port]) -> Vec<Port> {
    use std::collections::{BTreeMap, HashMap};

    // A multi-select submits its field key once per selected option (e.g. two
    // `tagged_vlans.0` pairs for two selected VLANs), so each row collects a
    // Vec of values per field rather than overwriting on repeat.
    let mut rows: BTreeMap<usize, HashMap<String, Vec<String>>> = BTreeMap::new();
    for (key, val) in pairs {
        if let Some((field, idx)) = key.rsplit_once('.') {
            if let Ok(i) = idx.parse::<usize>() {
                rows.entry(i).or_default().entry(field.to_string()).or_default().push(val);
            }
        }
    }

    let mut ports = Vec::new();
    for fields in rows.into_values() {
        let port_id = match fields.get("port_id").and_then(|v| v.last()) {
            Some(p) if !p.trim().is_empty() => p.clone(),
            _ => continue,
        };
        let tagged: Vec<u16> = fields.get("tagged_vlans")
            .map(|vs| vs.iter().filter_map(|v| v.parse::<u16>().ok()).collect())
            .unwrap_or_default();
        let vlan = fields.get("vlan").and_then(|v| v.last()).and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
        let description = fields.get("description").and_then(|v| v.last()).cloned().unwrap_or_default();
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
            speed_duplex: parse_speed_duplex(fields.get("speed_duplex").and_then(|v| v.last()).map(String::as_str).unwrap_or("auto")),
            vlan_name: None,
            tagged_vlan_refs: vec![],
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

fn port_to_editable(p: &Port, vlans: &[VlanOption]) -> EditablePort {
    EditablePort {
        port_id: p.port_id.clone(),
        vlan: p.vlan,
        tagged_vlan_choices: tagged_vlan_choices(vlans, &p.tagged_vlans),
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

    fn vlan(id: u16, name: &str) -> Vlan {
        Vlan { id, name: name.to_string(), description: None, ip_config: VlanIpConfig::None }
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
            vlan_name: None,
            tagged_vlan_refs: vec![],
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
            // A multi-select submits the same key once per selected option,
            // not a comma-joined string.
            pair("tagged_vlans.0", "20"),
            pair("tagged_vlans.0", "30"),
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

    #[test]
    fn test_parse_ports_bulk_no_tagged_vlans_selected() {
        // Nothing selected in the multi-select means the key is entirely
        // absent from the submission (same as an unchecked checkbox).
        let pairs = vec![
            pair("port_id.0", "1"),
            pair("vlan.0", "10"),
            pair("enabled.0", "on"),
            pair("speed_duplex.0", "auto"),
        ];
        let ports = parse_ports_bulk(pairs, &[]);
        assert!(ports[0].tagged_vlans.is_empty());
        assert_eq!(ports[0].mode, PortMode::Access);
    }

    #[test]
    fn test_form_to_port_uses_vec_tagged_vlans_directly() {
        // PortForm.tagged_vlans is already Vec<u16> (bound from a multi-select),
        // so form_to_port shouldn't need any string parsing for it.
        let form = PortForm {
            port_id: "5".to_string(),
            vlan: 10,
            tagged_vlans: vec![20, 30],
            description: "test".to_string(),
            enabled: Some("on".to_string()),
            poe_enabled: None,
            speed_duplex: "auto".to_string(),
        };
        let port = form_to_port(&form);
        assert_eq!(port.vlan, 10);
        assert_eq!(port.tagged_vlans, vec![20, 30]);
        assert_eq!(port.mode, PortMode::Trunk);
    }

    #[test]
    fn test_parse_merge_preview_extracts_config_and_sources() {
        let json = serde_json::json!({
            "valid": false,
            "config": {
                "id": "broken-switch",
                "hostname": "broken-switch",
                "model": "Aruba2930F",
                "management_ip": "192.168.1.2",
                "credentials": null,
                "vlans": [
                    {"id": 10, "name": "users", "description": null, "ip_config": "none"},
                    {"id": 99, "name": "users", "description": null, "ip_config": "none"}
                ],
                "ports": [],
                "port_mirrors": [],
                "snmp": null,
                "management_vlan": null,
                "validation": null,
                "vendor_specific": {},
                "settings": {"ssh_timeout_secs": 30, "max_retries": 3, "enforce_port_config": true}
            },
            "vlan_sources": {"10": "/etc/main.yaml", "99": "/etc/switch-configurator/overlay.yaml"},
            "port_sources": {}
        });

        let (config, vlan_sources, port_sources) = parse_merge_preview(&json).expect("should parse");
        assert_eq!(config.id, "broken-switch");
        assert_eq!(config.vlans.len(), 2);
        assert_eq!(vlan_sources.get(&10).map(String::as_str), Some("/etc/main.yaml"));
        assert_eq!(vlan_sources.get(&99).map(String::as_str), Some("/etc/switch-configurator/overlay.yaml"));
        assert!(port_sources.is_empty());
    }

    #[test]
    fn test_build_editable_vlans_marks_main_config_row_read_only() {
        let vlans = vec![vlan(10, "users"), vlan(99, "users")];
        let mut sources = std::collections::HashMap::new();
        sources.insert(10, "/etc/main.yaml".to_string());
        sources.insert(99, "/etc/switch-configurator/overlay.yaml".to_string());

        let rows = build_editable_vlans(&vlans, &sources, "/etc/main.yaml");

        let main_row = rows.iter().find(|r| r.id == 10).unwrap();
        assert!(main_row.read_only, "row sourced from the main config should be read-only");
        assert_eq!(main_row.source_label, None);

        let overlay_row = rows.iter().find(|r| r.id == 99).unwrap();
        assert!(!overlay_row.read_only, "row sourced from an overlay should stay editable");
        assert_eq!(overlay_row.source_label.as_deref(), Some("from overlay.yaml"));
    }

    #[test]
    fn test_build_editable_vlans_notes_name_collision_both_ways() {
        let vlans = vec![vlan(10, "users"), vlan(99, "users")];
        let mut sources = std::collections::HashMap::new();
        sources.insert(10, "/etc/main.yaml".to_string());
        sources.insert(99, "/etc/switch-configurator/overlay.yaml".to_string());

        let rows = build_editable_vlans(&vlans, &sources, "/etc/main.yaml");

        let main_row = rows.iter().find(|r| r.id == 10).unwrap();
        assert_eq!(main_row.collision_note.as_deref(), Some("also used by id 99 (overlay.yaml)"));

        let overlay_row = rows.iter().find(|r| r.id == 99).unwrap();
        assert_eq!(overlay_row.collision_note.as_deref(), Some("also used by id 10 (main config)"));
    }

    #[test]
    fn test_build_editable_vlans_no_collision_no_note() {
        let vlans = vec![vlan(10, "users"), vlan(20, "servers")];
        let rows = build_editable_vlans(&vlans, &std::collections::HashMap::new(), "/etc/main.yaml");
        assert!(rows.iter().all(|r| r.collision_note.is_none()));
        assert!(rows.iter().all(|r| !r.read_only), "healthy-switch draft rows should all stay editable");
    }

    #[test]
    fn test_vlan_options_from_switch_vlans() {
        let vlans = vec![
            Vlan { id: 10, name: "users".to_string(), description: None, ip_config: VlanIpConfig::None },
            Vlan { id: 20, name: "servers".to_string(), description: None, ip_config: VlanIpConfig::None },
        ];
        let options = vlan_options(&vlans);
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, 10);
        assert_eq!(options[0].name, "users");
    }
}
