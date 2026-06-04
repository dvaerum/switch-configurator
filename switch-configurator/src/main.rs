mod api;
mod config;
mod diff;
mod models;
mod ssh;
mod status;
mod validation;
mod vendors;
mod watcher;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::{info, warn, Level};
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Configuration file path
    #[arg(long, default_value = "config.yaml")]
    config_file: PathBuf,

    /// Configuration folders to scan for additional YAML files (can be specified multiple times)
    #[arg(long)]
    config_folder: Vec<PathBuf>,

    /// API server port (ignored in one-off mode)
    #[arg(short, long, default_value = "4002")]
    port: u16,

    /// Enable file watching mode (ignored in one-off mode)
    #[arg(short, long, default_value = "true")]
    watch: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: Level,

    /// Run once and exit (one-off mode, no service/API)
    #[arg(long, default_value = "false")]
    one_off: bool,

    /// Debug mode: prompt for confirmation before each command
    #[arg(long, default_value = "false")]
    debug: bool,

    /// Dry-run mode: show what would be done without applying changes
    #[arg(long, default_value = "false")]
    dry_run: bool,

    /// Target specific switch by hostname (optional, applies to all if not specified)
    #[arg(long)]
    switch: Option<String>,

    /// Apply configuration on startup (before starting API server in service mode)
    #[arg(long, default_value = "false")]
    apply_on_startup: bool,

    /// Show merged configuration and exit (for debugging multi-config merges)
    #[arg(long, default_value = "false")]
    show_merged_config: bool,

    /// Show detailed merge trace and exit (audit trail of merge decisions)
    #[arg(long, default_value = "false")]
    show_merge_trace: bool,

    /// Unix socket path for the API (in addition to TCP)
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Strict deployment: fail if any switch config has validation errors
    #[arg(long, default_value = "false")]
    strict_deployment: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .with_target(false)
        .init();

    info!("Starting switch-configurator v{}", env!("CARGO_PKG_VERSION"));
    info!("Configuration file: {:?}", args.config_file);
    if !args.config_folder.is_empty() {
        info!("Configuration folders: {:?}", args.config_folder);
    }

    // Load initial configuration (single or multi-config mode)
    let (app_config, validation_failures) = if args.config_folder.is_empty() {
        config::AppConfig::load(&args.config_file)?
    } else {
        config::AppConfig::load_multi(&args.config_file, &args.config_folder)?
    };

    if !validation_failures.is_empty() {
        for f in &validation_failures {
            warn!("Switch '{}' ({}) skipped due to validation failure: {}",
                f.switch_id, f.hostname.as_deref().unwrap_or("unknown"), f.error);
        }
        if args.strict_deployment {
            anyhow::bail!(
                "Strict deployment: {} switch(es) failed validation. Fix all configs or disable strict mode.",
                validation_failures.len()
            );
        }
    }
    info!("Loaded configuration for {} switches ({} skipped due to validation failures)",
        app_config.switches.len(), validation_failures.len());

    // Handle debug output flags
    if args.show_merged_config {
        info!("========================================");
        info!("Merged Configuration Output");
        info!("========================================");
        let yaml = serde_yaml::to_string(&app_config)?;
        println!("{}", yaml);
        return Ok(());
    }

    if args.show_merge_trace {
        info!("========================================");
        info!("Merge Trace Output");
        info!("========================================");
        warn!("--show-merge-trace is not yet fully implemented");
        warn!("Use --log-level debug to see detailed merge information");
        // TODO: Implement detailed merge trace with audit trail
        return Ok(());
    }

    // Create runtime configuration
    let runtime_config = config::RuntimeConfig {
        debug: args.debug,
        dry_run: args.dry_run,
        one_off: args.one_off,
        target_switch: args.switch.clone(),
    };

    // Log mode information
    if runtime_config.one_off {
        info!("Running in ONE-OFF mode (will exit after applying configuration)");
    } else {
        info!("Running in SERVICE mode");
        info!("API port: {}", args.port);
        info!("File watching: {}", args.watch);
    }

    if runtime_config.debug {
        info!("DEBUG mode enabled: will prompt before executing each command");
    }

    if runtime_config.dry_run {
        info!("DRY-RUN mode enabled: will show changes without applying");
    }

    if let Some(ref switch) = runtime_config.target_switch {
        info!("Targeting specific switch: {}", switch);
    }

    // One-off mode: apply configuration and exit
    if runtime_config.one_off {
        return run_one_off(app_config, runtime_config).await;
    }

    // Service mode: apply configuration on startup if requested
    if args.apply_on_startup {
        info!("Applying configuration on startup...");
        info!("========================================");

        // Apply configuration once using the same logic as one-off mode
        match apply_configuration_once(&app_config, &runtime_config).await {
            Ok(summary) => {
                info!("✓ Startup configuration applied successfully");
                info!("  Successful: {}, Failed: {}", summary.0, summary.1);
            }
            Err(e) => {
                warn!("Failed to apply configuration on startup: {}", e);
                warn!("Continuing to start service...");
            }
        }

        info!("========================================");
        info!("");
    }

    // Service mode: start API and file watcher
    // Create shared ConfigStore for both API and watcher
    let store = config::ConfigStore::new(app_config.clone(), args.port);

    // Store validation failures for dashboard display
    if !validation_failures.is_empty() {
        *store.validation_failures.write().await = validation_failures;
    }

    // Initialize switch status tracking
    store.status.initialize_switches(&app_config.switches).await;

    // Set config metadata for reload endpoints
    store.status.set_config_metadata(status::ConfigMetadata {
        config_file: args.config_file.clone(),
        config_folders: args.config_folder.clone(),
        last_loaded: chrono::Utc::now(),
        switches_count: app_config.switches.len(),
    }).await;

    // Start API server
    let api_handle = tokio::spawn(api::server::start(store.clone(), args.socket.clone()));

    // Start file watcher if enabled
    let watcher_handle = if args.watch {
        Some(tokio::spawn(watcher::start(
            args.config_file.clone(),
            args.config_folder.clone(),
            store.clone(),
        )))
    } else {
        None
    };

    // Wait for both tasks
    tokio::select! {
        result = api_handle => {
            result??;
        }
        result = async {
            match watcher_handle {
                Some(handle) => handle.await,
                None => std::future::pending().await,
            }
        } => {
            result??;
        }
    }

    Ok(())
}

