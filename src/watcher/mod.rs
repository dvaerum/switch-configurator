use crate::config::{AppConfig, ConfigStore};
use crate::status::StatusTracker;
use crate::vendors;
use anyhow::{Context, Result};
use notify::{Event, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

pub async fn start(
    config_path: PathBuf,
    config_folders: Vec<PathBuf>,
    store: ConfigStore,
) -> Result<()> {
    info!("Starting file watcher for: {:?}", config_path);
    if !config_folders.is_empty() {
        info!("Also watching config folders: {:?}", config_folders);
    }

    let config_path = Arc::new(config_path);
    let config_folders = Arc::new(config_folders);

    // Create channel for file system events
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Create file watcher
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        match res {
            Ok(event) => {
                if let Err(e) = tx.blocking_send(event) {
                    error!("Failed to send file watch event: {}", e);
                }
            }
            Err(e) => error!("File watch error: {}", e),
        }
    })?;

    // Watch the main config file directly
    watcher
        .watch(&*config_path, RecursiveMode::NonRecursive)
        .context("Failed to watch main configuration file")?;
    info!("Watching main config file: {:?}", config_path);

    // Watch all config folders (non-recursively)
    for folder in config_folders.iter() {
        match watcher.watch(folder, RecursiveMode::NonRecursive) {
            Ok(_) => info!("Watching config folder: {:?}", folder),
            Err(e) => warn!("Failed to watch config folder {:?}: {}", folder, e),
        }
    }

    // Keep watcher alive
    let _watcher = Arc::new(Mutex::new(watcher));

    // Process events
    while let Some(event) = rx.recv().await {
        if should_reload_config(&event) {
            info!("Configuration file changed, reloading...");

            // Use load_multi if config_folders are specified, otherwise load single file
            let reload_result = if config_folders.is_empty() {
                AppConfig::load(&config_path)
            } else {
                AppConfig::load_multi(&config_path, &config_folders)
            };

            match reload_result {
                Ok(new_config) => {
                    info!("Configuration reloaded successfully");

                    // Clear any previous validation issues on success
                    store.status.clear_validation_issues().await;

                    // Update the store
                    let mut config_write = store.config.write().await;
                    *config_write = new_config.clone();
                    drop(config_write);

                    // Re-initialize switch status with new configuration
                    store.status.initialize_switches(&new_config.switches).await;

                    // Apply configuration to all switches (with conflict detection)
                    if let Err(e) = apply_all_configurations(&new_config, &store.status).await {
                        error!("Failed to apply configurations: {}", e);
                    }
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    error!("Failed to reload configuration: {}", error_msg);

                    // Try to extract affected switch ID from error message
                    let affected_switch = extract_affected_switch(&error_msg);

                    // Determine error type
                    let error_type = if error_msg.contains("missing required fields") {
                        "MergeValidation"
                    } else if error_msg.contains("merge conflict") {
                        "MergeConflict"
                    } else if error_msg.contains("Failed to parse") {
                        "ParseError"
                    } else {
                        "ConfigLoad"
                    };

                    // Store error in status tracker for /api/status visibility
                    store.status.set_validation_issues(vec![error_msg.clone()]).await;

                    // Set structured error info
                    store.status.set_last_config_error(
                        error_type.to_string(),
                        error_msg.clone(),
                        affected_switch.clone(),
                    ).await;

                    // Also record as an error event
                    store.status.record_error(
                        "ConfigReload".to_string(),
                        error_msg,
                        affected_switch,
                        "config_reload".to_string(),
                    ).await;
                }
            }
        }
    }

    Ok(())
}

fn should_reload_config(event: &Event) -> bool {
    use notify::EventKind;

    matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    )
}

// TODO: Add support for parallel switch configuration (apply to multiple switches simultaneously)
// Currently switches are configured sequentially to simplify conflict detection and error handling.
// Parallel configuration would improve performance when managing many switches.

async fn apply_all_configurations(config: &AppConfig, status: &StatusTracker) -> Result<()> {
    info!("Applying configuration to {} switches", config.switches.len());

    let mut success_count = 0;
    let mut failure_count = 0;

    for switch_config in &config.switches {
        let switch_id = &switch_config.id;

        // Check if this switch is already being configured (e.g., by API)
        if status.is_currently_configuring(switch_id).await {
            // Queue this switch for later instead of skipping
            info!(
                "Switch '{}' ({}) is busy, queuing for later",
                switch_id, switch_config.hostname()
            );
            status.queue_pending_reload(switch_id.clone()).await;
            continue;
        }

        // Apply configuration to this switch (handles pending reloads internally)
        match apply_switch_with_pending(switch_config, status).await {
            Ok(()) => success_count += 1,
            Err(_) => failure_count += 1,
        }
    }

    // Log summary (consistent with main startup flow)
    info!("");
    info!("========================================");
    info!("Configuration Summary");
    info!("========================================");
    info!("Successful: {}", success_count);
    info!("Failed: {}", failure_count);
    info!("========================================");

    if failure_count > 0 {
        info!("✗ Configuration reload completed with errors");
    } else {
        info!("✓ Configuration reload completed successfully");
    }

    Ok(())
}

