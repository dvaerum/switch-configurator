use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;

use super::AppState;
use super::switch::{SwitchView, VlanView, PortView, MirrorView, SnmpView, SnmpCommunityView, SnmpTrapReceiverView};

pub async fn start_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Fetch current desired config from backend
    let json = match state.backend.get(&format!("/switches/{}/desired-config", id)).await {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to fetch config for draft: {}", e);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    // Parse into SwitchConfig
    let config: switch_configurator::models::SwitchConfig = match serde_json::from_value(json) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to parse config: {}", e);
            return Redirect::to(&format!("/switch/{}", id)).into_response();
        }
    };

    state.drafts.create(id.clone(), config).await;
    Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response()
}

pub async fn discard_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    state.drafts.discard(&id).await;
    Redirect::to(&format!("/switch/{}", id))
}

// Edit views — show editable forms for each config section

#[derive(Template)]
#[template(path = "edit_vlans.html")]
struct EditVlansTemplate {
    switch_id: String,
    hostname: String,
    vlans: Vec<EditableVlan>,
}

#[derive(Debug, Clone)]
pub struct EditableVlan {
    pub id: u16,
    pub name: String,
    pub ip_config: String,
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
                switch_configurator::models::VlanIpConfig::None => "none".to_string(),
                switch_configurator::models::VlanIpConfig::Dhcp => "dhcp".to_string(),
                switch_configurator::models::VlanIpConfig::Static { address, netmask } =>
                    format!("{}/{}", address, netmask),
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
        None => return Html("Draft not found".to_string()).into_response(),
    };

    if let Some(vlan) = draft.edited.vlans.iter_mut().find(|v| v.id == vlan_id) {
        vlan.name = form.name;
        vlan.ip_config = match form.ip_config.as_str() {
            "dhcp" => switch_configurator::models::VlanIpConfig::Dhcp,
            "none" => switch_configurator::models::VlanIpConfig::None,
            _ => switch_configurator::models::VlanIpConfig::None,
        };
    }

    state.drafts.update(&id, draft.edited).await;

    // Return updated VLAN row via HTMX
    Redirect::to(&format!("/switch/{}/edit/vlans", id)).into_response()
}

#[derive(Template)]
#[template(path = "edit_ports.html")]
struct EditPortsTemplate {
    switch_id: String,
    hostname: String,
    ports: Vec<EditablePort>,
}

#[derive(Debug, Clone)]
pub struct EditablePort {
    pub port_id: String,
    pub mode: String,
    pub vlan: u16,
    pub allowed_vlans: String,
    pub description: String,
    pub enabled: bool,
    pub poe_enabled: bool,
    pub speed_duplex: String,
}

pub async fn edit_ports(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let ports: Vec<EditablePort> = draft.edited.ports.iter().map(|p| {
        EditablePort {
            port_id: p.port_id.clone(),
            mode: format!("{:?}", p.mode).to_lowercase(),
            vlan: p.vlan,
            allowed_vlans: p.allowed_vlans.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            description: p.description.clone().unwrap_or_default(),
            enabled: p.enabled,
            poe_enabled: p.poe_enabled,
            speed_duplex: format!("{:?}", p.speed_duplex).to_lowercase(),
        }
    }).collect();

    EditPortsTemplate {
        switch_id: id,
        hostname: draft.edited.hostname.clone().unwrap_or_default(),
        ports,
    }.into_response()
}
