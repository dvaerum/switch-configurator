use crate::config::{validate_switch_config, AppConfig, ConfigStore, SseEvent};
use tokio_stream::StreamExt;
use crate::models::{
    Credentials, Port, PortMirror, SnmpConfig, SwitchConfig, SwitchModel, Vendor, Vlan,
};
use crate::vendors;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info, warn};

/// SSE endpoint for real-time status updates
pub async fn events(
    State(store): State<ConfigStore>,
) -> axum::response::sse::Sse<impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = store.events.subscribe();

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|result| {
            match result {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    let event_name = match &event {
                        crate::config::SseEvent::Status { .. } => "status",
                        crate::config::SseEvent::ConfigReload { .. } => "config-reload",
                        crate::config::SseEvent::Warning { .. } => "warning",
                        crate::config::SseEvent::PoeReset { .. } => "poe-reset",
                    };
                    Some(Ok(axum::response::sse::Event::default()
                        .event(event_name)
                        .data(data)))
                }
                Err(_) => None,
            }
        });

    axum::response::sse::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
}

/// Request body for POST /switches/{id}/preview-diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewDiffRequest {
    pub current_state: Option<crate::models::SwitchState>,
}

/// Request body for POST /switches/{id}/save-overlay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveOverlayRequest {
    pub filename: String,
    pub merge_priority: u16,
    pub config: crate::config::AppConfig,
}

/// Request body for PUT /switches/{id}/config (create/overwrite)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSwitchConfigRequest {
    /// Unique identifier - optional, if provided must match URL parameter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Switch hostname (required for new switches)
    pub hostname: Option<String>,

    /// Switch model (required for new switches)
    pub model: Option<SwitchModel>,

    /// Management IP address (required for new switches)
    pub management_ip: Option<String>,

    /// SSH/Serial credentials (required for new switches)
    pub credentials: Option<Credentials>,

    /// VLANs to configure
    #[serde(default)]
    pub vlans: Vec<Vlan>,

    /// Ports to configure
    #[serde(default)]
    pub ports: Vec<Port>,

    /// Port mirroring configurations
    #[serde(default)]
    pub port_mirrors: Vec<PortMirror>,

    /// SNMP configuration
    #[serde(default)]
    pub snmp: Option<SnmpConfig>,

    /// Management VLAN
    #[serde(default)]
    pub management_vlan: Option<u16>,
}

/// Request body for PATCH /switches/{id}/config (partial update)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSwitchConfigRequest {
    /// Unique identifier - optional, if provided must match URL parameter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Optional hostname update
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Optional model update
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SwitchModel>,

    /// Optional management IP update
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_ip: Option<String>,

    /// Optional credentials update
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,

    /// VLANs to add/update (merged by VLAN id)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlans: Option<Vec<Vlan>>,

    /// Ports to add/update (merged by port_id)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<Port>>,

    /// Port mirrors to add/update (merged by session_id)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_mirrors: Option<Vec<PortMirror>>,

    /// SNMP configuration (replaces entire SNMP config if provided)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snmp: Option<SnmpConfig>,

    /// Management VLAN
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_vlan: Option<u16>,
}

/// Health check endpoint
pub async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "switch-configurator"
    }))
}

/// List all configured switches
pub async fn list_switches(State(store): State<ConfigStore>) -> impl IntoResponse {
    let config = store.config.read().await;
    let switches: Vec<_> = config
        .switches
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "hostname": s.hostname,
                "model": s.model,
                "management_ip": s.management_ip,
                "vlans": s.vlans.len(),
                "ports": s.ports.len(),
            })
        })
        .collect();

    Json(json!({
        "switches": switches,
        "count": switches.len()
    }))
}

