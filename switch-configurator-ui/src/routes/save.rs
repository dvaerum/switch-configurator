use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;
use switch_configurator::models::{Port, Vlan};

use super::edit::{is_main_config_source, main_config_path};
use super::AppState;

/// Which overlay filename "Save" should default to. For a draft seeded from
/// a broken switch (`vlan_sources`/`port_sources` non-empty), that's the
/// disputed overlay's own file — saving under a *different* name wouldn't
/// actually fix anything: the old file would still exist, still colliding,
/// and (same priority, alphabetically first) would still win the merge over
/// a same-priority new file. For an ordinary healthy-switch draft (no
/// attribution recorded), falls back to the switch id as before.
fn default_overlay_filename(
    vlan_sources: &std::collections::HashMap<u16, String>,
    port_sources: &std::collections::HashMap<String, String>,
    main_config_path: &str,
    switch_id: &str,
) -> String {
    vlan_sources.values().chain(port_sources.values())
        .find(|path| path.as_str() != main_config_path)
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.yaml", switch_id))
}

/// Build the JSON body sent to `POST /switches/{id}/save-overlay`.
///
/// Identity fields (`model`, `management_ip`, `credentials`) are always
/// included, unconditionally — unlike VLANs/ports, they have no per-field
/// source tracking, but including them is safe either way: when the main
/// config also defines them, main always wins the merge regardless (it's
/// processed first), so restating them here is redundant but harmless; when
/// the overlay is the *only* source for them (a switch whose identity lives
/// entirely in a folder config, not the main config — a supported shape),
/// omitting them would silently strip required fields from the file on
/// every save. Found via live verification: saving a fix to
/// `demo-broken-switch.yaml` (identity only in the overlay) dropped its
/// `model`/`management_ip`/`credentials` because this payload never sent
/// them, turning a fixed VLAN collision into a new "missing required
/// fields" failure.
fn build_save_body(
    id: &str,
    form: &SaveForm,
    edited: &switch_configurator::models::SwitchConfig,
    vlans: Vec<Vlan>,
    ports: Vec<Port>,
) -> serde_json::Value {
    serde_json::json!({
        "filename": form.filename,
        "merge_priority": form.priority,
        "config": {
            "switches": [{
                "id": id,
                "hostname": edited.hostname,
                "model": edited.model,
                "management_ip": edited.management_ip,
                "credentials": edited.credentials,
                "vlans": vlans,
                "ports": ports,
                "port_mirrors": edited.port_mirrors,
                "snmp": edited.snmp,
            }]
        }
    })
}

/// Rows attributed to the main config must never be written into an
/// overlay — that's exactly the duplication that made two files drift out
/// of sync in the first place. A row with no recorded source (an ordinary
/// healthy-switch draft, where `vlan_sources`/`port_sources` are empty)
/// always passes through unfiltered.
fn exclude_main_config_rows(
    vlans: &[Vlan],
    vlan_sources: &std::collections::HashMap<u16, String>,
    ports: &[Port],
    port_sources: &std::collections::HashMap<String, String>,
    main_config_path: &str,
) -> (Vec<Vlan>, Vec<Port>) {
    let vlans = vlans.iter()
        .filter(|v| !is_main_config_source(vlan_sources.get(&v.id), main_config_path))
        .cloned()
        .collect();
    let ports = ports.iter()
        .filter(|p| !is_main_config_source(port_sources.get(&p.port_id), main_config_path))
        .cloned()
        .collect();
    (vlans, ports)
}

#[derive(Template)]
#[template(path = "partials/save_dialog.html")]
struct SaveDialogTemplate {
    switch_id: String,
    default_filename: String,
    default_priority: u16,
    error: Option<String>,
}

pub async fn save_dialog(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let main_config = main_config_path(&state).await;
    let default_filename = default_overlay_filename(&draft.vlan_sources, &draft.port_sources, &main_config, &id);

    SaveDialogTemplate {
        switch_id: id.clone(),
        default_filename,
        default_priority: 200,
        error: None,
    }.into_response()
}

#[derive(Deserialize)]
pub struct SaveForm {
    pub filename: String,
    pub priority: u16,
}

