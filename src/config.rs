pub mod errors;

use crate::models::{SwitchConfig, SwitchModel};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub switches: Vec<SwitchConfig>,
}

/// Configuration file with optional merge priority (for YAML parsing)
/// The merge_priority is extracted and used in ConfigWithMetadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfigFile {
    /// Optional merge priority (0 = highest, 9999 = lowest)
    /// Defaults: 50 for main config, 100 for folder configs
    #[serde(default)]
    pub merge_priority: Option<u16>,

    /// The actual configuration
    #[serde(flatten)]
    pub config: AppConfig,
}

/// Configuration source type (main config or folder config)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceType {
    MainConfig,
    FolderConfig,
}

impl ConfigSourceType {
    /// Get the default priority for this source type
    pub fn default_priority(&self) -> u16 {
        match self {
            ConfigSourceType::MainConfig => 50,
            ConfigSourceType::FolderConfig => 100,
        }
    }
}

/// Configuration with metadata for multi-config merging
#[derive(Debug, Clone)]
pub struct ConfigWithMetadata {
    /// The parsed configuration
    pub config: AppConfig,

    /// Merge priority (0 = highest, 9999 = lowest)
    /// Lower number = higher priority
    pub merge_priority: u16,

    /// Source file path for debugging/error messages
    pub source_file: PathBuf,

    /// Is this from main config or folder config?
    pub source_type: ConfigSourceType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_timeout")]
    pub ssh_timeout_secs: u64,
    #[serde(default = "default_retry")]
    pub max_retries: u32,
    /// When true, ports not in config will be reset to default state (disabled, VLAN 1)
    #[serde(default = "default_false")]
    pub enforce_port_config: bool,
}

fn default_timeout() -> u64 {
    30
}

fn default_retry() -> u32 {
    3
}

fn default_false() -> bool {
    false
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ssh_timeout_secs: default_timeout(),
            max_retries: default_retry(),
            enforce_port_config: default_false(),
        }
    }
}

impl AppConfig {
    /// Load configuration from a YAML file (legacy single-file mode)
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let mut config: AppConfig = {
            let deserializer = serde_yaml::Deserializer::from_str(&content);
            serde_path_to_error::deserialize(deserializer)
                .map_err(|err| {
                    let field_path = err.path().to_string();
                    let error_msg = err.into_inner().to_string();
                    let enhanced_msg = errors::enhance_parse_error(&field_path, &error_msg);
                    anyhow!(
                        "Failed to parse config file: {:?}\n\
                         Field path: {}\n\
                         Error: {}",
                        path,
                        field_path,
                        enhanced_msg
                    )
                })?
        };

        // Validate all switch configurations using shared validation function
        for switch in &mut config.switches {
            validate_switch_config(switch)?;
        }

        Ok(config)
    }

    /// Load configuration with metadata from a YAML file
    ///
    /// When used for multi-config merging, VLAN reference validation is skipped
    /// and performed after merging to allow splitting VLANs and ports into separate files.
    pub fn load_with_metadata(path: &Path, source_type: ConfigSourceType) -> Result<ConfigWithMetadata> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        // Parse as AppConfigFile to extract merge_priority
        let config_file: AppConfigFile = {
            let deserializer = serde_yaml::Deserializer::from_str(&content);
            serde_path_to_error::deserialize(deserializer)
                .map_err(|err| {
                    let field_path = err.path().to_string();
                    let error_msg = err.into_inner().to_string();
                    let enhanced_msg = errors::enhance_parse_error(&field_path, &error_msg);
                    anyhow!(
                        "Failed to parse config file: {:?}\n\
                         Field path: {}\n\
                         Error: {}",
                        path,
                        field_path,
                        enhanced_msg
                    )
                })?
        };

        let mut config = config_file.config;

        // Expand port ranges for all switches
        for switch in &mut config.switches {
            expand_port_ranges(switch)?;
        }

        // Validate all switch configurations (but NOT required fields or VLAN references yet)
        // Required field and VLAN reference validation is deferred to after merging in multi-config mode
        for switch in &mut config.switches {
            validator::Validate::validate(switch)
                .with_context(|| format!("Invalid configuration for switch id: {}", switch.id))?;
        }

        // Determine merge priority
        let merge_priority = config_file.merge_priority.unwrap_or_else(|| source_type.default_priority());

        // Validate priority based on source type
        if source_type == ConfigSourceType::FolderConfig && merge_priority < 11 {
            return Err(anyhow!(
                "Folder config {:?} has priority {} but folder configs must use priority 11-9999 (0-10 reserved for main config)",
                path,
                merge_priority
            ));
        }

        Ok(ConfigWithMetadata {
            config,
            merge_priority,
            source_file: path.to_path_buf(),
            source_type,
        })
    }

    /// Load and merge multiple configuration sources
    ///
    /// # Arguments
    /// * `main_config_path` - Path to the main configuration file
    /// * `folder_paths` - Paths to folders containing additional YAML files
    ///
    /// # Returns
    /// Merged configuration with all sources combined according to priority
    pub fn load_multi(main_config_path: &Path, folder_paths: &[PathBuf]) -> Result<Self> {
        use tracing::info;

        let mut configs: Vec<ConfigWithMetadata> = Vec::new();

        // Load main config
        info!("Loading main config from: {:?}", main_config_path);
        let main_config = Self::load_with_metadata(main_config_path, ConfigSourceType::MainConfig)?;
        info!("Main config priority: {}", main_config.merge_priority);
        configs.push(main_config);

        // Load folder configs
        for folder_path in folder_paths {
            info!("Scanning folder for configs: {:?}", folder_path);
            let folder_configs = scan_config_folder(folder_path)?;
            info!("Found {} config files in folder", folder_configs.len());
            configs.extend(folder_configs);
        }

        // Sort configs by priority (lower number = higher priority = processed first)
        configs.sort_by_key(|c| c.merge_priority);

        info!("Loaded {} total config sources", configs.len());
        for config in &configs {
            info!(
                "  - {:?} (priority: {}, source: {:?})",
                config.source_file.file_name().unwrap_or_default(),
                config.merge_priority,
                config.source_type
            );
        }

        // Merge all configs according to priority
        merge_configs(configs)
    }

    /// Save configuration to a YAML file
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_yaml::to_string(self)
            .context("Failed to serialize configuration")?;

        std::fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {:?}", path))?;

        Ok(())
    }
}

/// Thread-safe configuration store
/// Shared configuration store with status tracking
#[derive(Clone)]
pub struct ConfigStore {
    pub config: Arc<RwLock<AppConfig>>,
    pub status: crate::status::StatusTracker,
    pub api_port: u16,
}

impl ConfigStore {
    pub fn new(config: AppConfig, api_port: u16) -> Self {
        let status = crate::status::StatusTracker::new();
        Self {
            config: Arc::new(RwLock::new(config)),
            status,
            api_port,
        }
    }
}

/// Legacy type alias for backward compatibility
pub type LegacyConfigStore = Arc<RwLock<AppConfig>>;

pub fn create_store(config: AppConfig) -> LegacyConfigStore {
    Arc::new(RwLock::new(config))
}

/// Runtime execution mode configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Debug mode: prompt before executing each command
    pub debug: bool,
    /// Dry-run mode: show what would be done without executing
    pub dry_run: bool,
    /// One-off mode: run once and exit (vs service mode)
    pub one_off: bool,
    /// Target specific switch hostname (None = all switches)
    pub target_switch: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            debug: false,
            dry_run: false,
            one_off: false,
            target_switch: None,
        }
    }
}