/// Apply configuration to a switch, then check for and process any pending reload
async fn apply_switch_with_pending(
    switch_config: &crate::models::SwitchConfig,
    status: &StatusTracker,
) -> Result<()> {
    let switch_id = &switch_config.id;

    // Mark switch as being configured
    status.set_currently_configuring(switch_id.clone()).await;

    info!("Configuring switch: {} ({})", switch_id, switch_config.hostname());

    let result = apply_single_switch(switch_config).await;

    match &result {
        Ok(warnings) => {
            // Always update warnings — clears stale warnings when issues resolve
            if !warnings.is_empty() {
                for w in warnings {
                    warn!("Switch {}: {}", switch_id, w);
                }
            }
            status.record_warnings(switch_id, warnings.clone()).await;
            status.record_apply_success(switch_id).await;
        }
        Err(e) => {
            error!(
                "Failed to configure switch '{}' ({}): {}",
                switch_id, switch_config.hostname(), e
            );
            status.record_apply_failure(switch_id, &e.to_string()).await;
        }
    }

    // Check if there's a pending reload queued while we were applying
    // If so, apply again immediately (the pending reload replaces any previous one)
    if status.take_pending_reload(switch_id).await {
        info!(
            "Processing pending config reload for switch '{}' ({})",
            switch_id, switch_config.hostname()
        );

        let pending_result = apply_single_switch(switch_config).await;

        match &pending_result {
            Ok(warnings) => {
                if !warnings.is_empty() {
                    for w in warnings {
                        warn!("Switch {}: {}", switch_id, w);
                    }
                }
                status.record_warnings(switch_id, warnings.clone()).await;
                status.record_apply_success(switch_id).await;
            }
            Err(e) => {
                error!(
                    "Failed to apply pending config for switch '{}' ({}): {}",
                    switch_id, switch_config.hostname(), e
                );
                status.record_apply_failure(switch_id, &e.to_string()).await;
            }
        }
    }

    // Always clear the configuring status when done
    status.clear_currently_configuring(switch_id).await;

    // Return result (map Ok(Vec<String>) to Ok(()))
    result.map(|_| ()).map_err(|e| anyhow::anyhow!("{}", e))
}

/// Apply configuration to a single switch. Returns warnings on success.
async fn apply_single_switch(switch_config: &crate::models::SwitchConfig) -> Result<Vec<String>> {
    let mut vendor = vendors::create_vendor_with_runtime(
        switch_config,
        &crate::config::RuntimeConfig::default(),
        switch_config.settings.enforce_port_config
    )
        .with_context(|| format!("Failed to create vendor for {}", switch_config.hostname()))?;

    // Connect
    if let Err(e) = vendor.connect().await {
        anyhow::bail!("Failed to connect: {}", e);
    }

    // Apply configuration
    match vendor.apply_configuration().await {
        Ok(results) => {
            info!(
                "Successfully applied configuration to {}: {:?}",
                switch_config.hostname(), results
            );

            // Save configuration
            if let Err(e) = vendor.save_configuration().await {
                warn!(
                    "Failed to save configuration on {}: {}",
                    switch_config.hostname(), e
                );
            }
        }
        Err(e) => {
            // Disconnect before returning error
            let _ = vendor.disconnect().await;
            anyhow::bail!("Failed to apply configuration: {}", e);
        }
    }

    // Collect warnings before disconnecting
    let warnings = vendor.get_warnings();

    // Disconnect
    if let Err(e) = vendor.disconnect().await {
        warn!(
            "Failed to disconnect from {}: {}",
            switch_config.hostname(), e
        );
    }

    Ok(warnings)
}

/// Extract affected switch ID from an error message
/// Looks for patterns like "Switch 'ID'" in the error text
fn extract_affected_switch(error_msg: &str) -> Option<String> {
    // Look for pattern: Switch 'ID' or switch 'ID'
    if let Some(start) = error_msg.find("Switch '").or_else(|| error_msg.find("switch '")) {
        let after_quote = start + 8; // Length of "Switch '"
        if let Some(end) = error_msg[after_quote..].find('\'') {
            return Some(error_msg[after_quote..after_quote + end].to_string());
        }
    }
    None
}