/// Apply configuration to a specific switch
///
/// Always runs asynchronously in background and returns 202 Accepted immediately.
/// Client should poll /api/status to check progress:
/// - `currently_configuring` field shows which switch is being configured (null when done)
/// - `switches[].last_result` shows the outcome after completion
///
/// Returns 409 Conflict if another configuration is already in progress.
pub async fn apply_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config = store.config.read().await;

    let switch_config = match config
        .switches
        .iter()
        .find(|s| s.id == id)
    {
        Some(cfg) => cfg.clone(),
        None => {
            // Record error
            store.status.record_error(
                "NotFound".to_string(),
                format!("Switch with id '{}' not found", id),
                Some(id.clone()),
                "apply_config".to_string(),
            ).await;

            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Switch with id '{}' not found", id)})),
            )
                .into_response();
        }
    };

    drop(config);

    // Check if this specific switch is busy (being configured or has pending reload)
    if store.status.is_switch_busy(&id).await {
        let reason = if store.status.is_currently_configuring(&id).await {
            "is already being configured"
        } else {
            "has a pending config reload queued"
        };
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("Switch '{}' {}", id, reason),
                "switch_id": id
            })),
        )
            .into_response();
    }

    // Spawn background task and return immediately
    let store_clone = store.clone();
    let id_clone = id.clone();
    let switch_config_clone = switch_config.clone();

    tokio::spawn(async move {
        match apply_config_impl(store_clone, id_clone.clone(), switch_config_clone).await {
            Ok(results) => {
                info!("Apply completed for switch '{}': {} results", id_clone, results.len());
            }
            Err(e) => {
                error!("Apply failed for switch '{}': {}", id_clone, e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": format!("Configuration apply started for switch '{}'", id),
            "switch_id": id,
            "poll_url": "/api/status",
            "hint": "Poll /api/status and check 'currently_configuring' array. When empty or switch not in list, check switches[].last_result for the outcome."
        })),
    )
        .into_response()
}

/// Internal implementation of apply_config that can be called sync or async
async fn apply_config_impl(
    store: ConfigStore,
    id: String,
    switch_config: SwitchConfig,
) -> Result<Vec<crate::models::ConfigResult>, String> {
    let enforce_port_config = switch_config.settings.enforce_port_config;

    // Mark this switch as currently being configured
    store.status.set_currently_configuring(id.clone()).await;

    info!("Applying configuration to switch: {} ({})", id, switch_config.hostname.as_ref().unwrap_or(&id));

    // Create vendor-specific implementation
    let mut vendor = match vendors::create_vendor_with_runtime(&switch_config, &crate::config::RuntimeConfig::default(), enforce_port_config) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to create vendor implementation: {}", e);

            // Record error
            store.status.record_error(
                "VendorCreation".to_string(),
                e.to_string(),
                Some(id.clone()),
                "apply_config".to_string(),
            ).await;
            store.status.record_apply_failure(&id, &e.to_string()).await;
            store.status.clear_currently_configuring(&id).await;

            return Err(e.to_string());
        }
    };

    // Connect to switch
    if let Err(e) = vendor.connect().await {
        error!("Failed to connect to switch: {}", e);

        // Record error
        store.status.record_error(
            "Connection".to_string(),
            e.to_string(),
            Some(id.clone()),
            "apply_config".to_string(),
        ).await;
        store.status.record_apply_failure(&id, &format!("Connection failed: {}", e)).await;
        store.status.clear_currently_configuring(&id).await;

        return Err(format!("Connection failed: {}", e));
    }

    // Apply configuration
    let results = match vendor.apply_configuration().await {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to apply configuration: {}", e);
            let _ = vendor.disconnect().await;

            // Record error
            store.status.record_error(
                "ApplyConfiguration".to_string(),
                e.to_string(),
                Some(id.clone()),
                "apply_config".to_string(),
            ).await;
            store.status.record_apply_failure(&id, &e.to_string()).await;
            store.status.clear_currently_configuring(&id).await;

            return Err(format!("Configuration failed: {}", e));
        }
    };

    // Save configuration
    if let Err(e) = vendor.save_configuration().await {
        error!("Failed to save configuration: {}", e);

        // Record error (non-fatal)
        store.status.record_error(
            "SaveConfiguration".to_string(),
            e.to_string(),
            Some(id.clone()),
            "apply_config".to_string(),
        ).await;
    }

    // Disconnect
    let _ = vendor.disconnect().await;

    info!("Successfully applied configuration to {} ({})", id, switch_config.hostname.as_ref().unwrap_or(&id));

    // Record warnings from state parsing (e.g., model mismatch)
    let warnings = vendor.get_warnings();
    if !warnings.is_empty() {
        for w in &warnings {
            warn!("Switch {}: {}", id, w);
        }
    }
    store.status.record_warnings(&id, warnings).await;

    // Record success
    store.status.record_apply_success(&id).await;
    store.status.clear_currently_configuring(&id).await;

    Ok(results)
}