/// Scan a folder for YAML configuration files and load them with metadata
///
/// Files are loaded in alphabetical order by filename.
/// Only files with .yaml or .yml extensions are loaded.
fn scan_config_folder(folder_path: &Path) -> Result<Vec<ConfigWithMetadata>> {
    use tracing::{info, warn};

    if !folder_path.exists() {
        return Err(anyhow!("Config folder does not exist: {:?}", folder_path));
    }

    if !folder_path.is_dir() {
        return Err(anyhow!("Config folder path is not a directory: {:?}", folder_path));
    }

    let mut configs = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(folder_path)
        .with_context(|| format!("Failed to read config folder: {:?}", folder_path))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            if let Ok(file_type) = entry.file_type() {
                file_type.is_file()
            } else {
                false
            }
        })
        .filter(|entry| {
            let path = entry.path();
            path.extension().map(|ext| ext == "yaml" || ext == "yml").unwrap_or(false)
        })
        .collect();

    // Sort alphabetically by filename
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        info!("  Loading folder config: {:?}", path.file_name().unwrap_or_default());

        match AppConfig::load_with_metadata(&path, ConfigSourceType::FolderConfig) {
            Ok(config) => {
                info!("    Priority: {}", config.merge_priority);
                configs.push(config);
            }
            Err(e) => {
                warn!("  Failed to load config file {:?}: {}", path, e);
                return Err(e.context(format!("Failed to load folder config: {:?}", path)));
            }
        }
    }

    Ok(configs)
}

/// Merge conflict information
#[derive(Debug, Clone)]
pub struct MergeConflict {
    pub switch_id: String,
    pub component_type: String,
    pub component_id: String,
    pub conflicting_sources: Vec<(PathBuf, u16)>, // (source_file, priority)
    pub description: String,
}

/// Merge multiple configurations according to priority
///
/// # Merge Strategy:
/// - Lower priority number = higher priority (0 is highest)
/// - Configs are processed in priority order (highest priority first)
/// - Component replacement strategy (not field-level merging):
///   - VLANs: Replace entire VLAN by id
///   - Ports: Replace entire port (after expanding ranges)
///   - Port Mirrors: Replace entire mirror by session_id
///   - SNMP: Sub-component lists (communities, trap_receivers, enabled_traps) are replaced
///   - Validation/Settings/Credentials: Replace entire object
///
/// # Conflict Detection:
/// - Identity fields (hostname, management_ip, model) must match across all configs for same switch
/// - Multiple definitions of same component with same priority = conflict
fn merge_configs(configs: Vec<ConfigWithMetadata>) -> Result<AppConfig> {
    use std::collections::{HashMap, BTreeMap};
    use tracing::{info, warn};

    if configs.is_empty() {
        return Err(anyhow!("No configurations to merge"));
    }

    // Group configs by switch ID and track sources for better error messages
    let mut switches_by_id: HashMap<String, Vec<ConfigWithMetadata>> = HashMap::new();
    let mut merge_trackers: HashMap<String, errors::SwitchMergeTracker> = HashMap::new();

    for config in &configs {
        for switch in &config.config.switches {
            // Track source for this switch
            let tracker = merge_trackers
                .entry(switch.id.clone())
                .or_insert_with(|| errors::SwitchMergeTracker::new(switch.id.clone()));
            tracker.add_source(
                config.source_file.clone(),
                config.merge_priority,
                switch,
            );

            // Group config for merging
            switches_by_id
                .entry(switch.id.clone())
                .or_insert_with(Vec::new)
                .push(ConfigWithMetadata {
                    config: AppConfig {
                        switches: vec![switch.clone()],
                    },
                    merge_priority: config.merge_priority,
                    source_file: config.source_file.clone(),
                    source_type: config.source_type,
                });
        }
    }

    info!("Merging {} unique switches", switches_by_id.len());

    let mut merged_switches = Vec::new();
    let mut all_conflicts = Vec::new();

    for (switch_id, switch_configs) in switches_by_id {
        info!("Merging switch: {}", switch_id);

        // Validate identity fields first
        match validate_switch_identity(&switch_id, &switch_configs) {
            Ok(conflicts) => {
                if !conflicts.is_empty() {
                    all_conflicts.extend(conflicts);
                    continue; // Skip this switch, will report conflicts at the end
                }
            }
            Err(e) => {
                return Err(e.context(format!("Failed to validate identity for switch {}", switch_id)));
            }
        }

        // Merge this switch's configs
        match merge_single_switch(switch_id.clone(), switch_configs) {
            Ok(merged_switch) => {
                merged_switches.push(merged_switch);
            }
            Err(e) => {
                return Err(e.context(format!("Failed to merge switch {}", switch_id)));
            }
        }
    }

    // Report all conflicts if any exist
    if !all_conflicts.is_empty() {
        let mut error_msg = format!("Found {} merge conflicts:\n", all_conflicts.len());
        for conflict in &all_conflicts {
            error_msg.push_str(&format!(
                "\n  Switch '{}' - {}.{}: {}\n",
                conflict.switch_id,
                conflict.component_type,
                conflict.component_id,
                conflict.description
            ));
            error_msg.push_str("    Conflicting sources:\n");
            for (source, priority) in &conflict.conflicting_sources {
                error_msg.push_str(&format!(
                    "      - {:?} (priority: {})\n",
                    source.file_name().unwrap_or_default(),
                    priority
                ));
            }
        }
        return Err(anyhow!(error_msg));
    }

    // Post-merge validation: VLAN references and required fields
    // This allows splitting VLANs and ports into separate files while ensuring final config is valid
    for switch in &mut merged_switches {
        // Validate that all required identity fields are present after merge
        // Use single-line error message for logging (detailed version available via API)
        let missing_fields = get_missing_required_fields(switch);
        if !missing_fields.is_empty() {
            let tracker = merge_trackers.get(&switch.id);
            let error = if let Some(tracker) = tracker {
                tracker.to_validation_error(missing_fields)
            } else {
                errors::SwitchValidationError {
                    switch_id: switch.id.clone(),
                    missing_fields,
                    contributing_sources: vec![],
                }
            };
            return Err(anyhow!(error.format_log_message()));
        }

        // Validate VLAN references (ports must reference defined VLANs)
        validate_vlan_references(switch)
            .with_context(|| format!("Post-merge VLAN reference validation failed for switch '{}'", switch.hostname.as_ref().unwrap_or(&switch.id)))?;

        // Validate that switch has at least one VLAN defined
        validate_has_vlans(switch)
            .with_context(|| format!("Post-merge validation failed for switch '{}'", switch.hostname.as_ref().unwrap_or(&switch.id)))?;
    }

    Ok(AppConfig {
        switches: merged_switches,
    })
}

/// Get list of missing required fields for a switch
fn get_missing_required_fields(switch: &SwitchConfig) -> Vec<String> {
    let mut missing = Vec::new();
    if switch.hostname.is_none() {
        missing.push("hostname".to_string());
    }
    if switch.model.is_none() {
        missing.push("model".to_string());
    }
    if switch.management_ip.is_none() {
        missing.push("management_ip".to_string());
    }
    if switch.credentials.is_none() {
        missing.push("credentials".to_string());
    }
    missing
}