pub async fn save_overlay(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<SaveForm>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let main_config = main_config_path(&state).await;
    let (vlans, ports) = exclude_main_config_rows(
        &draft.edited.vlans, &draft.vlan_sources,
        &draft.edited.ports, &draft.port_sources,
        &main_config,
    );

    let save_body = build_save_body(&id, &form, &draft.edited, vlans, ports);

    match state.backend.post(&format!("/switches/{}/save-overlay", id), &save_body).await {
        Ok((status, resp)) => {
            if status >= 200 && status < 300 {
                tracing::info!("Saved overlay for {}: {:?}", id, resp);
                state.drafts.discard(&id).await;
                Redirect::to(&format!("/switch/{}", id)).into_response()
            } else {
                let error_msg = resp["error"].as_str()
                    .unwrap_or("Unknown error from backend")
                    .to_string();
                tracing::error!("Failed to save overlay: {} {}", status, error_msg);
                SaveDialogTemplate {
                    switch_id: id,
                    default_filename: form.filename,
                    default_priority: form.priority,
                    error: Some(error_msg),
                }.into_response()
            }
        }
        Err(e) => {
            tracing::error!("Failed to save overlay: {}", e);
            SaveDialogTemplate {
                switch_id: id,
                default_filename: form.filename,
                default_priority: form.priority,
                error: Some(format!("Connection error: {}", e)),
            }.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use switch_configurator::models::VlanIpConfig;

    fn vlan(id: u16, name: &str) -> Vlan {
        Vlan { id, name: name.to_string(), description: None, ip_config: VlanIpConfig::None }
    }

    #[test]
    fn test_build_save_body_includes_identity_fields() {
        // Regression test: found via live verification on it-02634 — saving
        // a fix to an overlay whose identity lives entirely in that overlay
        // (no main config counterpart) silently stripped model/
        // management_ip/credentials, turning a fixed VLAN collision into a
        // new "missing required fields" failure.
        use switch_configurator::models::{Credentials, ConnectionType, SwitchConfig, SwitchModel};

        let edited = SwitchConfig {
            id: "demo-broken-switch".to_string(),
            hostname: Some("demo-broken-switch".to_string()),
            model: Some(SwitchModel::Aruba2530_24G_POE),
            management_ip: Some("203.0.113.1".to_string()),
            credentials: Some(Credentials {
                username: "demo".to_string(),
                password: Some("demo".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            settings: Default::default(),
        };
        let form = SaveForm { filename: "demo-broken-switch.yaml".to_string(), priority: 200 };

        let body = build_save_body("demo-broken-switch", &form, &edited, vec![], vec![]);

        let switch = &body["config"]["switches"][0];
        assert_eq!(switch["model"], "Aruba2530_24G_POE");
        assert_eq!(switch["management_ip"], "203.0.113.1");
        assert_eq!(switch["credentials"]["username"], "demo");
    }

    #[test]
    fn test_default_overlay_filename_uses_disputed_overlay_for_broken_switch() {
        let mut vlan_sources = std::collections::HashMap::new();
        vlan_sources.insert(10u16, "/etc/main.yaml".to_string());
        vlan_sources.insert(99u16, "/etc/switch-configurator/Shadow-Overlay.yaml".to_string());

        let filename = default_overlay_filename(&vlan_sources, &std::collections::HashMap::new(), "/etc/main.yaml", "test-switch");
        assert_eq!(filename, "Shadow-Overlay.yaml", "should default to the disputed overlay's own file, not a new one");
    }

    #[test]
    fn test_default_overlay_filename_falls_back_to_switch_id_when_healthy() {
        let filename = default_overlay_filename(
            &std::collections::HashMap::new(), &std::collections::HashMap::new(), "/etc/main.yaml", "test-switch",
        );
        assert_eq!(filename, "test-switch.yaml");
    }

    #[test]
    fn test_exclude_main_config_rows_keeps_only_overlay_rows() {
        let vlans = vec![vlan(10, "users"), vlan(99, "users")];
        let mut vlan_sources = std::collections::HashMap::new();
        vlan_sources.insert(10, "/etc/main.yaml".to_string());
        vlan_sources.insert(99, "/etc/switch-configurator/overlay.yaml".to_string());

        let (kept_vlans, _) = exclude_main_config_rows(
            &vlans, &vlan_sources, &[], &std::collections::HashMap::new(), "/etc/main.yaml",
        );

        assert_eq!(kept_vlans.len(), 1);
        assert_eq!(kept_vlans[0].id, 99, "the main config's own row must never be written into the overlay");
    }

    #[test]
    fn test_exclude_main_config_rows_keeps_everything_when_no_attribution() {
        // An ordinary healthy-switch draft has no source attribution at all
        // — nothing should be filtered out.
        let vlans = vec![vlan(10, "users"), vlan(20, "servers")];
        let (kept, _) = exclude_main_config_rows(
            &vlans, &std::collections::HashMap::new(), &[], &std::collections::HashMap::new(), "/etc/main.yaml",
        );
        assert_eq!(kept.len(), 2);
    }
}