/// Preview the diff and CLI commands that would be applied, without executing them.
/// Accepts an optional `current_state` in the request body; if omitted, connects to
/// the switch to read the current running configuration.
pub async fn preview_diff(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
    Json(body): Json<PreviewDiffRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let config = store.config.read().await;

    let switch_config = config
        .switches
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Switch '{}' not found", id)})),
            )
        })?
        .clone();

    let current_state = match body.current_state {
        Some(state) => state,
        None => {
            // Connect to switch and parse current state
            drop(config);
            let mut vendor = vendors::create_vendor_with_runtime(
                &switch_config,
                &crate::config::RuntimeConfig::default(),
                switch_config.settings.enforce_port_config,
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to create vendor: {}", e)})),
                )
            })?;

            vendor.connect().await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to connect: {}", e)})),
                )
            })?;

            let state = vendor.parse_current_state().await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to parse state: {}", e)})),
                )
            })?;

            let _ = vendor.disconnect().await;
            state
        }
    };

    let diff = crate::diff::compute_diff(
        &current_state,
        &switch_config,
        switch_config.settings.enforce_port_config,
    );

    // Generate command preview using the vendor
    let vendor = vendors::create_vendor_with_runtime(
        &switch_config,
        &crate::config::RuntimeConfig::default(),
        switch_config.settings.enforce_port_config,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to create vendor: {}", e)})),
        )
    })?;

    let commands = vendor.generate_commands_for_diff(&diff);

    Ok(Json(json!({
        "switch_id": id,
        "has_changes": diff.has_changes(),
        "diff": diff,
        "commands": commands,
    })))
}

/// Save switch config changes to an overlay YAML file in the config folder
pub async fn save_overlay(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
    Json(mut body): Json<SaveOverlayRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Validate filename: no path traversal, must end in .yaml or .yml
    let filename = &body.filename;
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid filename: path traversal not allowed"})),
        ));
    }
    if !filename.ends_with(".yaml") && !filename.ends_with(".yml") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Filename must end with .yaml or .yml"})),
        ));
    }

    // Validate priority range for folder configs (11-9999)
    if body.merge_priority <= 10 || body.merge_priority > 9999 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "merge_priority must be between 11 and 9999 for overlay files"})),
        ));
    }

    // Validate the overlay config before saving — checks VLAN references,
    // duplicate IDs, VLAN range, port ranges. Skips identity field checks
    // since overlays are partial configs.
    for switch in &mut body.config.switches {
        if let Err(e) = crate::config::validate_overlay_config(switch) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Validation failed: {}", e)})),
            ));
        }
    }

    let config_folder = get_first_config_folder(&store).await?;

    // Build the YAML content with merge_priority at the top
    let yaml_content = format!(
        "merge_priority: {}\n\n{}",
        body.merge_priority,
        serde_yaml::to_string(&body.config).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to serialize config: {}", e)})),
            )
        })?
    );

    let file_path = config_folder.join(filename);
    std::fs::write(&file_path, &yaml_content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to write file: {}", e)})),
        )
    })?;

    info!("Saved overlay config to {}", file_path.display());

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "saved",
            "file": file_path.to_string_lossy(),
            "merge_priority": body.merge_priority,
        })),
    ))
}

/// Read an overlay config file's raw YAML content
pub async fn read_overlay(
    State(store): State<ConfigStore>,
    Path((_id, filename)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    validate_overlay_filename(&filename)?;

    let config_folder = get_first_config_folder(&store).await?;
    let file_path = config_folder.join(&filename);

    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": format!("File '{}' not found", filename)}))));
    }

    let content = std::fs::read_to_string(&file_path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to read file: {}", e)})))
    })?;

    Ok(([(axum::http::header::CONTENT_TYPE, "text/yaml")], content))
}

/// Delete an overlay config file
pub async fn delete_overlay(
    State(store): State<ConfigStore>,
    Path((_id, filename)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    validate_overlay_filename(&filename)?;

    let config_folder = get_first_config_folder(&store).await?;
    let file_path = config_folder.join(&filename);

    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": format!("File '{}' not found", filename)}))));
    }

    std::fs::remove_file(&file_path).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Failed to delete file: {}", e)})))
    })?;

    info!("Deleted overlay config: {}", file_path.display());

    Ok(Json(json!({"status": "deleted", "file": filename})))
}

fn validate_overlay_filename(filename: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "Invalid filename: path traversal not allowed"}))));
    }
    if !filename.ends_with(".yaml") && !filename.ends_with(".yml") {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "Filename must end with .yaml or .yml"}))));
    }
    Ok(())
}

async fn get_first_config_folder(store: &ConfigStore) -> Result<std::path::PathBuf, (StatusCode, Json<serde_json::Value>)> {
    store.status.get_config_paths().await
        .and_then(|(_, folders)| folders.into_iter().next())
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "No config folder configured"}))))
}