/// Validate that identity fields match across all configs for a switch
/// Only validates if a field is present in multiple configs
/// Fields only need to be in ONE config (typically the main config)
fn validate_switch_identity(
    switch_id: &str,
    configs: &[ConfigWithMetadata],
) -> Result<Vec<MergeConflict>> {
    let mut conflicts = Vec::new();

    if configs.is_empty() {
        return Ok(conflicts);
    }

    // Find the first config with each identity field (for reference)
    let mut ref_hostname: Option<(&String, &PathBuf, u16)> = None;
    let mut ref_ip: Option<(&String, &PathBuf, u16)> = None;
    let mut ref_model: Option<(&SwitchModel, &PathBuf, u16)> = None;

    for config in configs {
        let switch = &config.config.switches[0];

        if let Some(hostname) = &switch.hostname {
            if ref_hostname.is_none() {
                ref_hostname = Some((hostname, &config.source_file, config.merge_priority));
            }
        }

        if let Some(ip) = &switch.management_ip {
            if ref_ip.is_none() {
                ref_ip = Some((ip, &config.source_file, config.merge_priority));
            }
        }

        if let Some(model) = &switch.model {
            if ref_model.is_none() {
                ref_model = Some((model, &config.source_file, config.merge_priority));
            }
        }
    }

    // Check all configs for conflicts (only if field exists in multiple places)
    for config in configs {
        let switch = &config.config.switches[0];

        // Check hostname conflicts
        if let Some(hostname) = &switch.hostname {
            if let Some((ref_host, ref_source, ref_priority)) = ref_hostname {
                if hostname != ref_host {
                    conflicts.push(MergeConflict {
                        switch_id: switch_id.to_string(),
                        component_type: "identity".to_string(),
                        component_id: "hostname".to_string(),
                        conflicting_sources: vec![
                            (ref_source.clone(), ref_priority),
                            (config.source_file.clone(), config.merge_priority),
                        ],
                        description: format!(
                            "Hostname mismatch: '{}' vs '{}'",
                            ref_host, hostname
                        ),
                    });
                }
            }
        }

        // Check management_ip conflicts
        if let Some(ip) = &switch.management_ip {
            if let Some((ref_ip_val, ref_source, ref_priority)) = ref_ip {
                if ip != ref_ip_val {
                    conflicts.push(MergeConflict {
                        switch_id: switch_id.to_string(),
                        component_type: "identity".to_string(),
                        component_id: "management_ip".to_string(),
                        conflicting_sources: vec![
                            (ref_source.clone(), ref_priority),
                            (config.source_file.clone(), config.merge_priority),
                        ],
                        description: format!(
                            "Management IP mismatch: '{}' vs '{}'",
                            ref_ip_val, ip
                        ),
                    });
                }
            }
        }

        // Check model conflicts
        if let Some(model) = &switch.model {
            if let Some((ref_model_val, ref_source, ref_priority)) = ref_model {
                if model != ref_model_val {
                    conflicts.push(MergeConflict {
                        switch_id: switch_id.to_string(),
                        component_type: "identity".to_string(),
                        component_id: "model".to_string(),
                        conflicting_sources: vec![
                            (ref_source.clone(), ref_priority),
                            (config.source_file.clone(), config.merge_priority),
                        ],
                        description: format!(
                            "Model mismatch: '{:?}' vs '{:?}'",
                            ref_model_val, model
                        ),
                    });
                }
            }
        }
    }

    Ok(conflicts)
}

/// Merge all configs for a single switch
fn merge_single_switch(
    switch_id: String,
    mut configs: Vec<ConfigWithMetadata>,
) -> Result<SwitchConfig> {
    use std::collections::BTreeMap;

    // Sort by priority (lower number = higher priority)
    configs.sort_by_key(|c| c.merge_priority);

    // Initialize merged config with the switch ID
    let mut merged = SwitchConfig {
        id: switch_id.clone(),
        hostname: None,
        model: None,
        management_ip: None,
        credentials: None,
        vlans: Vec::new(),
        ports: Vec::new(),
        port_mirrors: Vec::new(),
        snmp: None,
        validation: None,
        vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        settings: crate::config::Settings::default(),
    };

    // Merge identity fields (take first non-None value, respecting priority)
    for config in &configs {
        let switch = &config.config.switches[0];

        if merged.hostname.is_none() && switch.hostname.is_some() {
            merged.hostname = switch.hostname.clone();
        }
        if merged.model.is_none() && switch.model.is_some() {
            merged.model = switch.model.clone();
        }
        if merged.management_ip.is_none() && switch.management_ip.is_some() {
            merged.management_ip = switch.management_ip.clone();
        }
        if merged.credentials.is_none() && switch.credentials.is_some() {
            merged.credentials = switch.credentials.clone();
        }
    }

    // Merge VLANs using BTreeMap for priority tracking
    let mut vlans_by_id: BTreeMap<u16, (crate::models::Vlan, u16)> = BTreeMap::new();

    for config in &configs {
        let switch = &config.config.switches[0];
        for vlan in &switch.vlans {
            vlans_by_id
                .entry(vlan.id)
                .or_insert((vlan.clone(), config.merge_priority));
        }
    }
    merged.vlans = vlans_by_id.into_values().map(|(vlan, _)| vlan).collect();

    // Merge Ports
    let mut ports_by_id: BTreeMap<String, (crate::models::Port, u16)> = BTreeMap::new();

    for config in &configs {
        let switch = &config.config.switches[0];
        for port in &switch.ports {
            ports_by_id
                .entry(port.port_id.clone())
                .or_insert((port.clone(), config.merge_priority));
        }
    }
    merged.ports = ports_by_id.into_values().map(|(port, _)| port).collect();

    // Merge Port Mirrors
    let mut mirrors_by_id: BTreeMap<String, (crate::models::PortMirror, u16)> = BTreeMap::new();

    for config in &configs {
        let switch = &config.config.switches[0];
        for mirror in &switch.port_mirrors {
            mirrors_by_id
                .entry(mirror.session_id.clone())
                .or_insert((mirror.clone(), config.merge_priority));
        }
    }
    merged.port_mirrors = mirrors_by_id.into_values().map(|(mirror, _)| mirror).collect();

    // Merge SNMP (sub-component replacement)
    merged.snmp = merge_snmp_configs(&configs)?;

    // Merge Validation (replace entire object, highest priority wins)
    for config in &configs {
        let switch = &config.config.switches[0];
        if switch.validation.is_some() {
            merged.validation = switch.validation.clone();
            break; // Highest priority wins
        }
    }

    // Merge Settings (replace entire object, highest priority wins)
    for config in &configs {
        let switch = &config.config.switches[0];
        // Settings always has a value (uses Default), so take from highest priority
        merged.settings = switch.settings.clone();
        break;
    }

    // Credentials already merged above as optional field

    Ok(merged)
}

/// Merge SNMP configurations with sub-component list replacement
fn merge_snmp_configs(configs: &[ConfigWithMetadata]) -> Result<Option<crate::models::SnmpConfig>> {
    use std::collections::BTreeMap;

    let mut merged_snmp: Option<crate::models::SnmpConfig> = None;
    let mut communities_priority: Option<u16> = None;
    let mut trap_receivers_priority: Option<u16> = None;
    let mut enabled_traps_priority: Option<u16> = None;

    for config in configs {
        let switch = &config.config.switches[0];

        if let Some(ref snmp) = switch.snmp {
            // Initialize merged_snmp if not yet created
            if merged_snmp.is_none() {
                merged_snmp = Some(crate::models::SnmpConfig {
                    communities: Vec::new(),
                    trap_receivers: Vec::new(),
                    enabled_traps: Vec::new(),
                });
            }

            let merged = merged_snmp.as_mut().unwrap();

            // Merge communities list (replace if higher priority)
            if !snmp.communities.is_empty() {
                if communities_priority.is_none() || config.merge_priority < communities_priority.unwrap() {
                    merged.communities = snmp.communities.clone();
                    communities_priority = Some(config.merge_priority);
                }
            }

            // Merge trap_receivers list (replace if higher priority)
            if !snmp.trap_receivers.is_empty() {
                if trap_receivers_priority.is_none() || config.merge_priority < trap_receivers_priority.unwrap() {
                    merged.trap_receivers = snmp.trap_receivers.clone();
                    trap_receivers_priority = Some(config.merge_priority);
                }
            }

            // Merge enabled_traps list (replace if higher priority)
            if !snmp.enabled_traps.is_empty() {
                if enabled_traps_priority.is_none() || config.merge_priority < enabled_traps_priority.unwrap() {
                    merged.enabled_traps = snmp.enabled_traps.clone();
                    enabled_traps_priority = Some(config.merge_priority);
                }
            }
        }
    }

    Ok(merged_snmp)
}