/// Apply configuration once and return summary (success_count, failure_count)
async fn apply_configuration_once(
    app_config: &config::AppConfig,
    runtime_config: &config::RuntimeConfig,
) -> Result<(usize, usize)> {
    use crate::vendors::create_vendor_with_runtime;
    use tracing::{error, warn};

    info!("========================================");
    info!("Starting configuration application");
    info!("========================================");

    let switches_to_configure: Vec<_> = if let Some(ref target) = runtime_config.target_switch {
        app_config
            .switches
            .iter()
            .filter(|s| s.hostname.as_ref().map(|h| h.as_str()) == Some(target.as_str()))
            .collect()
    } else {
        app_config.switches.iter().collect()
    };

    if switches_to_configure.is_empty() {
        if let Some(ref target) = runtime_config.target_switch {
            error!("No switch found with hostname: {}", target);
            anyhow::bail!("Target switch not found");
        } else {
            warn!("No switches configured");
            return Ok((0, 0));
        }
    }

    info!(
        "Will configure {} switch(es)",
        switches_to_configure.len()
    );

    let mut success_count = 0;
    let mut failure_count = 0;

    for switch_config in switches_to_configure {
        info!("");
        info!("========================================");
        info!("Configuring: {}", switch_config.hostname());
        info!("Model: {:?}", switch_config.model());
        info!("Management IP: {}", switch_config.management_ip());
        info!("========================================");

        let mut vendor = match create_vendor_with_runtime(switch_config, &runtime_config, switch_config.settings.enforce_port_config) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to create vendor implementation: {}", e);
                failure_count += 1;
                continue;
            }
        };

        // Connect
        info!("Connecting to {}...", switch_config.hostname());
        if let Err(e) = vendor.connect().await {
            error!("Failed to connect: {}", e);
            failure_count += 1;
            continue;
        }
        info!("✓ Connected successfully");

        // Apply configuration
        match vendor.apply_configuration().await {
            Ok(results) => {
                if results.is_empty() {
                    info!("✓ No changes needed - switch already in desired state");
                    success_count += 1;
                } else {
                    info!("✓ Applied {} configuration change(s)", results.len());
                    for result in results {
                        info!("  - {}", result.message);
                    }

                    // Run validation tests if configured
                    let validation_passed = if let Some(validation_config) = &switch_config.validation {
                        if validation_config.enabled && !runtime_config.dry_run {
                            info!("Running validation tests...");
                            match vendor.run_validation_tests(validation_config).await {
                                Ok(validation_result) => {
                                    if validation_result.passed {
                                        info!("✓ Validation passed: {}/{} tests successful",
                                            validation_result.tests_passed,
                                            validation_result.tests_run);
                                        true
                                    } else {
                                        warn!("✗ Validation failed: {}/{} tests failed",
                                            validation_result.tests_failed,
                                            validation_result.tests_run);

                                        for failure in &validation_result.failures {
                                            warn!("  - {}: {}", failure.test_name, failure.error);
                                        }

                                        // Handle validation failure based on configuration
                                        match validation_config.on_failure {
                                            validation::FailureAction::Rollback => {
                                                warn!("Rolling back configuration...");
                                                if let Err(e) = vendor.rollback_configuration(validation_config.rollback_method).await {
                                                    error!("Failed to rollback configuration: {}", e);
                                                } else {
                                                    info!("✓ Configuration rolled back");
                                                }
                                                false
                                            }
                                            validation::FailureAction::SaveAnyway => {
                                                warn!("Validation failed but saving configuration anyway (as configured)");
                                                true
                                            }
                                            validation::FailureAction::Manual => {
                                                warn!("Validation failed - manual intervention required");
                                                warn!("Configuration NOT saved - running config differs from startup config");
                                                false
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to run validation tests: {}", e);
                                    false
                                }
                            }
                        } else {
                            // Validation disabled or dry-run mode
                            true
                        }
                    } else {
                        // No validation configured
                        true
                    };

                    // Save configuration only if validation passed (or no validation configured)
                    if validation_passed && !runtime_config.dry_run {
                        info!("Saving configuration...");
                        if let Err(e) = vendor.save_configuration().await {
                            warn!("Failed to save configuration: {}", e);
                            failure_count += 1;
                        } else {
                            info!("✓ Configuration saved");
                            success_count += 1;
                        }
                    } else if !validation_passed {
                        failure_count += 1;
                    } else {
                        // Dry-run mode
                        success_count += 1;
                    }
                }
            }
            Err(e) => {
                error!("Failed to apply configuration: {}", e);
                failure_count += 1;
            }
        }

        // Disconnect
        if let Err(e) = vendor.disconnect().await {
            warn!("Failed to disconnect cleanly: {}", e);
        } else {
            info!("✓ Disconnected");
        }
    }

    info!("");
    info!("========================================");
    info!("Configuration Summary");
    info!("========================================");
    info!("Successful: {}", success_count);
    info!("Failed: {}", failure_count);
    info!("========================================");

    Ok((success_count, failure_count))
}

/// Run in one-off mode: apply configuration once and exit
async fn run_one_off(
    app_config: config::AppConfig,
    runtime_config: config::RuntimeConfig,
) -> Result<()> {
    let (_success_count, failure_count) = apply_configuration_once(&app_config, &runtime_config).await?;

    if failure_count > 0 {
        anyhow::bail!("{} switch(es) failed configuration", failure_count);
    }

    Ok(())
}