/// Get configuration source files and their priorities for a switch
pub async fn config_sources(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Verify switch exists
    let config = store.config.read().await;
    if !config.switches.iter().any(|s| s.id == id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Switch '{}' not found", id)})),
        ));
    }
    drop(config);

    let config_paths = store.status.get_config_paths().await;
    let mut sources = Vec::new();

    if let Some((main_file, folders)) = config_paths {
        // Main config file — read actual merge_priority
        let main_priority = std::fs::read_to_string(&main_file)
            .ok()
            .and_then(|content| {
                serde_yaml::from_str::<crate::config::AppConfigFile>(&content).ok()
            })
            .and_then(|f| f.merge_priority)
            .unwrap_or(50);

        sources.push(json!({
            "file": main_file.to_string_lossy(),
            "priority": main_priority,
            "source_type": "main",
        }));

        // Scan config folders for overlay files
        for folder in &folders {
            if let Ok(entries) = std::fs::read_dir(folder) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                        // Try to read the merge_priority from the file
                        let priority = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|content| {
                                serde_yaml::from_str::<crate::config::AppConfigFile>(&content).ok()
                            })
                            .and_then(|f| f.merge_priority)
                            .unwrap_or(100);

                        sources.push(json!({
                            "file": path.to_string_lossy(),
                            "priority": priority,
                            "source_type": "folder",
                        }));
                    }
                }
            }
        }
    }

    // Sort by priority (lowest first = highest priority)
    sources.sort_by_key(|s| s["priority"].as_u64().unwrap_or(u64::MAX));

    Ok(Json(json!({
        "switch_id": id,
        "sources": sources,
    })))
}

/// Get running configuration from a switch
pub async fn get_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config = store.config.read().await;

    let switch_config = match config
        .switches
        .iter()
        .find(|s| s.id == id)
    {
        Some(cfg) => cfg.clone(),
        None => {
            // Record error
            store.status.record_error(
                "NotFound".to_string(),
                format!("Switch with id '{}' not found", id),
                Some(id.clone()),
                "get_config".to_string(),
            ).await;

            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Switch with id '{}' not found", id)})),
            )
                .into_response();
        }
    };

    let enforce_port_config = switch_config.settings.enforce_port_config;
    drop(config);

    // Check if this switch is busy (would conflict, especially for serial)
    if store.status.is_switch_busy(&id).await {
        let reason = if store.status.is_currently_configuring(&id).await {
            "is currently being configured"
        } else {
            "has a pending config reload queued"
        };
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("Switch '{}' {}", id, reason),
                "switch_id": id
            })),
        )
            .into_response();
    }

    // Create vendor-specific implementation
    let mut vendor = match vendors::create_vendor_with_runtime(&switch_config, &crate::config::RuntimeConfig::default(), enforce_port_config) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to create vendor implementation: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Connect and get config
    if let Err(e) = vendor.connect().await {
        error!("Failed to connect to switch: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Connection failed: {}", e)})),
        )
            .into_response();
    }

    let running_config = match vendor.get_running_config().await {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to get running config: {}", e);
            let _ = vendor.disconnect().await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to retrieve config: {}", e)})),
            )
                .into_response();
        }
    };

    // Parse the current state from the running config
    let parsed_state = match vendor.parse_current_state().await {
        Ok(state) => Some(state),
        Err(e) => {
            warn!("Failed to parse current state (raw config still available): {}", e);
            None
        }
    };

    let _ = vendor.disconnect().await;

    (
        StatusCode::OK,
        Json(json!({
            "id": switch_config.id,
            "hostname": switch_config.hostname,
            "model": switch_config.model,
            "management_ip": switch_config.management_ip,
            "raw_config": running_config,
            "parsed_state": parsed_state
        })),
    )
        .into_response()
}