/// Expand port ranges in a switch configuration
/// Supports:
/// - Ranges: "1-5" expands to ["1", "2", "3", "4", "5"]
/// - Lists: "1,3,5" expands to ["1", "3", "5"]
/// - Mixed: "1-5,7,10-12" expands to ["1", "2", "3", "4", "5", "7", "10", "11", "12"]
/// - Vendor-specific: "GigabitEthernet1/0/1-5" expands correctly for Cisco
fn expand_port_ranges(switch: &mut SwitchConfig) -> Result<()> {
    let original_port_count = switch.ports.len();
    let mut expanded_ports = Vec::new();

    for port in &switch.ports {
        let port_ids = parse_port_id(&port.port_id)?;

        if port_ids.len() > 1 {
            tracing::debug!(
                "Expanding port_id '{}' to {} ports: {:?}",
                port.port_id,
                port_ids.len(),
                port_ids
            );
        }

        for port_id in port_ids {
            let mut new_port = port.clone();
            new_port.port_id = port_id;
            expanded_ports.push(new_port);
        }
    }

    // Also expand port ranges in port_mirrors
    let mut expanded_mirrors = Vec::new();
    for mirror in &switch.port_mirrors {
        let mut new_mirror = mirror.clone();

        // Expand source ports
        let mut expanded_sources = Vec::new();
        for source_port in &mirror.source_ports {
            expanded_sources.extend(parse_port_id(source_port)?);
        }
        new_mirror.source_ports = expanded_sources;

        // Expand destination port
        let dest_ports = parse_port_id(&mirror.destination_port)?;
        if dest_ports.len() > 1 {
            return Err(anyhow!(
                "Mirror session '{}': destination_port must be a single port, got: {}",
                mirror.session_id,
                mirror.destination_port
            ));
        }
        new_mirror.destination_port = dest_ports[0].clone();

        expanded_mirrors.push(new_mirror);
    }

    let expanded_port_count = expanded_ports.len();
    switch.ports = expanded_ports;
    switch.port_mirrors = expanded_mirrors;

    if expanded_port_count != original_port_count {
        tracing::info!(
            "Port expansion for switch '{}': {} config entries expanded to {} individual ports",
            switch.hostname.as_ref().unwrap_or(&switch.id),
            original_port_count,
            expanded_port_count
        );
    }

    Ok(())
}

/// Validate that all required identity fields are present (for single-file mode)
fn validate_required_fields(switch: &SwitchConfig) -> Result<()> {
    let missing_fields = get_missing_required_fields(switch);

    if !missing_fields.is_empty() {
        // For single-file mode, create a simple error without source tracking
        let error = errors::SwitchValidationError {
            switch_id: switch.id.clone(),
            missing_fields,
            contributing_sources: vec![], // No source tracking in single-file mode
        };
        return Err(anyhow!(error.format_log_message()));
    }

    Ok(())
}

/// Validate that switch has at least one VLAN defined after merge
fn validate_has_vlans(switch: &SwitchConfig) -> Result<()> {
    if switch.vlans.is_empty() {
        return Err(anyhow!(
            "Switch '{}' (id: {}) has no VLANs defined after merging all configuration sources. \
             At least one VLAN is required. Please ensure at least one config file provides VLANs for this switch.",
            switch.hostname.as_ref().unwrap_or(&switch.id),
            switch.id
        ));
    }
    Ok(())
}

/// Validate that all VLANs referenced in port configurations exist
/// Filters out non-existent VLANs from allowed_vlans lists and logs warnings
/// Also validates VLAN name lengths against switch model limits
fn validate_vlan_references(switch: &mut SwitchConfig) -> Result<()> {
    use std::collections::HashSet;
    use tracing::{debug, error, warn};

    // Note: These unwraps are safe because validate_required_fields() is called first
    let hostname = switch.hostname.as_ref().expect("hostname validated");
    let model = switch.model.as_ref().expect("model validated");

    debug!("Validating VLAN references for switch: {}", hostname);

    // Validate VLAN name lengths and characters
    let max_name_len = model.max_vlan_name_length();
    for vlan in &switch.vlans {
        if vlan.name.len() > max_name_len {
            error!(
                "Switch '{}' VLAN {}: Name '{}' exceeds maximum length of {} characters (actual: {}). \
                The switch will truncate or reject this name, causing constant reconfiguration diffs.",
                hostname, vlan.id, vlan.name, max_name_len, vlan.name.len()
            );
            return Err(anyhow!(
                "VLAN {} name '{}' is {} characters, but switch model {:?} only supports up to {} characters",
                vlan.id, vlan.name, vlan.name.len(), model, max_name_len
            ));
        }

        // Note: Aruba switches accept most printable ASCII characters in VLAN names.
        // Hardware testing confirmed these work: @ # / & ' ; ! ( ) : ` and more.
        // The only restriction we enforce is the range pattern below.

        // Check for number-hyphen-number patterns (e.g., "8-10") which Aruba interprets as ranges
        let range_pattern = regex::Regex::new(r"\d+-\d+").unwrap();
        if range_pattern.is_match(&vlan.name) {
            error!(
                "Switch '{}' VLAN {}: Name '{}' contains a number range pattern (e.g., '8-10') \
                which Aruba switches interpret as a port/VLAN range. Use underscores or spaces instead.",
                hostname, vlan.id, vlan.name
            );
            return Err(anyhow!(
                "VLAN {} name '{}' contains a number range pattern (e.g., '8-10'). \
                Aruba switches interpret this as a range. Use 'Access 8_10 port sw' or 'Access 8 10 port sw' instead.",
                vlan.id, vlan.name
            ));
        }
    }

    // Collect all defined VLAN IDs
    let defined_vlans: HashSet<u16> = switch.vlans.iter().map(|v| v.id).collect();
    debug!("Defined VLANs: {:?}", defined_vlans);

    let mut has_errors = false;

    // Check each port's VLAN references and speed_duplex compatibility
    for port in &mut switch.ports {
        debug!("Checking port {}: vlan={}, allowed_vlans={:?}, speed_duplex={:?}",
               port.port_id, port.vlan, port.allowed_vlans, port.speed_duplex);

        // Check native/untagged VLAN
        if !defined_vlans.contains(&port.vlan) {
            let msg = format!(
                "Port {} references non-existent VLAN {} as native/untagged VLAN",
                port.port_id, port.vlan
            );
            error!("{}", msg);
            has_errors = true;
        }

        // Check speed_duplex compatibility with switch model
        if !model.supports_speed(port.speed_duplex) {
            let msg = format!(
                "Port {} has unsupported speed_duplex setting {:?} for switch model {:?}. Supported speeds: {:?}",
                port.port_id, port.speed_duplex, model, model.supported_speeds()
            );
            error!("{}", msg);
            has_errors = true;
        }

        // Check and filter allowed VLANs (for trunk ports)
        let original_allowed = port.allowed_vlans.clone();
        let valid_vlans: Vec<u16> = original_allowed.iter()
            .filter(|&&vlan_id| defined_vlans.contains(&vlan_id))
            .copied()
            .collect();

        // Log warnings for filtered VLANs
        for &vlan_id in &original_allowed {
            if !defined_vlans.contains(&vlan_id) {
                warn!(
                    "Switch '{}' Port {}: Filtering out non-existent VLAN {} from allowed_vlans. \
                    This VLAN is not defined in the switch configuration and would cause the switch \
                    to reject the configuration. The valid VLANs {:?} will still be applied.",
                    hostname, port.port_id, vlan_id, valid_vlans
                );
            }
        }

        // Update the port's allowed_vlans to only include valid ones
        port.allowed_vlans = valid_vlans;
    }

    // Check port mirror source/destination ports exist in port list
    let defined_port_ids: HashSet<String> = switch.ports.iter().map(|p| p.port_id.clone()).collect();

    for mirror in &switch.port_mirrors {
        for source_port in &mirror.source_ports {
            if !defined_port_ids.contains(source_port) {
                warn!(
                    "Port mirror session {} references undefined source port {}",
                    mirror.session_id, source_port
                );
            }
        }

        if !defined_port_ids.contains(&mirror.destination_port) {
            warn!(
                "Port mirror session {} references undefined destination port {}",
                mirror.session_id, mirror.destination_port
            );
        }
    }

    if has_errors {
        return Err(anyhow!(
            "Configuration validation failed for switch '{}': Ports reference non-existent VLANs",
            switch.hostname.as_ref().unwrap_or(&switch.id)
        ));
    }

    Ok(())
}