/// Reload configuration from YAML files and apply to all switches
///
/// POST /config/reload
///
/// Re-reads YAML configuration files from disk, updates in-memory config,
/// and applies configuration to all switches. Each switch is configured
/// in a separate background task running in parallel.
///
/// Returns 202 Accepted immediately with status of which switches will be configured.
/// Switches that are currently busy (already being configured) will be skipped.
/// Poll /api/status to check progress.
pub async fn reload_config(State(store): State<ConfigStore>) -> impl IntoResponse {
    info!("Global configuration reload requested");

    // Get config paths from status tracker
    let config_paths = match store.status.get_config_paths().await {
        Some(paths) => paths,
        None => {
            error!("Configuration metadata not available for reload");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Configuration metadata not available. Service may not be fully initialized."
                })),
            )
                .into_response();
        }
    };

    let (config_file, config_folders) = config_paths;

    // Reload configuration from YAML files
    let reload_result = if config_folders.is_empty() {
        AppConfig::load(&config_file)
    } else {
        AppConfig::load_multi(&config_file, &config_folders)
    };

    let (new_config, validation_failures) = match reload_result {
        Ok(result) => result,
        Err(e) => {
            error!("Failed to reload configuration from files: {}", e);
            store.status.record_error(
                "ConfigReload".to_string(),
                e.to_string(),
                None,
                "reload_config".to_string(),
            ).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to reload configuration: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Store validation failures
    if !validation_failures.is_empty() {
        for f in &validation_failures {
            warn!("Switch '{}' skipped: {}", f.switch_id, f.error);
        }
    }
    *store.validation_failures.write().await = validation_failures;

    info!("Reloaded configuration with {} switches", new_config.switches.len());

    // Update the entire in-memory config
    {
        let mut config = store.config.write().await;
        *config = new_config.clone();
    }

    // Re-initialize switch statuses with updated configuration
    store.status.initialize_switches(&new_config.switches).await;

    // Track which switches we'll configure and which are busy
    let mut switches_to_configure = Vec::new();
    let mut switches_skipped = Vec::new();

    for switch_config in &new_config.switches {
        if store.status.is_switch_busy(&switch_config.id).await {
            warn!("Switch '{}' is busy, skipping configuration", switch_config.id);
            switches_skipped.push(switch_config.id.clone());
        } else {
            switches_to_configure.push(switch_config.clone());
        }
    }

    // Spawn background tasks for each switch (in parallel)
    for switch_config in switches_to_configure.iter().cloned() {
        let store_clone = store.clone();
        let id = switch_config.id.clone();

        tokio::spawn(async move {
            match apply_config_impl(store_clone, id.clone(), switch_config).await {
                Ok(results) => {
                    info!("Reload+apply completed for switch '{}': {} results", id, results.len());
                }
                Err(e) => {
                    error!("Reload+apply failed for switch '{}': {}", id, e);
                }
            }
        });
    }

    let configuring_ids: Vec<String> = switches_to_configure.iter().map(|s| s.id.clone()).collect();

    info!(
        "Started configuration for {} switches, skipped {} busy switches",
        configuring_ids.len(),
        switches_skipped.len()
    );

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": format!(
                "Configuration reload started for {} switch(es)",
                configuring_ids.len()
            ),
            "switches_configuring": configuring_ids,
            "switches_skipped": switches_skipped,
            "poll_url": "/api/status",
            "hint": "Poll /api/status and check 'currently_configuring' array. When empty, all switches are done. Check switches[].last_result for outcomes."
        })),
    )
        .into_response()
}

/// Reload configuration from YAML files and apply to a specific switch
///
/// POST /switches/{id}/reload
///
/// This does the same as POST /config/reload but only applies to one switch.
/// It re-reads YAML files from disk, updates in-memory config, and applies
/// only to the specified switch.
///
/// Returns 202 Accepted immediately and processes in background.
pub async fn reload_switch_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    info!("Switch-specific reload requested for: {}", id);

    // Check if this switch is busy FIRST (before doing any I/O)
    if store.status.is_switch_busy(&id).await {
        let reason = if store.status.is_currently_configuring(&id).await {
            "is already being configured"
        } else {
            "has a pending config reload queued"
        };
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("Switch '{}' {}", id, reason),
                "switch_id": id
            })),
        )
            .into_response();
    }

    // Get config paths from status tracker
    let config_paths = match store.status.get_config_paths().await {
        Some(paths) => paths,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Configuration metadata not available. Service may not be fully initialized."
                })),
            )
                .into_response();
        }
    };

    let (config_file, config_folders) = config_paths;

    // Reload configuration from YAML files
    let reload_result = if config_folders.is_empty() {
        AppConfig::load(&config_file)
    } else {
        AppConfig::load_multi(&config_file, &config_folders)
    };

    let (new_config, validation_failures) = match reload_result {
        Ok(result) => result,
        Err(e) => {
            error!("Failed to reload configuration: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to reload configuration: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Update validation failures
    *store.validation_failures.write().await = validation_failures;

    // Find the specific switch in the new config
    let switch_config = match new_config.switches.iter().find(|s| s.id == id) {
        Some(cfg) => cfg.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": format!("Switch with id '{}' not found in configuration files", id)
                })),
            )
                .into_response();
        }
    };

    // Update the in-memory config for just this switch
    {
        let mut config = store.config.write().await;
        if let Some(existing) = config.switches.iter_mut().find(|s| s.id == id) {
            *existing = switch_config.clone();
        } else {
            // Switch is new in config files, add it
            config.switches.push(switch_config.clone());
        }
    }

    // Re-initialize switch status with updated configuration
    store.status.initialize_switches(&[switch_config.clone()]).await;

    // Spawn background task to apply configuration
    let store_clone = store.clone();
    let id_clone = id.clone();
    let switch_config_clone = switch_config.clone();

    tokio::spawn(async move {
        match apply_config_impl(store_clone, id_clone.clone(), switch_config_clone).await {
            Ok(results) => {
                info!("Reload+apply completed for switch '{}': {} results", id_clone, results.len());
            }
            Err(e) => {
                error!("Reload+apply failed for switch '{}': {}", id_clone, e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": format!("Configuration reload and apply started for switch '{}'", id),
            "switch_id": id,
            "poll_url": "/api/status",
            "hint": "Poll /api/status and check 'currently_configuring' array. When empty or switch not in list, check switches[].last_result for the outcome."
        })),
    )
        .into_response()
}