/// Validate a single switch configuration.
///
/// This is the shared validation function used by both YAML loading and API endpoints.
/// It performs all validation steps in the correct order:
/// 1. Expand port ranges (converts "1-5" to individual ports)
/// 2. Validate required fields are present (hostname, model, management_ip, credentials)
/// 3. Run structural validation (field types, ranges, etc.)
/// 4. Validate VLAN references (ports reference valid VLANs, speed_duplex compatibility)
///
/// # Arguments
/// * `switch` - Mutable reference to the switch config (modified by port expansion)
///
/// # Returns
/// * `Ok(())` if all validation passes
/// * `Err` with detailed error message if validation fails
pub fn validate_switch_config(switch: &mut SwitchConfig) -> Result<()> {
    // Step 1: Expand port ranges
    expand_port_ranges(switch)
        .with_context(|| format!("Port range expansion failed for switch '{}'", switch.id))?;

    // Step 2: Validate required fields are present
    validate_required_fields(switch)
        .with_context(|| format!("Required field validation failed for switch '{}'", switch.id))?;

    // Step 3: Run structural validation (from validator crate)
    validator::Validate::validate(switch).with_context(|| {
        format!(
            "Structural validation failed for switch '{}'",
            switch.hostname.as_ref().unwrap_or(&switch.id)
        )
    })?;

    // Step 4: Validate VLAN references and speed/duplex compatibility
    validate_vlan_references(switch).with_context(|| {
        format!(
            "VLAN reference validation failed for switch '{}'",
            switch.hostname.as_ref().unwrap_or(&switch.id)
        )
    })?;

    Ok(())
}

/// Parse a port ID string that may contain ranges and/or comma-separated values
/// Examples:
/// - "1" → ["1"]
/// - "1-5" → ["1", "2", "3", "4", "5"]
/// - "1,3,5" → ["1", "3", "5"]
/// - "1-3,7,10-12" → ["1", "2", "3", "7", "10", "11", "12"]
/// - "GigabitEthernet1/0/1-5" → ["GigabitEthernet1/0/1", "GigabitEthernet1/0/2", ...]
/// - "port1-5" → ["port1", "port2", "port3", "port4", "port5"]
fn parse_port_id(port_id: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();

    // Split by comma first
    for segment in port_id.split(',') {
        let segment = segment.trim();

        // Check if this segment contains a range
        if segment.contains('-') {
            // Try to parse as a range
            let parts: Vec<&str> = segment.splitn(2, '-').collect();
            if parts.len() != 2 {
                return Err(anyhow!("Invalid port range format: {}", segment));
            }

            let start_str = parts[0].trim();
            let end_str = parts[1].trim();

            // Extract prefix and numeric parts
            let (prefix, start_num) = extract_number_suffix(start_str)?;
            let (end_prefix, end_num) = extract_number_suffix(end_str)?;

            // For ranges like "1-5", start has no prefix
            // For ranges like "GigabitEthernet1/0/1-5", end may have no prefix
            // So we use the start prefix for all if end has no prefix
            let final_prefix = if end_prefix.is_empty() && !prefix.is_empty() {
                prefix
            } else if !end_prefix.is_empty() && prefix != end_prefix {
                return Err(anyhow!(
                    "Port range prefix mismatch: '{}' vs '{}'",
                    prefix,
                    end_prefix
                ));
            } else {
                prefix
            };

            // Generate range
            if start_num > end_num {
                return Err(anyhow!(
                    "Invalid port range: start ({}) > end ({})",
                    start_num,
                    end_num
                ));
            }

            for num in start_num..=end_num {
                if final_prefix.is_empty() {
                    result.push(num.to_string());
                } else {
                    result.push(format!("{}{}", final_prefix, num));
                }
            }
        } else {
            // Not a range, just add as-is
            result.push(segment.to_string());
        }
    }

    Ok(result)
}

/// Extract the non-numeric prefix and numeric suffix from a string
/// Examples:
/// - "1" → ("", 1)
/// - "port5" → ("port", 5)
/// - "GigabitEthernet1/0/5" → ("GigabitEthernet1/0/", 5)
fn extract_number_suffix(s: &str) -> Result<(&str, u32)> {
    // Find the last contiguous sequence of digits
    let mut digit_start = s.len();

    for (i, c) in s.char_indices().rev() {
        if c.is_ascii_digit() {
            digit_start = i;
        } else {
            break;
        }
    }

    if digit_start == s.len() {
        return Err(anyhow!("No numeric suffix found in port ID: {}", s));
    }

    let prefix = &s[..digit_start];
    let number_str = &s[digit_start..];

    let number = number_str
        .parse::<u32>()
        .with_context(|| format!("Failed to parse number from: {}", number_str))?;

    Ok((prefix, number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_port_id_single() {
        assert_eq!(parse_port_id("1").unwrap(), vec!["1"]);
        assert_eq!(parse_port_id("23").unwrap(), vec!["23"]);
    }

    #[test]
    fn test_parse_port_id_range() {
        assert_eq!(
            parse_port_id("1-5").unwrap(),
            vec!["1", "2", "3", "4", "5"]
        );
        assert_eq!(parse_port_id("10-12").unwrap(), vec!["10", "11", "12"]);
    }

    #[test]
    fn test_parse_port_id_list() {
        assert_eq!(parse_port_id("1,3,5").unwrap(), vec!["1", "3", "5"]);
        assert_eq!(
            parse_port_id("1, 3, 5").unwrap(),
            vec!["1", "3", "5"]
        );
    }

    #[test]
    fn test_parse_port_id_mixed() {
        assert_eq!(
            parse_port_id("1-3,7,10-12").unwrap(),
            vec!["1", "2", "3", "7", "10", "11", "12"]
        );
    }

    #[test]
    fn test_parse_port_id_with_prefix() {
        assert_eq!(
            parse_port_id("port1-5").unwrap(),
            vec!["port1", "port2", "port3", "port4", "port5"]
        );
    }

    #[test]
    fn test_parse_port_id_cisco_style() {
        assert_eq!(
            parse_port_id("GigabitEthernet1/0/1-3").unwrap(),
            vec![
                "GigabitEthernet1/0/1",
                "GigabitEthernet1/0/2",
                "GigabitEthernet1/0/3"
            ]
        );
    }

    #[test]
    fn test_parse_port_id_cisco_list() {
        assert_eq!(
            parse_port_id("GigabitEthernet1/0/1,GigabitEthernet1/0/5").unwrap(),
            vec!["GigabitEthernet1/0/1", "GigabitEthernet1/0/5"]
        );
    }

    #[test]
    fn test_extract_number_suffix() {
        assert_eq!(extract_number_suffix("1").unwrap(), ("", 1));
        assert_eq!(extract_number_suffix("23").unwrap(), ("", 23));
        assert_eq!(extract_number_suffix("port5").unwrap(), ("port", 5));
        assert_eq!(
            extract_number_suffix("GigabitEthernet1/0/5").unwrap(),
            ("GigabitEthernet1/0/", 5)
        );
    }

    #[test]
    fn test_parse_port_id_invalid_range() {
        assert!(parse_port_id("5-1").is_err()); // start > end
        assert!(parse_port_id("port1-interface5").is_err()); // prefix mismatch
    }

    #[test]
    fn test_settings_default() {
        let settings = Settings::default();
        assert_eq!(settings.ssh_timeout_secs, 30);
        assert_eq!(settings.max_retries, 3);
        assert_eq!(settings.enforce_port_config, false);
    }

    #[test]
    fn test_runtime_config_default() {
        let runtime_config = RuntimeConfig::default();
        assert_eq!(runtime_config.debug, false);
        assert_eq!(runtime_config.dry_run, false);
        assert_eq!(runtime_config.one_off, false);
        assert_eq!(runtime_config.target_switch, None);
    }

    #[test]
    fn test_port_expansion_in_switch_config() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vendor, ConnectionType, Credentials};

        let yaml = r#"
id: test-switch-01
hostname: test-switch
model: Aruba2930F
vendor: Aruba
management_ip: 192.168.1.1
credentials:
  username: admin
  password: password
  connection_type: ssh
  port: 22
vlans:
  - id: 10
    name: vlan10
ports:
  - port_id: "1-5"
    mode: access
    vlan: 10
    enabled: true
port_mirrors: []
"#;

        let mut config: SwitchConfig = serde_yaml::from_str(yaml).unwrap();
        expand_port_ranges(&mut config).unwrap();

        // Should expand to 5 individual ports
        assert_eq!(config.ports.len(), 5);
        assert_eq!(config.ports[0].port_id, "1");
        assert_eq!(config.ports[1].port_id, "2");
        assert_eq!(config.ports[2].port_id, "3");
        assert_eq!(config.ports[3].port_id, "4");
        assert_eq!(config.ports[4].port_id, "5");

        // All ports should have the same configuration
        for port in &config.ports {
            assert_eq!(port.mode, PortMode::Access);
            assert_eq!(port.vlan, 10);
            assert_eq!(port.enabled, true);
        }
    }

    #[test]
    fn test_port_expansion_mixed_ranges() {
        use crate::models::{Port, PortMode, SwitchConfig};

        let yaml = r#"
id: test-switch-02
hostname: test-switch
model: Aruba2930F
vendor: Aruba
management_ip: 192.168.1.1
credentials:
  username: admin
  password: password
  connection_type: ssh
  port: 22
vlans:
  - id: 10
    name: vlan10
  - id: 20
    name: vlan20
ports:
  - port_id: "1-3"
    mode: access
    vlan: 10
    enabled: true
  - port_id: "5,7,9"
    mode: access
    vlan: 20
    enabled: true
  - port_id: "24"
    mode: trunk
    vlan: 1
    allowed_vlans: [10, 20]
    enabled: true
port_mirrors: []
"#;

        let mut config: SwitchConfig = serde_yaml::from_str(yaml).unwrap();
        expand_port_ranges(&mut config).unwrap();

        // Should have 7 ports total: 3 (1-3) + 3 (5,7,9) + 1 (24)
        assert_eq!(config.ports.len(), 7);

        // Check specific ports
        assert!(config.ports.iter().any(|p| p.port_id == "1" && p.vlan == 10));
        assert!(config.ports.iter().any(|p| p.port_id == "2" && p.vlan == 10));
        assert!(config.ports.iter().any(|p| p.port_id == "3" && p.vlan == 10));
        assert!(config.ports.iter().any(|p| p.port_id == "5" && p.vlan == 20));
        assert!(config.ports.iter().any(|p| p.port_id == "7" && p.vlan == 20));
        assert!(config.ports.iter().any(|p| p.port_id == "9" && p.vlan == 20));
        assert!(config.ports.iter().any(|p| p.port_id == "24" && p.mode == PortMode::Trunk));
    }

    #[test]
    fn test_port_mirror_source_expansion() {
        use crate::models::{PortMirror, MirrorDirection, SwitchConfig};

        let yaml = r#"
id: test-sw-01
hostname: test-switch
model: Aruba2930F
vendor: Aruba
management_ip: 192.168.1.1
credentials:
  username: admin
  password: password
  connection_type: ssh
  port: 22
vlans: []
ports: []
port_mirrors:
  - session_id: "1"
    source_ports: ["1-3", "5"]
    destination_port: "10"
    direction: both
"#;

        let mut config: SwitchConfig = serde_yaml::from_str(yaml).unwrap();
        expand_port_ranges(&mut config).unwrap();

        assert_eq!(config.port_mirrors.len(), 1);
        assert_eq!(config.port_mirrors[0].source_ports.len(), 4);
        assert!(config.port_mirrors[0].source_ports.contains(&"1".to_string()));
        assert!(config.port_mirrors[0].source_ports.contains(&"2".to_string()));
        assert!(config.port_mirrors[0].source_ports.contains(&"3".to_string()));
        assert!(config.port_mirrors[0].source_ports.contains(&"5".to_string()));
        assert_eq!(config.port_mirrors[0].destination_port, "10");
    }

    #[test]
    fn test_app_config_loading_from_yaml() {
        let yaml = r#"
switches:
  - id: sw-01
    hostname: switch1
    model: Aruba2930F
    vendor: Aruba
    management_ip: 192.168.1.1
    credentials:
      username: admin
      password: password
      connection_type: ssh
      port: 22
    vlans:
      - id: 10
        name: vlan10
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
        enabled: true
    port_mirrors: []
    settings:
      ssh_timeout_secs: 60
      max_retries: 5
      enforce_port_config: true
  - id: sw-02
    hostname: switch2
    model: CiscoCatalyst9300_24P_UPOE
    vendor: Cisco
    management_ip: 192.168.1.2
    credentials:
      username: admin
      password: password
      connection_type: ssh
      port: 22
    vlans:
      - id: 20
        name: vlan20
    ports:
      - port_id: "GigabitEthernet1/0/1"
        mode: access
        vlan: 20
        enabled: true
    port_mirrors: []
    settings:
      ssh_timeout_secs: 60
      max_retries: 5
      enforce_port_config: true
"#;

        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.switches.len(), 2);
        assert_eq!(config.switches[0].hostname, Some("switch1".to_string()));
        assert_eq!(config.switches[1].hostname, Some("switch2".to_string()));
        // Settings are now per-switch
        assert_eq!(config.switches[0].settings.ssh_timeout_secs, 60);
        assert_eq!(config.switches[0].settings.max_retries, 5);
        assert_eq!(config.switches[0].settings.enforce_port_config, true);
    }

    #[test]
    fn test_vlan_ip_config_deserialization() {
        use crate::models::{Vlan, VlanIpConfig};

        // Test DHCP
        let yaml = r#"
id: 10
name: management
ip_config: dhcp
"#;
        let vlan: Vlan = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(vlan.ip_config, VlanIpConfig::Dhcp));

        // Test None
        let yaml = r#"
id: 20
name: user
ip_config: none
"#;
        let vlan: Vlan = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(vlan.ip_config, VlanIpConfig::None));

        // Test Static
        // Note: The serialization format is a map with address and netmask directly,
        // not nested under a "static" key
        let yaml = r#"
id: 30
name: server
ip_config:
  address: "192.168.1.1"
  netmask: "255.255.255.0"