/// Get detailed service status
pub async fn status(State(store): State<ConfigStore>) -> impl IntoResponse {
    let service_status = store.status.get_status(store.api_port).await;
    let validation_failures = store.validation_failures.read().await;

    // Merge validation failures into the response
    let mut response = serde_json::to_value(&service_status).unwrap_or_default();
    if !validation_failures.is_empty() {
        response["validation_failures"] = serde_json::to_value(&*validation_failures).unwrap_or_default();
    }

    Json(response)
}

/// Set (create or overwrite) switch configuration in memory
///
/// PUT /switches/{id}/desired-config
///
/// Creates a new switch config or completely replaces an existing one.
/// The `id` in the request body is optional. If provided, it must match the URL parameter.
/// For new switches, hostname, model, management_ip, and credentials are required.
pub async fn set_switch_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
    Json(request): Json<SetSwitchConfigRequest>,
) -> impl IntoResponse {
    // Validate ID matches if provided in body
    if let Some(ref body_id) = request.id {
        if body_id != &id {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "ID mismatch: URL parameter '{}' does not match request body id '{}'",
                        id, body_id
                    )
                })),
            )
                .into_response();
        }
    }

    // Check if switch already exists
    let mut config = store.config.write().await;
    let existing_index = config.switches.iter().position(|s| s.id == id);

    // For new switches, validate required fields
    if existing_index.is_none() {
        let mut missing = Vec::new();
        if request.hostname.is_none() {
            missing.push("hostname");
        }
        if request.model.is_none() {
            missing.push("model");
        }
        if request.management_ip.is_none() {
            missing.push("management_ip");
        }
        if request.credentials.is_none() {
            missing.push("credentials");
        }

        if !missing.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "New switch '{}' requires fields: {}",
                        id,
                        missing.join(", ")
                    )
                })),
            )
                .into_response();
        }
    }

    // Build the new switch config (use URL id, not body id)
    let mut new_config = SwitchConfig {
        id: id.clone(),
        hostname: request.hostname,
        model: request.model,
        management_ip: request.management_ip,
        credentials: request.credentials,
        vlans: request.vlans,
        ports: request.ports,
        port_mirrors: request.port_mirrors,
        snmp: request.snmp,
        management_vlan: request.management_vlan,
        validation: None,
        vendor_specific: std::collections::HashMap::new(),
        settings: crate::config::Settings::default(),
    };

    // Validate the configuration (expands port ranges, validates fields and VLAN references)
    if let Err(e) = validate_switch_config(&mut new_config) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Configuration validation failed",
                "details": e.to_string()
            })),
        )
            .into_response();
    }

    // Insert or replace
    let is_new = existing_index.is_none();
    if let Some(idx) = existing_index {
        info!("Replacing switch config for '{}'", id);
        config.switches[idx] = new_config;
    } else {
        info!("Creating new switch config for '{}'", id);
        config.switches.push(new_config);
    }

    drop(config);

    (
        if is_new { StatusCode::CREATED } else { StatusCode::OK },
        Json(json!({
            "status": "ok",
            "message": if is_new {
                format!("Switch '{}' created", id)
            } else {
                format!("Switch '{}' config replaced", id)
            },
            "switch_id": id
        })),
    )
        .into_response()
}