"#;
        let vlan: Vlan = serde_yaml::from_str(yaml).unwrap();
        match vlan.ip_config {
            VlanIpConfig::Static { address, netmask } => {
                assert_eq!(address, "192.168.1.1");
                assert_eq!(netmask, "255.255.255.0");
            }
            _ => panic!("Expected static IP config"),
        }
    }

    #[test]
    fn test_serial_credentials_deserialization() {
        use crate::models::{Credentials, ConnectionType};

        let yaml = r#"
username: admin
password: password
connection_type: serial
serial_device: /dev/ttyUSB0
baud_rate: 9600
"#;
        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(creds.connection_type, ConnectionType::Serial);
        assert_eq!(creds.serial_device, Some("/dev/ttyUSB0".to_string()));
        assert_eq!(creds.baud_rate, 9600);
    }

    #[test]
    fn test_enable_secret_deserialization_when_set() {
        use crate::models::Credentials;

        let yaml = r#"
username: admin
password: loginpass
enable_secret: enablepass
connection_type: serial
serial_device: /dev/ttyUSB0
"#;
        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(creds.enable_secret, Some("enablepass".to_string()));
        assert_eq!(creds.password, Some("loginpass".to_string()));
    }

    #[test]
    fn test_enable_secret_deserialization_when_absent() {
        use crate::models::Credentials;

        let yaml = r#"
username: admin
password: loginpass
connection_type: serial
serial_device: /dev/ttyUSB0
"#;
        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(creds.enable_secret, None);
        assert_eq!(creds.password, Some("loginpass".to_string()));
    }

    #[test]
    fn test_enable_secret_fallback_logic() {
        use crate::models::Credentials;

        // When enable_secret is set, it should be used
        let creds_with_secret = Credentials {
            username: "admin".to_string(),
            password: Some("loginpass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: crate::models::ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            enable_secret: Some("enablepass".to_string()),
            jump_hosts: None,
        };

        let secret = creds_with_secret.enable_secret.clone()
            .or_else(|| creds_with_secret.password.clone());
        assert_eq!(secret, Some("enablepass".to_string()));

        // When enable_secret is None, should fall back to password
        let creds_without_secret = Credentials {
            username: "admin".to_string(),
            password: Some("loginpass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: crate::models::ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            enable_secret: None,
            jump_hosts: None,
        };

        let secret = creds_without_secret.enable_secret.clone()
            .or_else(|| creds_without_secret.password.clone());
        assert_eq!(secret, Some("loginpass".to_string()));
    }

    #[test]
    fn test_enable_secret_not_serialized() {
        use crate::models::Credentials;

        let creds = Credentials {
            username: "admin".to_string(),
            password: Some("loginpass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: crate::models::ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            enable_secret: Some("supersecret".to_string()),
            jump_hosts: None,
        };

        let json = serde_json::to_string(&creds).unwrap();
        assert!(!json.contains("supersecret"), "enable_secret should not appear in serialized output");
        assert!(!json.contains("enable_secret"), "enable_secret field should be skip_serialized");
        // password should also not appear (skip_serializing)
        assert!(!json.contains("loginpass"), "password should not appear in serialized output");
    }

    #[test]
    fn test_full_config_with_enable_secret() {
        let yaml = r#"
switches:
  - id: test-switch
    hostname: test-switch
    model: Aruba2930F
    management_ip: 192.168.1.1
    credentials:
      username: admin
      password: admin
      enable_secret: secretpass
      connection_type: serial
      serial_device: /dev/ttyUSB0
      baud_rate: 115200
    vlans: []
    ports: []
"#;
        let config: crate::config::AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.switches.len(), 1);
        let creds = config.switches[0].credentials.as_ref().unwrap();
        assert_eq!(creds.enable_secret, Some("secretpass".to_string()));
        assert_eq!(creds.password, Some("admin".to_string()));
        assert_eq!(creds.username, "admin");
    }

    #[test]
    fn test_validate_vlan_references_valid_config() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 20,
                    name: "users".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 10,
                    allowed_vlans: vec![10, 20],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Should not return an error
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vlan_references_invalid_native_vlan() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 999, // Non-existent VLAN!
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Should return an error because native VLAN doesn't exist
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("test-switch"));
        assert!(error_msg.contains("non-existent VLANs"));
    }

    #[test]
    fn test_validate_vlan_references_invalid_allowed_vlan() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 20,
                    name: "users".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 10,
                    allowed_vlans: vec![10, 20, 2099], // 2099 doesn't exist!
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Should succeed and filter out invalid VLAN
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());

        // Verify that VLAN 2099 was filtered out from allowed_vlans
        let port2 = switch.ports.iter().find(|p| p.port_id == "2").unwrap();
        assert_eq!(port2.allowed_vlans, vec![10, 20],
            "Invalid VLAN 2099 should be filtered out, leaving only valid VLANs 10 and 20");
    }

    #[test]
    fn test_validate_vlan_references_port_mirror_invalid_ports() {
        use crate::models::{Port, PortMode, PortMirror, MirrorDirection, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![
                PortMirror {
                    session_id: "1".to_string(),
                    source_ports: vec!["1".to_string(), "99".to_string()], // Port 99 doesn't exist
                    destination_port: "88".to_string(), // Port 88 doesn't exist
                    direction: MirrorDirection::Both,
                },
            ],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Should succeed but generate warnings about undefined ports
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vlan_references_multiple_issues() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 500, // Invalid native VLAN
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 600, // Invalid native VLAN
                    allowed_vlans: vec![10, 700, 800], // 700 and 800 invalid
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Should error because multiple ports have invalid native VLANs
        // Note: allowed_vlans will be filtered, but native VLAN errors still cause failure
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_err());

        // Even though validation failed, allowed_vlans should have been filtered
        let port2 = switch.ports.iter().find(|p| p.port_id == "2").unwrap();
        assert_eq!(port2.allowed_vlans, vec![10],
            "Invalid VLANs 700 and 800 should be filtered out, leaving only valid VLAN 10");
    }

    #[test]
    fn test_validate_vlan_filtering_complete_workflow() {
        use crate::models::{Port, PortMode, SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // Create a config with mixed valid/invalid VLANs in allowed_vlans
        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 20,
                    name: "users".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 30,
                    name: "guest".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 10,
                    // Mix of valid (10, 20, 30) and invalid (999, 888, 777) VLANs
                    allowed_vlans: vec![10, 999, 20, 888, 30, 777],
                    description: Some("Trunk with mixed VLANs".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        // Before validation: port has 6 VLANs (3 valid, 3 invalid)
        assert_eq!(switch.ports[0].allowed_vlans.len(), 6);

        // Run validation
        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());

        // After validation: port should only have 3 valid VLANs
        assert_eq!(switch.ports[0].allowed_vlans.len(), 3);
        assert_eq!(switch.ports[0].allowed_vlans, vec![10, 20, 30],
            "Only valid VLANs should remain; 999, 888, and 777 should be filtered out");

        // Verify native VLAN is unchanged
        assert_eq!(switch.ports[0].vlan, 10);
    }

    #[test]
    fn test_vlan_filtering_integration() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a complete config with mixed valid/invalid VLANs
        let yaml = r#"
switches:
  - id: filter-test-01
    hostname: filter-test
    model: Aruba2930F
    management_ip: "192.168.1.100"
    credentials:
      username: admin
      password: admin
      connection_type: ssh
      port: 22
    vlans:
      - id: 10
        name: vlan10
      - id: 20
        name: vlan20
      - id: 30
        name: vlan30
    ports:
      - port_id: "5"
        mode: trunk
        vlan: 10
        allowed_vlans: [10, 100, 20, 200, 30, 999]
        enabled: true
settings:
  enforce_port_config: false
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(yaml.as_bytes()).unwrap();
        file.flush().unwrap();

        // Load the configuration - this should trigger validation and filtering
        let config = AppConfig::load(file.path()).unwrap();

        // Verify the switch loaded
        assert_eq!(config.switches.len(), 1);
        assert_eq!(config.switches[0].hostname, Some("filter-test".to_string()));

        // Verify the port configuration
        let port = &config.switches[0].ports[0];
        assert_eq!(port.port_id, "5");

        // CRITICAL: Verify that invalid VLANs (100, 200, 999) were filtered out
        // and only valid VLANs (10, 20, 30) remain
        assert_eq!(port.allowed_vlans.len(), 3,
            "Should have 3 valid VLANs after filtering out 100, 200, and 999");
        assert_eq!(port.allowed_vlans, vec![10, 20, 30],
            "Only valid VLANs should remain after filtering");

        // Verify native VLAN is unchanged
        assert_eq!(port.vlan, 10);
    }

    #[test]
    fn test_appconfig_load_with_validation() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a temporary config file with invalid VLAN reference
        let yaml = r#"