/// Patch (partial update) switch configuration in memory
///
/// PATCH /switches/{id}/desired-config
///
/// Updates specific fields of an existing switch config.
/// The `id` in the request body is optional. If provided, it must match the URL parameter.
/// The switch must already exist.
///
/// Merge behavior:
/// - Simple fields (hostname, model, etc.): Replace if provided
/// - vlans: Merge by VLAN id (add new, update existing)
/// - ports: Merge by port_id (add new, update existing)
/// - port_mirrors: Merge by session_id (add new, update existing)
/// - snmp: Replace entire config if provided
pub async fn patch_switch_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
    Json(request): Json<PatchSwitchConfigRequest>,
) -> impl IntoResponse {
    // Validate ID matches if provided in body
    if let Some(ref body_id) = request.id {
        if body_id != &id {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "ID mismatch: URL parameter '{}' does not match request body id '{}'",
                        id, body_id
                    )
                })),
            )
                .into_response();
        }
    }

    let mut config = store.config.write().await;

    // Find existing switch index
    let switch_index = match config.switches.iter().position(|s| s.id == id) {
        Some(idx) => idx,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": format!("Switch '{}' not found. Use PUT to create new switches.", id)
                })),
            )
                .into_response();
        }
    };

    info!("Patching switch config for '{}'", id);

    // Clone original for rollback in case validation fails
    let original_switch = config.switches[switch_index].clone();

    // Apply patches to the switch
    let switch = &mut config.switches[switch_index];

    // Update simple fields if provided
    if let Some(hostname) = request.hostname {
        switch.hostname = Some(hostname);
    }
    if let Some(model) = request.model {
        switch.model = Some(model);
    }
    if let Some(management_ip) = request.management_ip {
        switch.management_ip = Some(management_ip);
    }
    if let Some(credentials) = request.credentials {
        switch.credentials = Some(credentials);
    }
    if let Some(management_vlan) = request.management_vlan {
        switch.management_vlan = Some(management_vlan);
    }

    // Merge VLANs by id
    if let Some(new_vlans) = request.vlans {
        for new_vlan in new_vlans {
            if let Some(existing) = switch.vlans.iter_mut().find(|v| v.id == new_vlan.id) {
                *existing = new_vlan;
            } else {
                switch.vlans.push(new_vlan);
            }
        }
    }

    // Merge ports by port_id
    if let Some(new_ports) = request.ports {
        for new_port in new_ports {
            if let Some(existing) = switch.ports.iter_mut().find(|p| p.port_id == new_port.port_id)
            {
                *existing = new_port;
            } else {
                switch.ports.push(new_port);
            }
        }
    }

    // Merge port_mirrors by session_id
    if let Some(new_mirrors) = request.port_mirrors {
        for new_mirror in new_mirrors {
            if let Some(existing) = switch
                .port_mirrors
                .iter_mut()
                .find(|m| m.session_id == new_mirror.session_id)
            {
                *existing = new_mirror;
            } else {
                switch.port_mirrors.push(new_mirror);
            }
        }
    }

    // Replace SNMP config entirely if provided
    if request.snmp.is_some() {
        switch.snmp = request.snmp;
    }

    // Validate the updated configuration
    // Note: validate_switch_config expands port ranges, validates fields and VLAN references
    let switch = &mut config.switches[switch_index];
    if let Err(e) = validate_switch_config(switch) {
        // Restore original config on validation failure
        config.switches[switch_index] = original_switch;
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Configuration validation failed",
                "details": e.to_string()
            })),
        )
            .into_response();
    }

    drop(config);

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "message": format!("Switch '{}' config updated", id),
            "switch_id": id
        })),
    )
        .into_response()
}

/// Get the in-memory configuration for a specific switch (not the running config from hardware)
///
/// GET /switches/{id}/desired-config
///
/// Returns the current desired configuration stored in memory for the given switch.
/// This is different from GET /switches/{id}/config which retrieves the actual
/// running configuration from the switch hardware via SSH.
pub async fn get_desired_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config = store.config.read().await;

    match config.switches.iter().find(|s| s.id == id) {
        Some(switch) => (StatusCode::OK, Json(json!(switch))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Switch '{}' not found", id)})),
        )
            .into_response(),
    }
}

/// Delete a switch configuration from memory
///
/// DELETE /switches/{id}/config
///
/// Removes the switch from in-memory configuration.
/// Note: This does NOT affect the switch hardware or any YAML files.
pub async fn delete_switch_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut config = store.config.write().await;

    let original_len = config.switches.len();
    config.switches.retain(|s| s.id != id);

    if config.switches.len() < original_len {
        info!("Deleted switch config for '{}'", id);
        (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "message": format!("Switch '{}' deleted", id)
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Switch '{}' not found", id)})),
        )
            .into_response()
    }
}