switches:
  - id: test-switch-03
    hostname: test-switch
    model: Aruba2930F
    management_ip: "192.168.1.1"
    credentials:
      username: admin
      password: password
      connection_type: ssh
      port: 22
    vlans:
      - id: 10
        name: management
    ports:
      - port_id: "1"
        mode: access
        vlan: 999
        enabled: true
    port_mirrors: []
settings:
  enforce_port_config: false
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // Should fail to load because of validation error
        let result = AppConfig::load(temp_file.path());
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("validation") || error_msg.contains("VLAN"));
    }

    #[test]
    fn test_appconfig_load_valid_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a temporary config file with valid VLAN references
        let yaml = r#"
switches:
  - id: test-switch-04
    hostname: test-switch
    model: Aruba2930F
    management_ip: "192.168.1.1"
    credentials:
      username: admin
      password: password
      connection_type: ssh
      port: 22
    vlans:
      - id: 10
        name: management
      - id: 20
        name: users
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
        enabled: true
      - port_id: "2"
        mode: trunk
        vlan: 10
        allowed_vlans: [10, 20]
        enabled: true
    port_mirrors: []
settings:
  enforce_port_config: false
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // Should load successfully
        let result = AppConfig::load(temp_file.path());
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.switches.len(), 1);
        assert_eq!(config.switches[0].hostname, Some("test-switch".to_string()));
        assert_eq!(config.switches[0].vlans.len(), 2);
        assert_eq!(config.switches[0].ports.len(), 2);
    }

    #[test]
    fn test_vlan_name_length_validation_aruba_rejects_long_names() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // Aruba limit is 32 chars - this name is 36 chars (real-world example)
        let long_name = "vlan used to access 8/10 port switch";
        assert_eq!(long_name.len(), 36);

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 2097,
                name: long_name.to_string(),
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("36 characters"));
        assert!(err_msg.contains("32 characters"));
        assert!(err_msg.contains("VLAN 2097"));
    }

    #[test]
    fn test_vlan_name_length_validation_aruba_accepts_32_char_names() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // Exactly 32 characters - should be accepted
        let valid_name = "12345678901234567890123456789012";
        assert_eq!(valid_name.len(), 32);

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 100,
                name: valid_name.to_string(),
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vlan_name_length_validation_fortiswitch_allows_longer_names() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // FortiSwitch allows up to 63 chars - this 49 char name should be OK
        let long_name = "This is a fairly long VLAN name for FortiSwitch12";
        assert_eq!(long_name.len(), 49);

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("forti-switch".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 100,
                name: long_name.to_string(),
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vlan_name_length_validation_fortiswitch_rejects_over_63() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // 64 characters - exceeds FortiSwitch's 63 char limit
        let too_long_name = "1234567890123456789012345678901234567890123456789012345678901234";
        assert_eq!(too_long_name.len(), 64);

        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("forti-switch".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 100,
                name: too_long_name.to_string(),
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("64 characters"));
        assert!(err_msg.contains("63 characters"));
    }

    #[test]
    fn test_switch_model_max_vlan_name_length() {
        use crate::models::SwitchModel;

        // Aruba models: 32 char limit
        assert_eq!(SwitchModel::Aruba2530_24G_POE.max_vlan_name_length(), 32);
        assert_eq!(SwitchModel::Aruba2530_8G_POE.max_vlan_name_length(), 32);
        assert_eq!(SwitchModel::Aruba2530_48G_2SFP.max_vlan_name_length(), 32);
        assert_eq!(SwitchModel::Aruba2540_24G.max_vlan_name_length(), 32);
        assert_eq!(SwitchModel::Aruba2540_48G_4SFP.max_vlan_name_length(), 32);
        assert_eq!(SwitchModel::Aruba2930F.max_vlan_name_length(), 32);

        // Cisco: 32 char limit
        assert_eq!(SwitchModel::CiscoCatalyst9300_24P_UPOE.max_vlan_name_length(), 32);

        // FortiSwitch: 63 char limit
        assert_eq!(SwitchModel::Fortiswitch124F_FPOE.max_vlan_name_length(), 63);
    }

    // Note: test_vlan_name_character_validation_rejects_slash and
    // test_vlan_name_character_validation_rejects_multiple_special_chars were removed.
    // Hardware testing on Aruba HP-2530-8G-PoE+ confirmed that these characters are
    // actually accepted by the switch: @ # / & ' ; ! ( ) : ` and more.
    // The only restriction we enforce is the range pattern (e.g., "8-10").

    #[test]
    fn test_vlan_name_special_characters_accepted() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, Port, PortMode, ConnectionType, Credentials};

        // VLAN name with special characters - hardware testing confirmed these work
        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 100,
                name: "Test@VLAN#1/2&3".to_string(),  // Characters verified on hardware
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![Port {
                port_id: "1".to_string(),
                description: None,
                enabled: true,
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                poe_enabled: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                mac_notify: false,
            }],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok(), "VLAN names with special characters should be accepted");
    }

    #[test]
    fn test_vlan_name_character_validation_accepts_valid_names() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, Port, PortMode, ConnectionType, Credentials};

        // Valid VLAN names with allowed characters
        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "management".to_string(),  // Simple alphanumeric
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 20,
                    name: "user-vlan".to_string(),  // With hyphen
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 30,
                    name: "prod_network".to_string(),  // With underscore
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 40,
                    name: "VLAN 40 Guest".to_string(),  // With spaces
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
                Vlan {
                    id: 50,
                    name: "DMZ.external".to_string(),  // With period
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports: vec![Port {
                port_id: "1".to_string(),
                description: None,
                enabled: true,
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                poe_enabled: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                mac_notify: false,
            }],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vlan_name_character_validation_allows_hyphens_underscores_periods_spaces() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, Port, PortMode, ConnectionType, Credentials};

        // Test all allowed special characters in one name
        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 100,
                name: "Net-work_2024.Q1 test".to_string(),  // All allowed special chars
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![Port {
                port_id: "1".to_string(),
                description: None,
                enabled: true,
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                poe_enabled: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                mac_notify: false,
            }],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vlan_name_character_validation_rejects_number_range_pattern() {
        use crate::models::{SwitchConfig, SwitchModel, Vlan, ConnectionType, Credentials};

        // VLAN name with "8-10" pattern which Aruba interprets as a range
        let mut switch = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("password".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![Vlan {
                id: 2097,
                name: "Access 8-10 port sw".to_string(),
                description: None,
                ip_config: crate::models::VlanIpConfig::None,
            }],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            settings: Settings::default(),
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
        };

        let result = validate_vlan_references(&mut switch);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("number range pattern"));
        assert!(err_msg.contains("8-10"));
    }
}