pub async fn poe_reset(
    State(store): State<ConfigStore>,
    Path((id, port_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let config = store.config.read().await;
    let switch_config = match config.switches.iter().find(|s| s.id == id) {
        Some(cfg) => cfg.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Switch '{}' not found", id)})),
            )
                .into_response();
        }
    };
    drop(config);

    let model = switch_config.model();

    if !model.supports_poe() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Switch model {:?} does not support PoE", model)})),
        )
            .into_response();
    }

    if !model.port_supports_poe(&port_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Port {} does not support PoE on {:?}", port_id, model)})),
        )
            .into_response();
    }

    if model.vendor() != Vendor::Aruba {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("PoE reset not yet supported for {:?} switches", model.vendor())})),
        )
            .into_response();
    }

    if store.status.is_switch_busy(&id).await {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": format!("Switch '{}' is busy", id), "switch_id": id})),
        )
            .into_response();
    }

    store.status.set_currently_configuring(id.clone()).await;

    // Run the reset in the background and stream progress via SSE.
    // Returns 202 immediately; the UI listens on /api/events for the staged
    // connecting -> disabling -> waiting(3,2,1) -> enabling -> done|failed events.
    let store_bg = store.clone();
    let id_bg = id.clone();
    let port_bg = port_id.clone();
    let cfg_bg = switch_config.clone();

    tokio::spawn(async move {
        let result = poe_reset_impl(&store_bg, &id_bg, &cfg_bg, &port_bg).await;
        match &result {
            Ok(()) => {
                store_bg.emit_event(SseEvent::PoeReset {
                    switch_id: id_bg.clone(),
                    port_id: port_bg.clone(),
                    stage: "done".to_string(),
                    detail: None,
                });
                info!("PoE reset completed for {}:{}", id_bg, port_bg);
            }
            Err(error_msg) => {
                store_bg.emit_event(SseEvent::PoeReset {
                    switch_id: id_bg.clone(),
                    port_id: port_bg.clone(),
                    stage: "failed".to_string(),
                    detail: Some(error_msg.clone()),
                });
                error!("PoE reset failed for {}:{}: {}", id_bg, port_bg, error_msg);
                store_bg
                    .status
                    .record_error(
                        "PoeReset".to_string(),
                        error_msg.clone(),
                        Some(id_bg.clone()),
                        "poe_reset".to_string(),
                    )
                    .await;
            }
        }
        store_bg.status.clear_currently_configuring(&id_bg).await;
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": format!("PoE reset started for port {} on switch '{}'", port_id, id),
            "switch_id": id,
            "port_id": port_id,
            "hint": "Listen on /api/events for poe-reset stage events"
        })),
    )
        .into_response()
}

async fn poe_reset_impl(
    store: &ConfigStore,
    id: &str,
    switch_config: &SwitchConfig,
    port_id: &str,
) -> Result<(), String> {
    let emit = |stage: &str, detail: Option<String>| {
        store.emit_event(SseEvent::PoeReset {
            switch_id: id.to_string(),
            port_id: port_id.to_string(),
            stage: stage.to_string(),
            detail,
        });
    };

    emit("connecting", None);

    let mut vendor = vendors::create_vendor_with_runtime(
        switch_config,
        &crate::config::RuntimeConfig::default(),
        switch_config.settings.enforce_port_config,
    )
    .map_err(|e| format!("Failed to create vendor: {}", e))?;

    vendor
        .connect()
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    let aruba = vendors::aruba::ArubaSwitch::new(
        switch_config.clone(),
        crate::config::RuntimeConfig::default(),
        false,
    );
    let disable_cmds = aruba.poe_disable_commands(port_id);
    let enable_cmds = aruba.poe_enable_commands(port_id);

    emit("disabling", None);
    if let Err(e) = vendor.execute_raw_commands(&disable_cmds).await {
        let _ = vendor.disconnect().await;
        return Err(format!("Failed to disable PoE: {}", e));
    }

    info!("PoE disabled on port {}, waiting 3 seconds...", port_id);
    // Emit one tick per second so the dashboard countdown reflects the real
    // backend timer rather than a client-side guess.
    for n in (1..=3).rev() {
        emit("waiting", Some(n.to_string()));
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    emit("enabling", None);
    if let Err(e) = vendor.execute_raw_commands(&enable_cmds).await {
        warn!(
            "First PoE re-enable attempt failed for port {}: {}. Retrying...",
            port_id, e
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Err(e2) = vendor.execute_raw_commands(&enable_cmds).await {
            let _ = vendor.disconnect().await;
            return Err(format!(
                "PoE was DISABLED on port {} but re-enable FAILED after retry: {}. Manual intervention required.",
                port_id, e2
            ));
        }
    }

    info!("PoE re-enabled on port {}", port_id);
    let _ = vendor.disconnect().await;

    Ok(())
}
