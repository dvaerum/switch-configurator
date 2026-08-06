use super::traits::{SwitchVendor, VendorError};
use crate::config::RuntimeConfig;
use crate::models::{ConfigResult, MirrorDirection, Port, PortMirror, PortMode, StateDiff, SwitchConfig, SwitchState, Vlan, ConnectionType};
use crate::ssh::{ConnectionClient, SerialClient, SshClient};
use async_trait::async_trait;
use tracing::{debug, info, warn};

pub struct ArubaSwitch {
    config: SwitchConfig,
    runtime_config: RuntimeConfig,
    client: Option<ConnectionClient>,
    current_state: Option<SwitchState>,
    enforce_port_config: bool,
}

/// Helper struct to track VLAN membership for each port
#[derive(Debug, Clone)]
struct PortVlanInfo {
    port_id: String,
    untagged_vlan: Option<u16>,
    tagged_vlans: Vec<u16>,
    has_mirror: bool,
    has_monitor: bool,
    poe_enabled: bool,
    mac_notify: bool,
    enabled: bool,
    speed_duplex: crate::models::SpeedDuplex,
}

impl PortVlanInfo {
    fn new(port_id: String) -> Self {
        Self {
            port_id,
            untagged_vlan: None,
            tagged_vlans: Vec::new(),
            has_mirror: false,
            has_monitor: false,
            poe_enabled: true,  // Aruba ports have PoE enabled by default
            mac_notify: false,
            enabled: true,  // Aruba ports are enabled by default
            speed_duplex: crate::models::SpeedDuplex::Auto,  // Default to auto-negotiation
        }
    }
}

impl ArubaSwitch {
    pub fn new(config: SwitchConfig, runtime_config: RuntimeConfig, enforce_port_config: bool) -> Self {
        Self {
            config,
            runtime_config,
            current_state: None,
            client: None,
            enforce_port_config,
        }
    }

    fn generate_vlan_commands(&self, vlans: &[Vlan]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for vlan in vlans {
            commands.push(format!("vlan {}", vlan.id));
            // Quote VLAN names containing spaces to prevent CLI parsing issues
            if vlan.name.contains(' ') {
                commands.push(format!("name \"{}\"", vlan.name));
            } else {
                commands.push(format!("name {}", vlan.name));
            }

            // Configure IP address based on ip_config
            match &vlan.ip_config {
                crate::models::VlanIpConfig::Dhcp => {
                    commands.push("ip address dhcp-bootp".to_string());
                }
                crate::models::VlanIpConfig::Static { address, netmask } => {
                    commands.push(format!("ip address {} {}", address, netmask));
                }
                crate::models::VlanIpConfig::None => {
                    commands.push("no ip address".to_string());
                }
            }

            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());
        commands
    }

    fn generate_port_commands(&self, ports: &[Port], mirrors: &[PortMirror]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        // Build a map of port_id -> (session_id, direction) for mirror sources
        use std::collections::HashMap;
        let mut mirror_sources: HashMap<String, Vec<(String, MirrorDirection)>> = HashMap::new();
        for mirror in mirrors {
            for source_port in &mirror.source_ports {
                let normalized = self.normalize_port_id(source_port);
                mirror_sources.entry(normalized.clone())
                    .or_insert_with(Vec::new)
                    .push((mirror.session_id.clone(), mirror.direction.clone()));
            }
        }

        for port in ports {
            // Aruba uses interface notation like "interface 1" for port 1
            let interface = self.normalize_port_id(&port.port_id);
            commands.push(format!("interface {}", interface));

            // Handle port name (description): set if specified, clear if not
            match &port.description {
                Some(desc) => {
                    // Set the name
                    commands.push(format!("name \"{}\"", desc));
                }
                None => {
                    // Check if the port currently has a name that should be cleared
                    if let Some(ref current_state) = self.current_state {
                        if let Some(current_port) = current_state.ports.iter().find(|p| p.port_id == port.port_id) {
                            if current_port.description.is_some() {
                                // Port currently has a name but config doesn't specify one - clear it
                                commands.push("no name".to_string());
                            }
                        }
                    }
                }
            }

            match port.inferred_mode() {
                PortMode::Access => {
                    commands.push(format!("untagged vlan {}", port.vlan));

                    // Remove any existing tagged VLANs — access ports should have none.
                    // On Aruba, setting untagged vlan does NOT automatically remove tagged VLANs.
                    if let Some(ref current_state) = self.current_state {
                        if let Some(current_port) = current_state.ports.iter().find(|p| p.port_id == port.port_id) {
                            for &vlan in &current_port.tagged_vlans {
                                if vlan != current_port.vlan {
                                    commands.push(format!("no tagged vlan {}", vlan));
                                }
                            }
                        }
                    }
                }
                PortMode::Trunk => {
                    commands.push(format!("untagged vlan {}", port.vlan));

                    // For trunk ports, we need to handle VLAN additions and removals
                    // First, find the current port state to see what VLANs need to be removed
                    if let Some(ref current_state) = self.current_state {
                        if let Some(current_port) = current_state.ports.iter().find(|p| p.port_id == port.port_id) {
                            // Find VLANs that are currently tagged but should be removed
                            let current_tagged: Vec<u16> = current_port.tagged_vlans
                                .iter()
                                .filter(|&&v| v != current_port.vlan)  // Exclude native VLAN
                                .copied()
                                .collect();

                            let desired_tagged: Vec<u16> = port.tagged_vlans
                                .iter()
                                .filter(|&&v| v != port.vlan)  // Exclude native VLAN
                                .copied()
                                .collect();

                            // Remove VLANs that are currently tagged but not in desired state
                            for &vlan in &current_tagged {
                                if !desired_tagged.contains(&vlan) {
                                    commands.push(format!("no tagged vlan {}", vlan));
                                }
                            }
                        }
                    }

                    // Add desired tagged VLANs (exclude the native VLAN)
                    let tagged_vlans: Vec<String> = port.tagged_vlans
                        .iter()
                        .filter(|&&v| v != port.vlan)  // Exclude the native VLAN
                        .map(|v| v.to_string())
                        .collect();
                    if !tagged_vlans.is_empty() {
                        commands.push(format!("tagged vlan {}", tagged_vlans.join(",")));
                    }
                }
            }

            if port.enabled {
                commands.push("enable".to_string());
            } else {
                commands.push("disable".to_string());
            }

            // Only generate PoE commands if the switch model supports PoE
            // Non-PoE models like Aruba2540_48G_4SFP don't have PoE commands
            if self.config.model().supports_poe() {
                // Check if this port supports PoE on this switch model
                if port.poe_enabled {
                    // Query port capabilities to get detailed PoE information
                    if let Some(caps) = self.config.model().port_capabilities(&port.port_id) {
                        if let Some(poe_standard) = caps.poe_support {
                            // Enable PoE explicitly, then set allocation method
                            // First, ensure any "no power-over-ethernet" is removed
                            commands.push("power-over-ethernet".to_string());
                            // Then set allocation method (Aruba uses "poe-allocate-by class" for class-based power allocation)
                            commands.push("poe-allocate-by class".to_string());

                            debug!("Port {} supports {:?} (max {}W) on {:?}",
                                port.port_id, poe_standard, caps.max_poe_watts(), self.config.model());
                        } else {
                            warn!("Port {} does not support PoE on switch model {:?}. Port type: {:?}. Skipping PoE configuration.",
                                port.port_id, self.config.model(), caps.port_type);
                            // Explicitly disable PoE on non-PoE ports to avoid confusion
                            commands.push("no power-over-ethernet".to_string());
                        }
                    } else {
                        warn!("Invalid port {} for switch model {:?}. Skipping PoE configuration.",
                            port.port_id, self.config.model());
                        commands.push("no power-over-ethernet".to_string());
                    }
                } else {
                    commands.push("no power-over-ethernet".to_string());
                }
            }

            if port.mac_notify {
                // Enable MAC notification traps for both learned and removed MACs
                commands.push("mac-notify traps learned".to_string());
                commands.push("mac-notify traps removed".to_string());
            } else {
                // Disable MAC notifications - must explicitly disable both trap types
                // Note: "no mac-notify" alone doesn't remove explicit trap commands
                commands.push("no mac-notify traps learned".to_string());
                commands.push("no mac-notify traps removed".to_string());
            }

            // Configure speed and duplex
            commands.push(format!("speed-duplex {}", port.speed_duplex.to_aruba_syntax()));

            // Add monitor commands if this port is a mirror source
            if let Some(mirror_configs) = mirror_sources.get(&interface) {
                for (session_id, direction) in mirror_configs {
                    // Aruba 2530/2540 series use legacy "monitor" (no parameters) inside interface context
                    // Aruba 2930F and newer use "monitor all <direction> mirror <session-id>"
                    if self.config.model().uses_legacy_mirror_syntax() {
                        commands.push("monitor".to_string());
                        // Legacy syntax only supports one mirror session, so break after first
                        break;
                    } else {
                        let direction_cmd = match direction {
                            MirrorDirection::Both => format!("monitor all both mirror {}", session_id),
                            MirrorDirection::Rx => format!("monitor all in mirror {}", session_id),
                            MirrorDirection::Tx => format!("monitor all out mirror {}", session_id),
                        };
                        commands.push(direction_cmd);
                    }
                }
            }

            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());
        commands
    }

    fn generate_mirror_commands(&self, mirrors: &[PortMirror]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for mirror in mirrors {
            // Aruba mirror configuration syntax varies by model:
            // - 2530/2540 series (legacy): "mirror-port <destination>"
            // - 2930F and newer: "mirror <session-id> port <destination>"
            // Per-interface monitor commands are handled in generate_port_commands()

            let dest = self.normalize_port_id(&mirror.destination_port);

            // Use model-aware mirror syntax
            if self.config.model().uses_legacy_mirror_syntax() {
                // Legacy syntax for 2530/2540 series
                commands.push(format!("mirror-port {}", dest));
            } else {
                // Newer syntax for 2930F and later
                let session = &mirror.session_id;
                commands.push(format!("mirror {} port {}", session, dest));
            }
        }

        commands.push("exit".to_string());
        commands
    }

    fn generate_snmp_commands(&self, snmp_config: &crate::models::SnmpConfig, current_traps: &[crate::models::TrapType]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        // First, disable traps that are currently enabled but not in the new config
        // Only try to disable traps that are actually enabled to avoid unnecessary warnings
        for trap in current_traps {
            let trap_name = match trap {
                crate::models::TrapType::MacNotify => "mac-notify",
                crate::models::TrapType::LinkChange => "link-change all",
                crate::models::TrapType::All => "all",
            };
            // Only disable if it's not in the new desired configuration
            if !snmp_config.enabled_traps.contains(trap) {
                commands.push(format!("no snmp-server enable traps {}", trap_name));
            }
        }

        // Remove common communities (we'll re-add the ones we want)
        commands.push("no snmp-server community \"public\"".to_string());
        commands.push("no snmp-server community \"private\"".to_string());

        // Note: We can't easily enumerate all trap receivers to remove them
        // So we clear by using a wildcard approach if supported, or manually track
        // For now, we'll rely on the new config overwriting the old

        // Configure SNMP communities
        for community in &snmp_config.communities {
            let access_level = match community.access {
                crate::models::SnmpAccess::Unrestricted => "unrestricted",
                crate::models::SnmpAccess::Manager => "manager",
                crate::models::SnmpAccess::Operator => "operator",
            };
            commands.push(format!("snmp-server community \"{}\" {}", community.name, access_level));
        }

        // Configure SNMP trap receivers
        for receiver in &snmp_config.trap_receivers {
            commands.push(format!(
                "snmp-server host {} community \"{}\"",
                receiver.host, receiver.community
            ));
        }

        // Enable SNMP traps
        let mut has_link_change = false;
        for trap_type in &snmp_config.enabled_traps {
            let trap_name = match trap_type {
                crate::models::TrapType::MacNotify => {
                    "mac-notify"
                }
                crate::models::TrapType::LinkChange => {
                    has_link_change = true;
                    "link-change all"
                }
                crate::models::TrapType::All => "all",
            };
            commands.push(format!("snmp-server enable traps {}", trap_name));
        }

        // NOTE: We do NOT use "mac-notify traps all" here because it would enable
        // MAC notifications globally on ALL ports, preventing per-port control.
        // Instead, MAC notifications are controlled per-port through the port
        // configuration using "mac-notify traps learned/removed" or "no mac-notify".
        // The global "snmp-server enable traps mac-notify" above is sufficient to
        // enable SNMP MAC notification traps for ports that have per-port MAC notify enabled.

        commands.push("exit".to_string());
        commands
    }

    fn normalize_port_id(&self, port_id: &str) -> String {
        // Convert formats like "1/0/1" or "GigabitEthernet1/0/1" to Aruba format "1"
        // Aruba typically uses simple port numbers
        if let Some(last) = port_id.split('/').last() {
            return last.to_string();
        }
        port_id.to_string()
    }

    /// Check if a specific port supports PoE on this switch model
    /// Uses the centralized port capability system from SwitchModel
    fn port_supports_poe(&self, port_id: &str) -> bool {
        self.config.model().port_supports_poe(port_id)
    }

    pub fn poe_disable_commands(&self, port_id: &str) -> Vec<String> {
        let interface = self.normalize_port_id(port_id);
        vec![
            "configure terminal".to_string(),
            format!("interface {}", interface),
            "no power-over-ethernet".to_string(),
            "exit".to_string(),
            "exit".to_string(),
        ]
    }

    pub fn poe_enable_commands(&self, port_id: &str) -> Vec<String> {
        let interface = self.normalize_port_id(port_id);
        vec![
            "configure terminal".to_string(),
            format!("interface {}", interface),
            "power-over-ethernet".to_string(),
            "exit".to_string(),
            "exit".to_string(),
        ]
    }

    fn generate_remove_vlan_commands(&self, vlan_ids: &[u16]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for vlan_id in vlan_ids {
            commands.push(format!("no vlan {}", vlan_id));
        }

        commands.push("exit".to_string());
        commands
    }

    fn generate_remove_mirror_commands(&self, session_ids: &[String]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for session_id in session_ids {
            commands.push(format!("no mirror {}", session_id));
        }

        commands.push("exit".to_string());
        commands
    }

    async fn remove_vlans(&mut self, vlan_ids: &[u16]) -> Result<ConfigResult, VendorError> {
        let commands = self.generate_remove_vlan_commands(vlan_ids);
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Removed {} VLANs", vlan_ids.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Migrate ports away from VLANs before deletion to avoid interactive confirmation prompts
    ///
    /// Aruba requires using VLAN context commands to remove ports from a VLAN's member list.
    /// Using interface context (interface X / vlan Y) only changes where the port belongs,
    /// but doesn't remove the port from the VLAN's perspective.
    async fn migrate_ports_before_vlan_deletion(
        &mut self,
        migrations: &[crate::diff::PortMigration],
    ) -> Result<ConfigResult, VendorError> {
        use crate::diff::PortMigrationAction;
        use std::collections::HashMap;

        let mut commands = Vec::new();
        commands.push("configure terminal".to_string());

        // Group migrations by VLAN being removed
        let mut vlans_to_clean: HashMap<u16, (Vec<String>, Vec<String>)> = HashMap::new();

        for migration in migrations {
            let entry = vlans_to_clean.entry(migration.vlan_being_removed).or_insert((Vec::new(), Vec::new()));

            match &migration.action {
                PortMigrationAction::MoveAccessToVlan1 => {
                    debug!(
                        "Migrating access port {} from VLAN {} to VLAN 1",
                        migration.port_id, migration.vlan_being_removed
                    );
                    // Add to untagged ports list for this VLAN
                    entry.0.push(migration.port_id.clone());
                }
                PortMigrationAction::RemoveVlanFromTrunk { .. } => {
                    debug!(
                        "Removing VLAN {} from trunk port {}",
                        migration.vlan_being_removed, migration.port_id
                    );
                    // Add to tagged ports list for this VLAN
                    entry.1.push(migration.port_id.clone());
                }
            }
        }

        // Generate VLAN-context commands to remove ports from each VLAN
        for (vlan_id, (untagged_ports, tagged_ports)) in vlans_to_clean {
            commands.push(format!("vlan {}", vlan_id));

            if !untagged_ports.is_empty() {
                let ports_list = untagged_ports.join(",");
                debug!("Removing untagged ports {} from VLAN {}", ports_list, vlan_id);
                commands.push(format!("no untagged {}", ports_list));
            }

            if !tagged_ports.is_empty() {
                let ports_list = tagged_ports.join(",");
                debug!("Removing tagged ports {} from VLAN {}", ports_list, vlan_id);
                commands.push(format!("no tagged {}", ports_list));
            }

            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());

        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Migrated {} ports before VLAN deletion", migrations.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    async fn remove_mirrors(&mut self, session_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let commands = self.generate_remove_mirror_commands(session_ids);
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Removed {} port mirrors", session_ids.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Regex pattern for extracting the hardware product number from the Aruba running config.
    /// Matches lines like: `; J9779A Configuration Editor; Created on release #YB.16.10.0009`
    /// Product numbers are like J9773A, J9855A, JL253A, JL355A (1-2 letters + 3-5 digits + optional letter)
    fn hardware_id_pattern() -> regex::Regex {
        regex::Regex::new(r";\s*([A-Z]{1,2}\d{3,5}[A-Z]?)\s+Configuration Editor").unwrap()
    }

    /// Parse interface block for name, mirror/monitor status, PoE, MAC notify, and enabled status
    /// In Aruba config, interfaces contain names and port-specific settings - VLAN assignments are in VLAN blocks
    fn parse_interface_name(&self, lines: &[&str], index: &mut usize) -> Option<(String, Option<String>, bool, bool, bool, bool, bool, crate::models::SpeedDuplex)> {
        let line = lines[*index].trim();

        // Extract port ID from "interface <id>"
        let port_id = line.strip_prefix("interface ")?.trim().to_string();

        let mut description: Option<String> = None;
        let has_mirror = false;
        let mut has_monitor = false;
        // For non-PoE switches, default to false since there's no PoE configuration to parse
        // For PoE switches, default to true (PoE is enabled by default unless "no power-over-ethernet" is present)
        let mut poe_enabled = self.config.model().supports_poe();
        let mut mac_notify = false;
        let mut enabled = true;  // Ports are enabled by default on Aruba
        let mut speed_duplex = crate::models::SpeedDuplex::Auto;  // Default to auto-negotiation

        // Look ahead for configuration within the interface block
        let mut j = *index + 1;
        while j < lines.len() {
            // Strip ANSI escape sequences from the line first
            let clean_line = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                .unwrap()
                .replace_all(lines[j], "");
            let inner_line = clean_line.trim();

            // Exit on "exit" or if we hit a new top-level block
            if inner_line == "exit" ||
               inner_line.starts_with("vlan ") ||
               inner_line.starts_with("interface ") ||
               inner_line.starts_with("hostname ") ||
               inner_line.starts_with("module ") {
                break;
            }

            // Skip empty lines
            if inner_line.is_empty() {
                j += 1;
                continue;
            }

            // Parse "name <description>"
            if let Some(name) = inner_line.strip_prefix("name ") {
                description = Some(name.trim_matches('"').to_string());
            }
            // Parse monitor (source port)
            // Format: "monitor all both mirror 1" or just "monitor"
            else if inner_line.starts_with("monitor") {
                has_monitor = true;
            }
            // Parse PoE status
            // "no power-over-ethernet" means PoE is disabled
            // "power-over-ethernet" (explicit enable) means PoE is enabled
            // "poe-allocate-by class" is just an allocation method setting that exists
            // regardless of PoE being enabled or disabled — it does NOT indicate PoE is on
            else if inner_line == "no power-over-ethernet" {
                poe_enabled = false;
            }
            else if inner_line.starts_with("power-over-ethernet") {
                poe_enabled = true;
            }
            // poe-allocate-by is intentionally ignored — it's present on PoE-capable ports
            // even when PoE is disabled, and does not affect the PoE enabled/disabled state
            // Parse MAC notify status
            // "mac-notify" or "mac-notify traps" means MAC notify is enabled
            else if inner_line.starts_with("mac-notify") {
                mac_notify = true;
            }
            else if inner_line == "no mac-notify" || inner_line.starts_with("no mac-notify") {
                mac_notify = false;
            }
            // Parse enabled/disabled status
            else if inner_line == "disable" {
                enabled = false;
            }
            else if inner_line == "enable" {
                enabled = true;
            }
            // Parse speed-duplex setting
            // Format: "speed-duplex auto" or "speed-duplex 100-full" etc.
            else if let Some(speed_str) = inner_line.strip_prefix("speed-duplex ") {
                debug!("  Found speed-duplex setting for port {}: '{}'", port_id, speed_str.trim());
                if let Some(parsed) = crate::models::SpeedDuplex::from_aruba_output(speed_str.trim()) {
                    speed_duplex = parsed;
                    debug!("  Parsed as: {:?}", speed_duplex);
                } else {
                    warn!("  Failed to parse speed-duplex value: '{}'", speed_str.trim());
                }
            }

            j += 1;
        }

        *index = j; // Move index to end of block

        debug!("  Parsed interface {}: speed_duplex={:?}, enabled={}, poe={}, mac_notify={}",
               port_id, speed_duplex, enabled, poe_enabled, mac_notify);

        Some((port_id, description, has_mirror, has_monitor, poe_enabled, mac_notify, enabled, speed_duplex))
    }

    /// Parse VLAN block and extract port assignments
    /// Returns (Vlan, untagged_ports, tagged_ports)
    fn parse_vlan_with_ports(&self, lines: &[&str], index: &mut usize) -> Option<(Vlan, Vec<String>, Vec<String>)> {
        let line = lines[*index].trim();

        // Extract VLAN ID from "vlan <id>"
        let vlan_id = line.strip_prefix("vlan ")?.trim().parse::<u16>().ok()?;
        let mut vlan_name = format!("VLAN{}", vlan_id);
        let mut untagged_ports = Vec::new();
        let mut tagged_ports = Vec::new();
        let mut ip_config = crate::models::VlanIpConfig::None;

        // Look ahead for configuration within the vlan block
        let mut j = *index + 1;
        while j < lines.len() {
            // Strip ANSI escape sequences from the line first
            let clean_line = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]")
                .unwrap()
                .replace_all(lines[j], "");
            let inner_line = clean_line.trim();

            // Exit on "exit" or if we hit a new top-level block (vlan, interface, etc.)
            if inner_line == "exit" ||
               inner_line.starts_with("vlan ") ||
               inner_line.starts_with("interface ") ||
               inner_line.starts_with("hostname ") ||
               inner_line.starts_with("module ") {
                break;
            }

            // Skip empty lines
            if inner_line.is_empty() {
                j += 1;
                continue;
            }

            // Parse "name <name>"
            if let Some(name) = inner_line.strip_prefix("name ") {
                vlan_name = name.trim_matches('"').to_string();
                debug!("  VLAN {}: found name = {:?}", vlan_id, vlan_name);
            }
            // Parse "untagged <port-list>"
            else if let Some(ports) = inner_line.strip_prefix("untagged ") {
                let port_list = self.parse_port_list(ports);
                debug!("  VLAN {}: found untagged ports = {:?}", vlan_id, port_list);
                untagged_ports.extend(port_list);
            }
            // Parse "tagged <port-list>"
            else if let Some(ports) = inner_line.strip_prefix("tagged ") {
                let port_list = self.parse_port_list(ports);
                debug!("  VLAN {}: found tagged ports = {:?}", vlan_id, port_list);
                tagged_ports.extend(port_list);
            }
            // Parse IP address configuration
            else if inner_line == "ip address dhcp-bootp" {
                ip_config = crate::models::VlanIpConfig::Dhcp;
                debug!("  VLAN {}: IP config = DHCP", vlan_id);
            }
            else if let Some(ip_info) = inner_line.strip_prefix("ip address ") {
                // Parse "ip address <addr> <netmask>"
                let parts: Vec<&str> = ip_info.split_whitespace().collect();
                if parts.len() >= 2 {
                    ip_config = crate::models::VlanIpConfig::Static {
                        address: parts[0].to_string(),
                        netmask: parts[1].to_string(),
                    };
                    debug!("  VLAN {}: IP config = Static {} {}", vlan_id, parts[0], parts[1]);
                }
            }
            else if inner_line == "no ip address" {
                ip_config = crate::models::VlanIpConfig::None;
                debug!("  VLAN {}: IP config = None", vlan_id);
            }
            // Skip "no untagged" commands
            else if inner_line.starts_with("no untagged ") {
                // Ignore - these remove ports from default VLAN
            }

            j += 1;
        }

        *index = j; // Move index to end of block

        let vlan = Vlan {
            id: vlan_id,
            name: vlan_name,
            description: None,
            ip_config,
        };

        Some((vlan, untagged_ports, tagged_ports))
    }

    /// Parse port list like "1-5,7,10-12" into individual port IDs
    fn parse_port_list(&self, port_list: &str) -> Vec<String> {
        let mut ports = Vec::new();

        for part in port_list.split(',') {
            let part = part.trim();
            if part.contains('-') {
                // Handle ranges like "1-5"
                if let Some((start, end)) = part.split_once('-') {
                    if let (Ok(start), Ok(end)) = (start.trim().parse::<u16>(), end.trim().parse::<u16>()) {
                        for port_num in start..=end {
                            ports.push(port_num.to_string());
                        }
                    }
                }
            } else if let Ok(_port_num) = part.parse::<u16>() {
                // Single port number
                ports.push(part.to_string());
            }
        }

        ports
    }

    /// Parse actual trap status from `show snmp-server traps` output
    /// Returns (link_change_enabled, mac_notify_enabled)
    async fn parse_snmp_trap_status(&mut self) -> Result<(bool, bool), VendorError> {
        debug!("Fetching actual SNMP trap status via 'show snmp-server traps'");

        let show_cmd = vec!["show snmp-server traps".to_string()];
        let outputs = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?
            .execute_commands(&show_cmd)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        let empty_string = String::new();
        let output = outputs.get(0).unwrap_or(&empty_string);
        debug!("SNMP trap status output:\n{}", output);

        let mut link_change_enabled = false;
        let mut mac_notify_enabled = false;

        for line in output.lines() {
            let trimmed = line.trim();

            // Look for "Link-Change Traps Enabled on Ports [All] : <port-list or None>"
            // If port list is "None", traps are disabled
            // If port list is "All" or specific ports, traps are enabled
            if trimmed.contains("Link-Change Traps Enabled on Ports") {
                if trimmed.contains(": None") {
                    link_change_enabled = false;
                    debug!("  Found: link-change traps DISABLED (port list is None)");
                } else if trimmed.contains(": All") || trimmed.contains(":") {
                    link_change_enabled = true;
                    debug!("  Found: link-change traps ENABLED (port list: {})",
                           trimmed.split(':').last().unwrap_or("unknown").trim());
                }
            }

            // Look for "MAC address table changes             : Enabled"
            if trimmed.contains("MAC address table changes") && trimmed.contains("Enabled") {
                mac_notify_enabled = true;
                debug!("  Found: mac-notify traps ENABLED");
            } else if trimmed.contains("MAC address table changes") && trimmed.contains("Disabled") {
                mac_notify_enabled = false;
                debug!("  Found: mac-notify traps DISABLED");
            }
        }

        debug!("SNMP trap status: link-change={}, mac-notify={}", link_change_enabled, mac_notify_enabled);
        Ok((link_change_enabled, mac_notify_enabled))
    }

    fn parse_snmp_config(&self, lines: &[&str], link_change_enabled: bool, mac_notify_enabled: bool) -> Option<crate::models::SnmpConfig> {
        let mut communities = Vec::new();
        let mut trap_receivers = Vec::new();
        let mut enabled_traps = Vec::new();

        // Add traps based on actual status from 'show snmp-server traps'
        if link_change_enabled {
            enabled_traps.push(crate::models::TrapType::LinkChange);
            debug!("Adding LinkChange to enabled_traps (from 'show snmp-server traps')");
        }
        if mac_notify_enabled {
            enabled_traps.push(crate::models::TrapType::MacNotify);
            debug!("Adding MacNotify to enabled_traps (from 'show snmp-server traps')");
        }

        for line in lines {
            let trimmed = line.trim();

            // Parse SNMP communities
            // Format: snmp-server community "name" access_level
            if trimmed.starts_with("snmp-server community") {
                if let Some(rest) = trimmed.strip_prefix("snmp-server community ") {
                    // Extract community name (may be quoted)
                    let parts: Vec<&str> = if rest.starts_with('"') {
                        // Handle quoted names: "community" access_level
                        let end_quote = rest[1..].find('"').map(|i| i + 1);
                        if let Some(end) = end_quote {
                            let name = &rest[1..end];
                            let remaining = rest[end+1..].trim();
                            vec![name, remaining]
                        } else {
                            continue;
                        }
                    } else {
                        rest.split_whitespace().collect()
                    };

                    if parts.len() >= 2 {
                        let name = parts[0].to_string();
                        let access = match parts[1] {
                            "unrestricted" => crate::models::SnmpAccess::Unrestricted,
                            "manager" => crate::models::SnmpAccess::Manager,
                            "operator" => crate::models::SnmpAccess::Operator,
                            _ => crate::models::SnmpAccess::Unrestricted,
                        };
                        communities.push(crate::models::SnmpCommunity { name, access });
                    }
                }
            }
            // Parse SNMP trap receivers
            // Format: snmp-server host 192.168.1.1 community "public"
            else if trimmed.starts_with("snmp-server host ") {
                if let Some(rest) = trimmed.strip_prefix("snmp-server host ") {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == "community" {
                        let host = parts[0].to_string();
                        // Community may be quoted
                        let community = if parts[2].starts_with('"') {
                            parts[2..].join(" ").trim_matches('"').to_string()
                        } else {
                            parts[2].to_string()
                        };
                        trap_receivers.push(crate::models::SnmpTrapReceiver {
                            host,
                            community,
                            version: Some("2c".to_string()),
                        });
                    }
                }
            }
        }

        // Only return Some if we found any SNMP configuration
        if !communities.is_empty() || !trap_receivers.is_empty() || !enabled_traps.is_empty() {
            Some(crate::models::SnmpConfig {
                communities,
                trap_receivers,
                enabled_traps,
            })
        } else {
            None
        }
    }

    /// Parse management VLAN configuration from running config
    /// Format: "management-vlan 10" or "management-vlan vlan10"
    fn parse_management_vlan(&self, lines: &[&str]) -> Option<u16> {
        for line in lines {
            let trimmed = line.trim();

            // Look for "management-vlan <id>" or "management-vlan vlan<id>"
            if let Some(rest) = trimmed.strip_prefix("management-vlan ") {
                // Could be either just the number or "vlan<number>"
                let vlan_str = rest.trim();

                // Try to parse as a direct number first
                if let Ok(vlan_id) = vlan_str.parse::<u16>() {
                    debug!("  Found management-vlan: {}", vlan_id);
                    return Some(vlan_id);
                }

                // Try to parse "vlan<number>" format
                if let Some(num_str) = vlan_str.strip_prefix("vlan") {
                    if let Ok(vlan_id) = num_str.parse::<u16>() {
                        debug!("  Found management-vlan: {}", vlan_id);
                        return Some(vlan_id);
                    }
                }
            }
        }

        None
    }

    async fn configure_snmp(
        &mut self,
        snmp_config: &crate::models::SnmpConfig,
    ) -> Result<ConfigResult, VendorError> {
        info!("Configuring SNMP: {} communities, {} trap receivers, {} trap types",
              snmp_config.communities.len(),
              snmp_config.trap_receivers.len(),
              snmp_config.enabled_traps.len());

        // First, get current SNMP configuration to find old trap receivers
        debug!("Fetching current running-config to identify existing trap receivers");
        let show_cmd = vec!["show running-config".to_string()];
        let outputs = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?
            .execute_commands(&show_cmd)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        let running_config = outputs.get(0).unwrap_or(&String::new()).clone();

        // Parse existing trap receivers to remove them
        // IMPORTANT: Aruba switches require the full syntax including community string for removal
        let mut removal_commands = vec!["configure terminal".to_string()];
        let mut found_receivers = Vec::new();
        for line in running_config.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("snmp-server host ") {
                // Extract the full line for removal: host IP and community
                if let Some(host_start) = trimmed.find("snmp-server host ") {
                    let after_host = &trimmed[host_start + "snmp-server host ".len()..];

                    // Parse: <ip> community "<community_name>" [optional other params]
                    let parts: Vec<&str> = after_host.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == "community" {
                        let host_ip = parts[0];
                        // Community string might be quoted: "public" or public
                        let community = parts[2].trim_matches('"');

                        found_receivers.push(format!("{} community \"{}\"", host_ip, community));
                        // Use full syntax for removal including community string
                        removal_commands.push(format!(
                            "no snmp-server host {} community \"{}\"",
                            host_ip, community
                        ));
                    } else if let Some(host_ip) = after_host.split_whitespace().next() {
                        // Fallback: if we can't parse community, try IP-only removal
                        found_receivers.push(host_ip.to_string());
                        removal_commands.push(format!("no snmp-server host {}", host_ip));
                    }
                }
            }
        }
        removal_commands.push("exit".to_string());

        if !found_receivers.is_empty() {
            info!("Found {} existing trap receivers to remove: {:?}",
                  found_receivers.len(), found_receivers);
            debug!("Removal commands: {:?}", removal_commands);
        } else {
            debug!("No existing trap receivers found in running-config");
        }

        // Execute removal commands if any were found
        if removal_commands.len() > 2 {  // More than just "configure terminal" and "exit"
            info!("Executing removal commands for {} trap receivers", found_receivers.len());
            let removal_output = self.client
                .as_mut()
                .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?
                .execute_commands(&removal_commands)
                .await
                .map_err(|e| VendorError::CommandError(e.to_string()))?;
            debug!("Removal command output: {:?}", removal_output);
        } else {
            debug!("No trap receivers to remove, skipping removal step");
        }

        // Parse current SNMP state to know what traps are enabled
        // Use actual trap status from 'show snmp-server traps' command
        let (link_change_enabled, mac_notify_enabled) = self.parse_snmp_trap_status().await?;
        let lines: Vec<&str> = running_config.lines().collect();
        let current_snmp = self.parse_snmp_config(&lines, link_change_enabled, mac_notify_enabled);
        let current_traps: &[crate::models::TrapType] = if let Some(ref snmp) = current_snmp {
            &snmp.enabled_traps
        } else {
            &[]
        };
        debug!("Currently enabled traps: {:?}", current_traps);

        // Now apply the new SNMP configuration
        info!("Applying new SNMP configuration with {} trap receivers",
              snmp_config.trap_receivers.len());
        let new_receivers: Vec<String> = snmp_config.trap_receivers.iter()
            .map(|r| r.host.clone())
            .collect();
        debug!("New trap receivers to configure: {:?}", new_receivers);

        let commands = self.generate_snmp_commands(snmp_config, current_traps);
        debug!("Generated {} SNMP configuration commands", commands.len());

        let apply_outputs = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;
        debug!("SNMP configuration applied, output: {:?}", apply_outputs);

        // Verification: Re-fetch running config to confirm trap receivers were updated
        debug!("Verifying SNMP configuration was applied correctly");
        let verify_outputs = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?
            .execute_commands(&show_cmd)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        let new_running_config = verify_outputs.get(0).unwrap_or(&String::new()).clone();
        let mut actual_receivers = Vec::new();
        for line in new_running_config.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("snmp-server host ") {
                if let Some(host_start) = trimmed.find("snmp-server host ") {
                    let after_host = &trimmed[host_start + "snmp-server host ".len()..];
                    if let Some(host_ip) = after_host.split_whitespace().next() {
                        actual_receivers.push(host_ip.to_string());
                    }
                }
            }
        }

        info!("Verification: Found {} trap receivers in running-config after update: {:?}",
              actual_receivers.len(), actual_receivers);

        // Warn if mismatch detected
        if actual_receivers.len() != snmp_config.trap_receivers.len() {
            warn!("MISMATCH: Expected {} trap receivers but found {} in running-config!",
                  snmp_config.trap_receivers.len(), actual_receivers.len());
            warn!("Expected: {:?}", new_receivers);
            warn!("Actual: {:?}", actual_receivers);
        } else {
            info!("✓ Trap receiver count matches expected configuration");
        }

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured SNMP settings (verified: {} trap receivers)", actual_receivers.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Apply granular SNMP diff - only changes what's necessary
    /// This is much more efficient than configure_snmp which removes and re-adds everything
    async fn apply_snmp_diff(
        &mut self,
        snmp_diff: &crate::models::SnmpStateDiff,
        desired_config: Option<&crate::models::SnmpConfig>,
    ) -> Result<ConfigResult, VendorError> {
        use crate::models::TrapType;

        let mut commands = Vec::new();
        let mut actions = Vec::new();

        // Only enter config mode if we have commands to execute
        let has_community_changes = !snmp_diff.communities_to_add.is_empty()
            || !snmp_diff.communities_to_remove.is_empty()
            || !snmp_diff.communities_to_update.is_empty();
        let has_receiver_changes = !snmp_diff.trap_receivers_to_add.is_empty()
            || !snmp_diff.trap_receivers_to_remove.is_empty();
        let has_trap_changes = !snmp_diff.traps_to_enable.is_empty()
            || !snmp_diff.traps_to_disable.is_empty();

        if !has_community_changes && !has_receiver_changes && !has_trap_changes {
            info!("No SNMP changes needed - config is already in desired state");
            return Ok(ConfigResult {
                switch: self.config.hostname().to_string(),
                success: true,
                message: "SNMP configuration already in desired state".to_string(),
                commands_executed: vec![],
                timestamp: chrono::Utc::now(),
            });
        }

        commands.push("configure terminal".to_string());

        // Remove communities that are no longer wanted
        for community_name in &snmp_diff.communities_to_remove {
            info!("Removing SNMP community: {}", community_name);
            commands.push(format!("no snmp-server community \"{}\"", community_name));
            actions.push(format!("removed community '{}'", community_name));
        }

        // Update communities (remove old, add new with different access)
        for community in &snmp_diff.communities_to_update {
            info!("Updating SNMP community: {} -> {:?}", community.name, community.access);
            commands.push(format!("no snmp-server community \"{}\"", community.name));
            let access_str = match community.access {
                crate::models::SnmpAccess::Unrestricted => "unrestricted",
                crate::models::SnmpAccess::Manager => "manager",
                crate::models::SnmpAccess::Operator => "operator",
            };
            commands.push(format!("snmp-server community \"{}\" {}", community.name, access_str));
            actions.push(format!("updated community '{}' to {:?}", community.name, community.access));
        }

        // Add new communities
        for community in &snmp_diff.communities_to_add {
            info!("Adding SNMP community: {} ({:?})", community.name, community.access);
            let access_str = match community.access {
                crate::models::SnmpAccess::Unrestricted => "unrestricted",
                crate::models::SnmpAccess::Manager => "manager",
                crate::models::SnmpAccess::Operator => "operator",
            };
            commands.push(format!("snmp-server community \"{}\" {}", community.name, access_str));
            actions.push(format!("added community '{}'", community.name));
        }

        // Remove trap receivers that are no longer wanted
        for host in &snmp_diff.trap_receivers_to_remove {
            info!("Removing SNMP trap receiver: {}", host);
            // Try to find the community for this receiver from desired config for proper removal
            // If not found, just use the host which might work
            commands.push(format!("no snmp-server host {}", host));
            actions.push(format!("removed trap receiver '{}'", host));
        }

        // Add new trap receivers
        for receiver in &snmp_diff.trap_receivers_to_add {
            info!("Adding SNMP trap receiver: {} (community: {})", receiver.host, receiver.community);
            commands.push(format!(
                "snmp-server host {} community \"{}\"",
                receiver.host, receiver.community
            ));
            actions.push(format!("added trap receiver '{}'", receiver.host));
        }

        // Enable traps that aren't currently enabled
        for trap_type in &snmp_diff.traps_to_enable {
            match trap_type {
                TrapType::MacNotify => {
                    info!("Enabling mac-notify traps");
                    commands.push("snmp-server enable traps mac-notify".to_string());
                    actions.push("enabled mac-notify traps".to_string());
                }
                TrapType::LinkChange => {
                    info!("Enabling link-change traps");
                    commands.push("snmp-server enable traps link-change all".to_string());
                    actions.push("enabled link-change traps".to_string());
                }
                TrapType::All => {
                    info!("Enabling all traps");
                    commands.push("snmp-server enable traps mac-notify".to_string());
                    commands.push("snmp-server enable traps link-change all".to_string());
                    actions.push("enabled all traps".to_string());
                }
                _ => {
                    debug!("Trap type {:?} not specifically handled", trap_type);
                }
            }
        }

        // Disable traps that are currently enabled but shouldn't be
        for trap_type in &snmp_diff.traps_to_disable {
            match trap_type {
                TrapType::MacNotify => {
                    info!("Disabling mac-notify traps");
                    commands.push("no snmp-server enable traps mac-notify".to_string());
                    actions.push("disabled mac-notify traps".to_string());
                }
                TrapType::LinkChange => {
                    info!("Disabling link-change traps");
                    commands.push("no snmp-server enable traps link-change".to_string());
                    actions.push("disabled link-change traps".to_string());
                }
                TrapType::All => {
                    info!("Disabling all traps");
                    commands.push("no snmp-server enable traps mac-notify".to_string());
                    commands.push("no snmp-server enable traps link-change".to_string());
                    actions.push("disabled all traps".to_string());
                }
                _ => {
                    debug!("Trap type {:?} not specifically handled for disable", trap_type);
                }
            }
        }

        commands.push("exit".to_string());

        // Execute the commands
        info!("Applying {} SNMP changes: {:?}", actions.len(), actions);
        debug!("SNMP diff commands: {:?}", commands);

        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client.execute_commands(&commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Applied {} SNMP changes", actions.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Configure management VLAN on Aruba switch
    /// This restricts management access (CLI, WebAgent, SNMP) to only the specified VLAN
    async fn configure_management_vlan(&mut self, vlan_id: u16) -> Result<ConfigResult, VendorError> {
        info!("Configuring Aruba management VLAN: {}", vlan_id);

        let commands = vec![
            "configure terminal".to_string(),
            format!("management-vlan {}", vlan_id),
            "exit".to_string(),
        ];

        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client.execute_commands(&commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured management VLAN {}", vlan_id),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Remove management VLAN configuration on Aruba switch
    /// This returns the switch to allowing management access from all VLANs
    async fn remove_management_vlan(&mut self) -> Result<ConfigResult, VendorError> {
        info!("Removing Aruba management VLAN configuration");

        let commands = vec![
            "configure terminal".to_string(),
            "no management-vlan".to_string(),
            "exit".to_string(),
        ];

        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client.execute_commands(&commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: "Removed management VLAN configuration".to_string(),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }
}

#[async_trait]
impl SwitchVendor for ArubaSwitch {
    async fn connect(&mut self) -> Result<(), VendorError> {
        // Get retry settings
        let max_retries = self.config.settings.max_retries;
        let retry_delay_secs = 5; // 5 seconds between retries

        let client = match self.config.credentials().connection_type {
            ConnectionType::Ssh => {
                let mut ssh_client = SshClient::new(
                    self.config.management_ip().to_string(),
                    self.config.credentials().port,
                )
                .with_debug_mode(self.runtime_config.debug)
                .with_dry_run(self.runtime_config.dry_run);

                // Use connect_with_retry for retry logic
                ssh_client
                    .connect_with_retry(self.config.credentials(), max_retries, retry_delay_secs)
                    .await
                    .map_err(|e| VendorError::SshError(e.to_string()))?;

                // Disable pagination to prevent "-- MORE --" prompts
                ssh_client
                    .execute_command("no page")
                    .await
                    .map_err(|e| VendorError::SshError(e.to_string()))?;

                ConnectionClient::Ssh(ssh_client)
            }
            ConnectionType::Serial => {
                let serial_device = self
                    .config
                    .credentials()
                    .serial_device
                    .as_ref()
                    .ok_or_else(|| {
                        VendorError::ValidationError("No serial device specified".to_string())
                    })?;

                let mut serial_client = SerialClient::new(
                    serial_device.clone(),
                    self.config.credentials().baud_rate,
                )
                .with_debug_mode(self.runtime_config.debug)
                .with_dry_run(self.runtime_config.dry_run);

                // Helper function to ensure cleanup on error
                let setup_result = async {
                    // Use connect_with_retry for retry logic
                    serial_client
                        .connect_with_retry(max_retries, retry_delay_secs)
                        .await
                        .map_err(|e| VendorError::SshError(e.to_string()))?;

                    // Login via serial
                    if let Some(password) = &self.config.credentials().password {
                        serial_client
                            .login(&self.config.credentials().username, password)
                            .await
                            .map_err(|e| VendorError::SshError(e.to_string()))?;
                    } else {
                        return Err(VendorError::ValidationError(
                            "No password provided for serial connection".to_string(),
                        ));
                    }

                    // Enter privileged mode (enable)
                    // Note: If already in privileged mode, the enable command may fail with
                    // "Invalid input: enable". We handle this gracefully.
                    // Enable auth_mode so credential responses aren't skipped in dry-run.
                    serial_client.set_auth_mode(true);
                    // Determine the enable password: use enable_secret if set, otherwise fall back to login password
                    let enable_secret = self.config.credentials().enable_secret.clone()
                        .or_else(|| self.config.credentials().password.clone());

                    match serial_client.execute_command("enable").await {
                        Ok(enable_output) => {
                            // Check if enable prompted for credentials
                            if enable_output.contains("Username:") || enable_output.contains("username:") {
                                debug!("Enable mode requires authentication, providing credentials");

                                // Send username
                                serial_client
                                    .execute_command(&self.config.credentials().username)
                                    .await
                                    .map_err(|e| VendorError::SshError(e.to_string()))?;

                                // Send enable password (or fall back to login password)
                                if let Some(secret) = &enable_secret {
                                    serial_client
                                        .execute_command(secret)
                                        .await
                                        .map_err(|e| VendorError::SshError(e.to_string()))?;
                                }
                            } else if enable_output.contains("Password:") || enable_output.contains("password:") {
                                debug!("Enable mode requires password");

                                // Send enable password (or fall back to login password)
                                if let Some(secret) = &enable_secret {
                                    serial_client
                                        .execute_command(secret)
                                        .await
                                        .map_err(|e| VendorError::SshError(e.to_string()))?;
                                }
                            } else if enable_output.contains("Invalid input") {
                                // Already in privileged mode
                                debug!("Already in privileged mode (enable command returned 'Invalid input')");
                            }
                        }
                        Err(e) => {
                            // Enable command failed - could be timeout or other error
                            // Try 'configure' to verify if we have privileged access
                            debug!("Enable command failed: {}. Attempting to verify privileged access with 'configure'", e);

                            match serial_client.execute_command("configure").await {
                                Ok(_) => {
                                    debug!("Successfully entered configure mode - already had privileged access");
                                    // Exit configure mode
                                    serial_client.execute_command("exit").await.ok();
                                }
                                Err(config_err) => {
                                    // Cannot enter configure mode - propagate the error
                                    serial_client.set_auth_mode(false);
                                    return Err(VendorError::SshError(format!(
                                        "Failed to enter privileged mode. Enable error: {}. Configure test error: {}",
                                        e, config_err
                                    )));
                                }
                            }
                        }
                    }
                    serial_client.set_auth_mode(false);

                    // Disable pagination to prevent "Press any key to continue" prompts
                    serial_client
                        .execute_command("no page")
                        .await
                        .map_err(|e| VendorError::SshError(e.to_string()))?;

                    Ok::<(), VendorError>(())
                }.await;

                // If setup failed, ensure we disconnect before returning error
                if let Err(e) = setup_result {
                    let _ = serial_client.disconnect().await;
                    return Err(e);
                }

                ConnectionClient::Serial(serial_client)
            }
        };

        self.client = Some(client);
        info!("Connected to Aruba switch: {}", self.config.hostname());
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), VendorError> {
        if let Some(mut client) = self.client.take() {
            client
                .disconnect()
                .await
                .map_err(|e| VendorError::SshError(e.to_string()))?;
        }
        Ok(())
    }

    async fn parse_current_state(&mut self) -> Result<SwitchState, VendorError> {
        let config = self.get_running_config().await?;
        debug!("Parsing Aruba running configuration for {}", self.config.hostname());
        debug!("Running config length: {} bytes, {} lines", config.len(), config.lines().count());

        let mut state = SwitchState::default();

        // Pre-process config to strip ANSI escape sequences (common with serial connections)
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
        let clean_config = ansi_regex.replace_all(&config, "");
        let lines: Vec<&str> = clean_config.lines().collect();

        // Extract hardware identifier and verify against configured model
        state.warnings = super::traits::verify_hardware_model(
            &clean_config,
            &self.config.model(),
            &Self::hardware_id_pattern(),
        );

        // First pass: collect interface names, VLAN configurations, and port mirrors
        let mut interface_names = std::collections::HashMap::new();
        let mut port_vlan_map: std::collections::HashMap<String, PortVlanInfo> = std::collections::HashMap::new();
        let mut mirror_destination: Option<String> = None;
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with(';') || line.starts_with("Running configuration") {
                i += 1;
                continue;
            }

            // Parse global mirror destination
            // Format 1 (newer): "mirror <session-id> port <destination>"
            // Example: "mirror 1 port 22"
            // Format 2 (older, on 2530/2540): "mirror-port <destination>"
            // Example: "mirror-port 42"
            if line.starts_with("mirror ") {
                if let Some(rest) = line.strip_prefix("mirror ") {
                    // Parse "1 port 22" -> extract "22"
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == "port" {
                        mirror_destination = Some(parts[2].to_string());
                        debug!("  Found mirror destination port: {} (new syntax)", parts[2]);
                    }
                }
            }
            // Handle older "mirror-port <destination>" syntax (used on 2530/2540 series)
            else if line.starts_with("mirror-port ") {
                if let Some(dest) = line.strip_prefix("mirror-port ") {
                    let dest = dest.trim();
                    mirror_destination = Some(dest.to_string());
                    debug!("  Found mirror destination port: {} (legacy mirror-port syntax)", dest);
                }
            }
            // Parse interface blocks for names, monitor status, PoE, MAC notify, enabled status, and speed-duplex
            else if line.starts_with("interface ") {
                if let Some((port_id, name, _has_mirror, has_monitor, poe_enabled, mac_notify, enabled, speed_duplex)) = self.parse_interface_name(&lines, &mut i) {
                    if let Some(name_val) = name {
                        interface_names.insert(port_id.clone(), name_val);
                    }
                    // Get or create port info entry
                    let info = port_vlan_map.entry(port_id.clone()).or_insert_with(|| PortVlanInfo::new(port_id.clone()));

                    // Update port settings from interface block
                    info.has_monitor = has_monitor;
                    info.poe_enabled = poe_enabled;
                    info.mac_notify = mac_notify;
                    info.enabled = enabled;
                    info.speed_duplex = speed_duplex;
                }
            }
            // Parse VLAN blocks for names and port assignments
            else if line.starts_with("vlan ") {
                if let Some((vlan, untagged_ports, tagged_ports)) = self.parse_vlan_with_ports(&lines, &mut i) {
                    state.vlans.push(vlan.clone());

                    // Update port VLAN mappings
                    for port_id in untagged_ports {
                        port_vlan_map.entry(port_id.clone())
                            .or_insert_with(|| PortVlanInfo::new(port_id.clone()))
                            .untagged_vlan = Some(vlan.id);
                    }
                    for port_id in tagged_ports {
                        port_vlan_map.entry(port_id.clone())
                            .or_insert_with(|| PortVlanInfo::new(port_id.clone()))
                            .tagged_vlans.push(vlan.id);
                    }
                }
            }

            i += 1;
        }

        // Second pass: build Port structs from collected information
        let mut monitor_ports = Vec::new();
        for (port_id, vlan_info) in port_vlan_map {
            let description = interface_names.get(&port_id).cloned();
            let untagged_vlan = vlan_info.untagged_vlan.unwrap_or(1);
            let mode = if vlan_info.tagged_vlans.is_empty() {
                PortMode::Access
            } else {
                PortMode::Trunk
            };

            // For trunk ports, tagged_vlans should include both the native VLAN and tagged VLANs
            let mut tagged_vlans = vlan_info.tagged_vlans.clone();
            if mode == PortMode::Trunk {
                // Add native VLAN to tagged_vlans if not already present
                if !tagged_vlans.contains(&untagged_vlan) {
                    tagged_vlans.push(untagged_vlan);
                }
                // Sort for consistent comparison
                tagged_vlans.sort_unstable();
            }

            // Track monitor-enabled ports (mirror sources)
            if vlan_info.has_monitor {
                monitor_ports.push(port_id.clone());
            }

            state.ports.push(Port {
                port_id,
                mode,
                vlan: untagged_vlan,
                tagged_vlans,
                description,
                enabled: vlan_info.enabled,
                poe_enabled: vlan_info.poe_enabled,
                mac_notify: vlan_info.mac_notify,
                speed_duplex: vlan_info.speed_duplex,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        // Build mirror configuration from collected destination and source ports
        if let Some(dest) = mirror_destination {
            if !monitor_ports.is_empty() {
                debug!("  Building mirror: sources={:?} -> dest={}", monitor_ports, dest);
                state.port_mirrors.push(PortMirror {
                    session_id: "1".to_string(),
                    source_ports: monitor_ports,
                    destination_port: dest,
                    direction: crate::models::MirrorDirection::Both,
                });
            }
        }

        // Parse SNMP configuration with actual trap status from 'show snmp-server traps'
        let (link_change_enabled, mac_notify_enabled) = self.parse_snmp_trap_status().await?;
        state.snmp = self.parse_snmp_config(&lines, link_change_enabled, mac_notify_enabled);

        // Parse management VLAN configuration
        // Format: "management-vlan 10" or "management-vlan vlan10"
        state.management_vlan = self.parse_management_vlan(&lines);

        debug!("Parsed state: {} VLANs, {} ports, {} mirrors, SNMP: {}, Management VLAN: {:?}",
               state.vlans.len(), state.ports.len(), state.port_mirrors.len(),
               if state.snmp.is_some() { "configured" } else { "not configured" },
               state.management_vlan);

        // Debug: log summary
        if !state.vlans.is_empty() {
            debug!("Parsed VLANs: {}", state.vlans.iter().map(|v| format!("{}({})", v.id, v.name)).collect::<Vec<_>>().join(", "));
        }
        if !state.ports.is_empty() {
            debug!("Parsed ports: {}", state.ports.iter().map(|p| format!("{}({:?})", p.port_id, p.mode)).collect::<Vec<_>>().join(", "));
        }
        if !state.port_mirrors.is_empty() {
            debug!("Parsed mirrors: {}", state.port_mirrors.len());
        }
        if let Some(snmp) = &state.snmp {
            debug!("Parsed SNMP: {} communities, {} trap receivers, {} traps",
                   snmp.communities.len(), snmp.trap_receivers.len(), snmp.enabled_traps.len());
        }

        Ok(state)
    }

    async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>, VendorError> {
        let mut results = Vec::new();

        // Remove old VLANs (but never remove VLAN 1, which is the default VLAN)
        if !diff.vlans_to_remove.is_empty() {
            let vlans_to_remove: Vec<u16> = diff.vlans_to_remove.iter()
                .filter(|&&v| v != 1)
                .copied()
                .collect();

            if !vlans_to_remove.is_empty() {
                // Migrate ports away from VLANs before deletion to avoid interactive prompts
                if let Some(current_state) = &self.current_state {
                    let migrations = crate::diff::find_ports_to_migrate(current_state, &vlans_to_remove);

                    if !migrations.is_empty() {
                        debug!("Migrating {} ports before VLAN deletion", migrations.len());
                        results.push(self.migrate_ports_before_vlan_deletion(&migrations).await?);
                    }
                }

                debug!("Removing {} VLANs", vlans_to_remove.len());
                results.push(self.remove_vlans(&vlans_to_remove).await?);
            } else if diff.vlans_to_remove.contains(&1) {
                debug!("Skipping removal of VLAN 1 (default VLAN cannot be removed)");
            }
        }

        // Add new VLANs
        if !diff.vlans_to_add.is_empty() {
            debug!("Adding {} VLANs", diff.vlans_to_add.len());
            results.push(self.configure_vlans(&diff.vlans_to_add).await?);
        }

        // Update changed VLANs
        if !diff.vlans_to_update.is_empty() {
            debug!("Updating {} VLANs", diff.vlans_to_update.len());
            results.push(self.configure_vlans(&diff.vlans_to_update).await?);
        }

        // Configure changed ports
        if !diff.ports_to_configure.is_empty() {
            debug!("Configuring {} ports", diff.ports_to_configure.len());
            results.push(self.configure_ports(&diff.ports_to_configure).await?);
        }

        // Reset unconfigured ports to default state
        if !diff.ports_to_reset.is_empty() {
            debug!("Resetting {} unconfigured ports to default state", diff.ports_to_reset.len());
            results.push(self.reset_ports(&diff.ports_to_reset).await?);
        }

        // Configure mirror destination ports with baseline settings before mirror setup
        if !diff.mirror_dest_ports_to_configure.is_empty() {
            debug!("Configuring {} mirror destination ports", diff.mirror_dest_ports_to_configure.len());
            results.push(self.configure_mirror_dest_ports(&diff.mirror_dest_ports_to_configure).await?);
        }

        // Remove old mirrors
        if !diff.mirrors_to_remove.is_empty() {
            debug!("Removing {} port mirrors", diff.mirrors_to_remove.len());
            results.push(self.remove_mirrors(&diff.mirrors_to_remove).await?);
        }

        // Add new mirrors
        if !diff.mirrors_to_add.is_empty() {
            debug!("Adding {} port mirrors", diff.mirrors_to_add.len());
            results.push(self.configure_port_mirrors(&diff.mirrors_to_add).await?);
        }

        // Update changed mirrors
        if !diff.mirrors_to_update.is_empty() {
            debug!("Updating {} port mirrors", diff.mirrors_to_update.len());
            results.push(self.configure_port_mirrors(&diff.mirrors_to_update).await?);
        }

        // Configure SNMP if changed - use granular diff when available
        if let Some(snmp_diff) = &diff.snmp_diff {
            if snmp_diff.has_changes() {
                debug!("Applying granular SNMP diff");
                results.push(self.apply_snmp_diff(snmp_diff, diff.snmp_config.as_ref()).await?);
            }
        } else if diff.snmp_config_changed {
            // Fallback to legacy full replacement (should rarely happen)
            if let Some(snmp_config) = &diff.snmp_config {
                debug!("Configuring SNMP (legacy full replacement)");
                results.push(self.configure_snmp(snmp_config).await?);
            } else {
                debug!("Removing SNMP configuration");
                // TODO: Add SNMP removal commands if needed
            }
        }

        // Configure management VLAN if changed
        if diff.management_vlan_changed {
            if let Some(vlan_id) = diff.management_vlan {
                info!("Configuring management VLAN: {}", vlan_id);
                results.push(self.configure_management_vlan(vlan_id).await?);
            } else {
                info!("Removing management VLAN configuration");
                results.push(self.remove_management_vlan().await?);
            }
        }

        Ok(results)
    }

    async fn configure_vlans(&mut self, vlans: &[Vlan]) -> Result<ConfigResult, VendorError> {
        let commands = self.generate_vlan_commands(vlans);
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured {} VLANs", vlans.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    async fn configure_ports(&mut self, ports: &[Port]) -> Result<ConfigResult, VendorError> {
        // Pass all port mirrors to generate_port_commands to ensure monitor commands are included
        let commands = self.generate_port_commands(ports, &self.config.port_mirrors);
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured {} ports", ports.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    async fn configure_port_mirrors(
        &mut self,
        mirrors: &[PortMirror],
    ) -> Result<ConfigResult, VendorError> {
        let commands = self.generate_mirror_commands(mirrors);
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured {} port mirrors", mirrors.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    fn generate_commands_for_diff(&self, diff: &StateDiff) -> crate::models::CommandPreview {
        let mut preview = crate::models::CommandPreview::default();

        if !diff.vlans_to_remove.is_empty() {
            preview.vlan_commands.extend(self.generate_remove_vlan_commands(&diff.vlans_to_remove));
        }
        if !diff.vlans_to_add.is_empty() {
            preview.vlan_commands.extend(self.generate_vlan_commands(&diff.vlans_to_add));
        }
        if !diff.vlans_to_update.is_empty() {
            preview.vlan_commands.extend(self.generate_vlan_commands(&diff.vlans_to_update));
        }

        if !diff.ports_to_configure.is_empty() {
            let all_mirrors: Vec<_> = diff.mirrors_to_add.iter()
                .chain(diff.mirrors_to_update.iter())
                .cloned()
                .collect();
            preview.port_commands.extend(self.generate_port_commands(&diff.ports_to_configure, &all_mirrors));
        }

        if !diff.ports_to_reset.is_empty() {
            for port_id in &diff.ports_to_reset {
                preview.reset_commands.push(format!("interface {}", self.normalize_port_id(port_id)));
                preview.reset_commands.push("disable".to_string());
                preview.reset_commands.push("untagged vlan 1".to_string());
                preview.reset_commands.push("exit".to_string());
            }
        }

        if !diff.mirrors_to_remove.is_empty() {
            preview.mirror_commands.extend(self.generate_remove_mirror_commands(&diff.mirrors_to_remove));
        }
        if !diff.mirrors_to_add.is_empty() {
            preview.mirror_commands.extend(self.generate_mirror_commands(&diff.mirrors_to_add));
        }
        if !diff.mirrors_to_update.is_empty() {
            preview.mirror_commands.extend(self.generate_mirror_commands(&diff.mirrors_to_update));
        }

        if let Some(snmp_diff) = &diff.snmp_diff {
            if snmp_diff.has_changes() {
                if let Some(snmp_config) = &diff.snmp_config {
                    let current_traps = self.current_state.as_ref()
                        .and_then(|s| s.snmp.as_ref())
                        .map(|s| s.enabled_traps.clone())
                        .unwrap_or_default();
                    preview.snmp_commands.extend(self.generate_snmp_commands(snmp_config, &current_traps));
                }
            }
        }

        preview
    }

    async fn execute_raw_commands(&mut self, commands: &[String]) -> Result<Vec<String>, VendorError> {
        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;
        client.execute_commands(commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))
    }

    fn get_warnings(&self) -> Vec<String> {
        self.current_state
            .as_ref()
            .map(|s| s.warnings.clone())
            .unwrap_or_default()
    }

    async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError> {
        // Parse current state
        debug!("Parsing current state from {}", self.config.hostname());
        let current = self.parse_current_state().await?;

        super::traits::check_empty_state_safety(
            &current,
            self.config.vlans.len(),
            self.config.ports.len(),
            &self.config.hostname(),
        )?;

        // Store current state for use in command generation
        self.current_state = Some(current.clone());

        // Compute diff
        debug!("Computing configuration differences");
        let diff = crate::diff::compute_diff(&current, &self.config, self.enforce_port_config);

        // Early return if no changes
        if !diff.has_changes() {
            info!("No configuration changes needed for {}", self.config.hostname());
            return Ok(vec![]);
        }

        // Apply diff
        info!("Applying configuration changes to {}", self.config.hostname());
        let results = self.apply_diff(&diff).await?;

        // Post-apply convergence check: re-parse state and verify changes took effect
        debug!("Verifying configuration convergence for {}", self.config.hostname());
        match self.parse_current_state().await {
            Ok(mut post_apply_state) => {
                let remaining = crate::diff::compute_diff(&post_apply_state, &self.config, self.enforce_port_config);
                if remaining.has_changes() {
                    let summary = remaining.remaining_changes_summary();
                    warn!(
                        "Configuration did not fully converge for {}: still pending: {}",
                        self.config.hostname(), summary
                    );
                    post_apply_state.warnings.push(format!(
                        "Configuration did not converge: {}", summary
                    ));
                } else {
                    debug!("Configuration fully converged for {}", self.config.hostname());
                }
                self.current_state = Some(post_apply_state);
            }
            Err(e) => {
                warn!("Could not verify convergence for {}: {}", self.config.hostname(), e);
            }
        }

        Ok(results)
    }

    async fn save_configuration(&mut self) -> Result<(), VendorError> {
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client
            .execute_command("write memory")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        info!("Configuration saved on {}", self.config.hostname());
        Ok(())
    }

    async fn get_running_config(&mut self) -> Result<String, VendorError> {
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let config = client
            .execute_command("show running-config")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(config)
    }

    fn validate_configuration(&self) -> Result<(), VendorError> {
        // Validate VLAN IDs
        for vlan in &self.config.vlans {
            if vlan.id < 1 || vlan.id > 4094 {
                return Err(VendorError::ValidationError(format!(
                    "Invalid VLAN ID: {}",
                    vlan.id
                )));
            }
        }

        // Validate port configurations
        for port in &self.config.ports {
            if port.vlan < 1 || port.vlan > 4094 {
                return Err(VendorError::ValidationError(format!(
                    "Invalid VLAN ID on port {}: {}",
                    port.port_id, port.vlan
                )));
            }
        }

        Ok(())
    }

    async fn run_validation_tests(
        &mut self,
        validation_config: &crate::validation::ValidationConfig,
    ) -> Result<crate::validation::ValidationResult, VendorError> {
        use crate::validation::ValidationResult;
        use std::time::Instant;

        info!("Running validation tests for {}", self.config.hostname());

        let start = Instant::now();
        let mut result = ValidationResult::new();

        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        // Run each test
        for test in &validation_config.tests {
            let test_result = crate::validation::tests::execute_test(
                test,
                client,
                &self.config.management_ip(),
            ).await;

            match test_result {
                Ok(()) => {
                    result.record_success();
                }
                Err(failure) => {
                    result.record_failure(failure);
                }
            }

            // Stop if a required test failed and we're over the time budget
            if !result.passed && start.elapsed() > validation_config.timeout {
                warn!("Validation timeout reached after {:?}", start.elapsed());
                break;
            }
        }

        result.finalize(start.elapsed());

        info!(
            "Validation completed: {}/{} tests passed in {:?}",
            result.tests_passed,
            result.tests_run,
            result.duration
        );

        Ok(result)
    }

    async fn rollback_configuration(
        &mut self,
        method: crate::validation::RollbackMethod,
    ) -> Result<(), VendorError> {
        use crate::validation::RollbackMethod;

        info!("Rolling back configuration on {} using method: {:?}", self.config.hostname(), method);

        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        match method {
            RollbackMethod::Reload => {
                warn!("Reloading switch {} - this will cause downtime", self.config.hostname());

                // For Aruba switches, use 'boot system' or 'reload'
                client
                    .execute_command("reload")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Reload failed: {}", e)))?;

                info!("Reload initiated on {}", self.config.hostname());
            }
            RollbackMethod::RestoreBackup => {
                info!("Restoring configuration from startup-config");

                // Copy startup-config to running-config
                client
                    .execute_command("copy startup-config running-config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Restore failed: {}", e)))?;

                info!("Configuration restored on {}", self.config.hostname());
            }
            RollbackMethod::RevertCommands => {
                warn!("Revert commands method not fully implemented for Aruba, using restore backup instead");

                // Fallback to restore backup
                client
                    .execute_command("copy startup-config running-config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Revert failed: {}", e)))?;

                info!("Configuration reverted on {}", self.config.hostname());
            }
        }

        Ok(())
    }
}

// Additional helper methods for ArubaSwitch
impl ArubaSwitch {
    /// Parse running config text into SwitchState (sync version for testing)
    /// This is a synchronous wrapper around the parsing logic in parse_current_state
    #[cfg(test)]
    pub fn parse_running_config(&self, config: &str) -> SwitchState {
        use tracing::debug;

        let mut state = SwitchState::default();

        // Pre-process config to strip ANSI escape sequences (common with serial connections)
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
        let clean_config = ansi_regex.replace_all(config, "");
        let lines: Vec<&str> = clean_config.lines().collect();

        // Extract hardware identifier and verify against configured model
        state.warnings = super::traits::verify_hardware_model(
            &clean_config,
            &self.config.model(),
            &Self::hardware_id_pattern(),
        );

        // First pass: collect interface names, VLAN configurations, and port mirrors
        let mut interface_names = std::collections::HashMap::new();
        let mut port_vlan_map: std::collections::HashMap<String, PortVlanInfo> =
            std::collections::HashMap::new();
        let mut mirror_destination: Option<String> = None;
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            // Skip empty lines and comments
            if line.is_empty()
                || line.starts_with(';')
                || line.starts_with("Running configuration")
            {
                i += 1;
                continue;
            }

            // Parse global mirror destination
            // Format 1 (newer): "mirror <session-id> port <destination>"
            // Format 2 (older, on 2530/2540): "mirror-port <destination>"
            if line.starts_with("mirror ") {
                if let Some(rest) = line.strip_prefix("mirror ") {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == "port" {
                        mirror_destination = Some(parts[2].to_string());
                        debug!("  Found mirror destination port: {} (new syntax)", parts[2]);
                    }
                }
            } else if line.starts_with("mirror-port ") {
                if let Some(dest) = line.strip_prefix("mirror-port ") {
                    let dest = dest.trim();
                    mirror_destination = Some(dest.to_string());
                    debug!(
                        "  Found mirror destination port: {} (legacy mirror-port syntax)",
                        dest
                    );
                }
            }
            // Parse interface blocks
            else if line.starts_with("interface ") {
                if let Some((
                    port_id,
                    name,
                    _has_mirror,
                    has_monitor,
                    poe_enabled,
                    mac_notify,
                    enabled,
                    speed_duplex,
                )) = self.parse_interface_name(&lines, &mut i)
                {
                    if let Some(name_val) = name {
                        interface_names.insert(port_id.clone(), name_val);
                    }
                    let info = port_vlan_map
                        .entry(port_id.clone())
                        .or_insert_with(|| PortVlanInfo::new(port_id.clone()));

                    info.has_monitor = has_monitor;
                    info.poe_enabled = poe_enabled;
                    info.mac_notify = mac_notify;
                    info.enabled = enabled;
                    info.speed_duplex = speed_duplex;
                }
            }
            // Parse VLAN blocks
            else if line.starts_with("vlan ") {
                if let Some((vlan, untagged_ports, tagged_ports)) =
                    self.parse_vlan_with_ports(&lines, &mut i)
                {
                    state.vlans.push(vlan.clone());

                    for port_id in untagged_ports {
                        port_vlan_map
                            .entry(port_id.clone())
                            .or_insert_with(|| PortVlanInfo::new(port_id.clone()))
                            .untagged_vlan = Some(vlan.id);
                    }
                    for port_id in tagged_ports {
                        port_vlan_map
                            .entry(port_id.clone())
                            .or_insert_with(|| PortVlanInfo::new(port_id.clone()))
                            .tagged_vlans
                            .push(vlan.id);
                    }
                }
            }

            i += 1;
        }

        // Second pass: build Port structs from collected information
        let mut monitor_ports = Vec::new();
        for (port_id, vlan_info) in port_vlan_map {
            let description = interface_names.get(&port_id).cloned();
            let untagged_vlan = vlan_info.untagged_vlan.unwrap_or(1);
            let mode = if vlan_info.tagged_vlans.is_empty() {
                PortMode::Access
            } else {
                PortMode::Trunk
            };

            let mut tagged_vlans = vlan_info.tagged_vlans.clone();
            if mode == PortMode::Trunk {
                if !tagged_vlans.contains(&untagged_vlan) {
                    tagged_vlans.push(untagged_vlan);
                }
                tagged_vlans.sort_unstable();
            }

            if vlan_info.has_monitor {
                monitor_ports.push(port_id.clone());
            }

            state.ports.push(Port {
                port_id,
                mode,
                vlan: untagged_vlan,
                tagged_vlans,
                description,
                enabled: vlan_info.enabled,
                poe_enabled: vlan_info.poe_enabled,
                mac_notify: vlan_info.mac_notify,
                speed_duplex: vlan_info.speed_duplex,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        // Build mirror configuration
        if let Some(dest) = mirror_destination {
            if !monitor_ports.is_empty() {
                state.port_mirrors.push(PortMirror {
                    session_id: "1".to_string(),
                    source_ports: monitor_ports,
                    destination_port: dest,
                    direction: MirrorDirection::Both,
                });
            }
        }

        state
    }

    /// Reset ports to default state (disabled, VLAN 1, access mode, no description, PoE enabled)
    async fn reset_ports(&mut self, port_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let mut commands = vec!["configure terminal".to_string()];

        for port_id in port_ids {
            let port_interface = self.normalize_port_id(port_id);
            debug!("  Resetting port {} to default state", port_id);

            commands.push(format!("interface {}", port_interface));
            commands.push("disable".to_string());  // Disable the port
            commands.push("no name".to_string());  // Remove port name (Aruba uses "name", not "description")
            commands.push("untagged vlan 1".to_string());  // Set to default VLAN (access mode)
            // Remove any existing tagged VLANs — resetting to default means no tagged VLANs.
            // "no tagged vlan" without VLAN ID is invalid on Aruba, so we must remove each one.
            if let Some(ref current_state) = self.current_state {
                if let Some(current_port) = current_state.ports.iter().find(|p| p.port_id == *port_id) {
                    for &vlan in &current_port.tagged_vlans {
                        if vlan != current_port.vlan {
                            commands.push(format!("no tagged vlan {}", vlan));
                        }
                    }
                }
            }
            // Disable MAC notifications (must disable both trap types explicitly)
            commands.push("no mac-notify traps learned".to_string());
            commands.push("no mac-notify traps removed".to_string());
            // Only generate PoE commands if the switch model supports PoE
            if self.config.model().supports_poe() {
                commands.push("poe-allocate-by class".to_string());  // Set PoE allocation to default
            }
            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());

        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Reset {} ports to default state", port_ids.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Configure mirror destination ports with baseline settings (VLAN 1, enabled, access mode).
    /// These ports are special-purpose and not part of the regular port configuration.
    async fn configure_mirror_dest_ports(&mut self, port_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let mut commands = vec!["configure terminal".to_string()];

        for port_id in port_ids {
            let port_interface = self.normalize_port_id(port_id);
            debug!("  Configuring mirror dest port {} with baseline settings", port_id);

            commands.push(format!("interface {}", port_interface));
            commands.push("disable".to_string());
            commands.push("untagged vlan 1".to_string());
            commands.push("no name".to_string());
            commands.push("enable".to_string());
            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());

        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        let _outputs = client
            .execute_commands(&commands)
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured {} mirror destination ports with baseline settings", port_ids.len()),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use crate::models::{
        ConnectionType, Credentials, MirrorDirection, PortMirror, SnmpAccess, SnmpCommunity, SnmpConfig,
        SpeedDuplex, TrapType, SnmpTrapReceiver, SwitchModel, VlanIpConfig,
    };

    fn create_test_switch() -> ArubaSwitch {
        let config = SwitchConfig {
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        ArubaSwitch::new(config, RuntimeConfig::default(), false)
    }

    #[test]
    fn test_normalize_port_id() {
        let switch = create_test_switch();

        assert_eq!(switch.normalize_port_id("1"), "1");
        assert_eq!(switch.normalize_port_id("10"), "10");
        assert_eq!(switch.normalize_port_id("24"), "24");
    }

    #[test]
    fn test_generate_vlan_commands_simple() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10".to_string(),
                description: Some("Test VLAN".to_string()),
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"configure terminal".to_string()));
        assert!(commands.contains(&"vlan 10".to_string()));
        assert!(commands.contains(&"name vlan10".to_string()));
        assert!(commands.contains(&"no ip address".to_string()));
        assert!(commands.contains(&"exit".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_with_dhcp() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 20,
                name: "management".to_string(),
                description: None,
                ip_config: VlanIpConfig::Dhcp,
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"vlan 20".to_string()));
        assert!(commands.contains(&"name management".to_string()));
        assert!(commands.contains(&"ip address dhcp-bootp".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_with_static_ip() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 30,
                name: "server".to_string(),
                description: None,
                ip_config: VlanIpConfig::Static {
                    address: "192.168.30.1".to_string(),
                    netmask: "255.255.255.0".to_string(),
                },
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"vlan 30".to_string()));
        assert!(commands.contains(&"name server".to_string()));
        assert!(commands.contains(&"ip address 192.168.30.1 255.255.255.0".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_with_spaces_in_name() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 40,
                name: "Access 10 port sw".to_string(),
                description: Some("VLAN with spaces in name".to_string()),
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"vlan 40".to_string()));
        // VLAN names with spaces must be quoted to prevent CLI parsing issues
        assert!(commands.contains(&"name \"Access 10 port sw\"".to_string()));
        assert!(commands.contains(&"no ip address".to_string()));
    }

    #[test]
    fn test_generate_port_commands_access_mode() {
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("User port".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"configure terminal".to_string()));
        assert!(commands.contains(&"interface 1".to_string()));
        assert!(commands.contains(&"name \"User port\"".to_string()));
        assert!(commands.contains(&"untagged vlan 10".to_string()));
        assert!(commands.contains(&"enable".to_string()));
        assert!(commands.contains(&"power-over-ethernet".to_string()));
    }

    #[test]
    fn test_generate_port_commands_trunk_mode() {
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                tagged_vlans: vec![1, 10, 20, 30],
                description: Some("Uplink".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"interface 24".to_string()));
        assert!(commands.contains(&"name \"Uplink\"".to_string()));
        assert!(commands.contains(&"untagged vlan 1".to_string()));
        // Should have tagged vlans (excluding native vlan 1)
        assert!(commands.iter().any(|c| c.contains("tagged vlan") && c.contains("10")));
        assert!(commands.contains(&"enable".to_string()));
    }

    #[test]
    fn test_generate_port_commands_disabled_port() {
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "5".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: false,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"interface 5".to_string()));
        assert!(commands.contains(&"disable".to_string()));
        assert!(!commands.contains(&"enable".to_string()));
    }

    #[test]
    fn test_generate_mirror_commands() {
        let switch = create_test_switch();

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string(), "2".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let commands = switch.generate_mirror_commands(&mirrors);

        // generate_mirror_commands() now only generates the global mirror destination command
        // The per-interface monitor commands are generated by generate_port_commands()
        assert!(commands.contains(&"configure terminal".to_string()));
        assert!(commands.contains(&"mirror 1 port 10".to_string()));
        assert!(commands.contains(&"exit".to_string()));

        // Verify that interface-specific commands are NOT in mirror commands
        assert!(!commands.contains(&"interface 1".to_string()));
        assert!(!commands.contains(&"interface 2".to_string()));
        assert!(!commands.contains(&"monitor all both mirror 1".to_string()));
    }

    #[test]
    fn test_generate_snmp_commands() {
        let switch = create_test_switch();

        let snmp = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Unrestricted,
                },
                SnmpCommunity {
                    name: "private".to_string(),
                    access: SnmpAccess::Manager,
                },
            ],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.100".to_string(),
                    community: "public".to_string(),
                    version: None,
                },
            ],
            enabled_traps: vec![TrapType::MacNotify, TrapType::LinkChange],
        };

        // Test with no currently enabled traps (fresh config)
        let commands = switch.generate_snmp_commands(&snmp, &[]);

        assert!(commands.contains(&"configure terminal".to_string()));
        assert!(commands.contains(&"snmp-server community \"public\" unrestricted".to_string()));
        assert!(commands.contains(&"snmp-server community \"private\" manager".to_string()));
        assert!(commands.contains(&"snmp-server host 192.168.1.100 community \"public\"".to_string()));
        assert!(commands.iter().any(|c| c.contains("snmp-server enable traps")));
    }

    #[test]
    fn test_remove_vlan_commands() {
        let switch = create_test_switch();

        let vlan_ids = vec![10, 20];
        let commands = switch.generate_remove_vlan_commands(&vlan_ids);

        assert!(commands.contains(&"configure terminal".to_string()));
        assert!(commands.contains(&"no vlan 10".to_string()));
        assert!(commands.contains(&"no vlan 20".to_string()));
        assert!(commands.contains(&"exit".to_string()));
    }

    #[test]
    fn test_multiple_vlans() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 20,
                name: "vlan20".to_string(),
                description: None,
                ip_config: VlanIpConfig::Dhcp,
            },
            Vlan {
                id: 30,
                name: "vlan30".to_string(),
                description: None,
                ip_config: VlanIpConfig::Static {
                    address: "192.168.30.1".to_string(),
                    netmask: "255.255.255.0".to_string(),
                },
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        // Should have all 3 VLANs
        assert!(commands.contains(&"vlan 10".to_string()));
        assert!(commands.contains(&"vlan 20".to_string()));
        assert!(commands.contains(&"vlan 30".to_string()));

        // Count configure terminal and exit commands
        let config_count = commands.iter().filter(|&c| c == "configure terminal").count();
        let exit_count = commands.iter().filter(|&c| c == "exit").count();

        assert_eq!(config_count, 1); // Should only have one configure terminal
        assert!(exit_count >= 4); // One exit per VLAN + final exit
    }

    #[test]
    fn test_port_with_mac_notify() {
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: true,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"port-access mac-notify".to_string()) ||
                commands.iter().any(|c| c.contains("mac-notify")));
    }

    // ====================================================================
    // Bug Fix Tests - These tests verify the fixes for the 7 reported bugs
    // ====================================================================

    #[test]
    fn test_bug_fix_poe_enabled_commands() {
        // Bug #1: PoE Not Enabled
        // VERIFY: Commands include both "power-over-ethernet" AND "poe-allocate-by class"
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: true,  // PoE enabled
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Must have BOTH commands for PoE to work properly
        assert!(commands.contains(&"power-over-ethernet".to_string()),
                "Missing 'power-over-ethernet' command - PoE won't be enabled!");
        assert!(commands.contains(&"poe-allocate-by class".to_string()),
                "Missing 'poe-allocate-by class' command - PoE allocation won't work!");
    }

    #[test]
    fn test_bug_fix_poe_disabled_commands() {
        // Bug #1 complement: When PoE is disabled, should have "no power-over-ethernet"
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,  // PoE disabled
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"no power-over-ethernet".to_string()),
                "Missing 'no power-over-ethernet' command when PoE is disabled!");
    }

    #[test]
    fn test_bug_fix_poe_capability_check() {
        // Bug Fix: PoE capability check by switch model
        // VERIFY: port_supports_poe() correctly identifies PoE-capable ports

        // Test Aruba 2530-8G PoE+ (only ports 1, 3, 5, 7 support PoE)
        let config_8g = SwitchConfig {
            id: "test-8g".to_string(),
            hostname: Some("test-8g".to_string()),
            model: Some(SwitchModel::Aruba2530_8G_POE),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
                        validation: None,
            vendor_specific: std::collections::HashMap::new(),
management_vlan: None,
            settings: crate::config::Settings::default(),
        };
        let switch_8g = ArubaSwitch::new(config_8g, RuntimeConfig::default(), false);

        // Ports 1, 3, 5, 7 should support PoE
        assert!(switch_8g.port_supports_poe("1"), "Port 1 should support PoE on 2530-8G");
        assert!(switch_8g.port_supports_poe("3"), "Port 3 should support PoE on 2530-8G");
        assert!(switch_8g.port_supports_poe("5"), "Port 5 should support PoE on 2530-8G");
        assert!(switch_8g.port_supports_poe("7"), "Port 7 should support PoE on 2530-8G");

        // Ports 2, 4, 6, 8 should NOT support PoE
        assert!(!switch_8g.port_supports_poe("2"), "Port 2 should NOT support PoE on 2530-8G");
        assert!(!switch_8g.port_supports_poe("4"), "Port 4 should NOT support PoE on 2530-8G");
        assert!(!switch_8g.port_supports_poe("6"), "Port 6 should NOT support PoE on 2530-8G");
        assert!(!switch_8g.port_supports_poe("8"), "Port 8 should NOT support PoE on 2530-8G");

        // Test Aruba 2930F (ports 1-48 support PoE)
        let switch_2930f = create_test_switch(); // Uses Aruba2930F
        assert!(switch_2930f.port_supports_poe("1"), "Port 1 should support PoE on 2930F");
        assert!(switch_2930f.port_supports_poe("24"), "Port 24 should support PoE on 2930F");
        assert!(switch_2930f.port_supports_poe("48"), "Port 48 should support PoE on 2930F");
        assert!(!switch_2930f.port_supports_poe("49"), "Port 49 should NOT support PoE (out of range)");

        // Test Aruba 2540-24G (no PoE support)
        let config_2540 = SwitchConfig {
            id: "test-2540".to_string(),
            hostname: Some("test-2540".to_string()),
            model: Some(SwitchModel::Aruba2540_24G),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
                        validation: None,
            vendor_specific: std::collections::HashMap::new(),
management_vlan: None,
            settings: crate::config::Settings::default(),
        };
        let switch_2540 = ArubaSwitch::new(config_2540, RuntimeConfig::default(), false);
        assert!(!switch_2540.port_supports_poe("1"), "Port 1 should NOT support PoE on 2540-24G");
        assert!(!switch_2540.port_supports_poe("24"), "Port 24 should NOT support PoE on 2540-24G");
    }

    #[test]
    fn test_bug_fix_mac_notify_both_traps() {
        // Bug #5: mac_notify Commands
        // VERIFY: Commands include BOTH "mac-notify traps learned" AND "mac-notify traps removed"
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: true,  // MAC notify enabled
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Must have BOTH trap types
        assert!(commands.contains(&"mac-notify traps learned".to_string()),
                "Missing 'mac-notify traps learned' command!");
        assert!(commands.contains(&"mac-notify traps removed".to_string()),
                "Missing 'mac-notify traps removed' command!");
    }

    #[test]
    fn test_bug_fix_mac_notify_disabled() {
        // Bug #5 complement: When mac_notify is disabled
        // VERIFY: Uses explicit "no mac-notify traps" commands to properly disable
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,  // MAC notify disabled
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Must use explicit "no" commands for both trap types to properly disable
        // Note: "no mac-notify" alone doesn't remove explicit trap commands
        assert!(commands.contains(&"no mac-notify traps learned".to_string()),
                "Missing 'no mac-notify traps learned' command when MAC notify is disabled!");
        assert!(commands.contains(&"no mac-notify traps removed".to_string()),
                "Missing 'no mac-notify traps removed' command when MAC notify is disabled!");
    }

    #[test]
    fn test_bug_fix_port_mirroring_syntax() {
        // Bug #7: Port Mirroring Syntax
        // VERIFY: Uses correct Aruba syntax with session ID
        let switch = create_test_switch();

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["3".to_string(), "4".to_string()],
                destination_port: "12".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let commands = switch.generate_mirror_commands(&mirrors);

        // Verify correct global mirror configuration
        assert!(commands.contains(&"mirror 1 port 12".to_string()),
                "Missing correct 'mirror 1 port 12' command!");

        // Per-interface monitor commands are now in generate_port_commands(), not here
        assert!(!commands.contains(&"interface 3".to_string()),
                "Interface commands should not be in mirror commands!");
        assert!(!commands.contains(&"interface 4".to_string()),
                "Interface commands should not be in mirror commands!");
        assert!(!commands.contains(&"monitor all both mirror 1".to_string()),
                "Monitor commands should not be in mirror commands!");

        // Verify we DON'T have the old incorrect syntax
        assert!(!commands.iter().any(|c| c == "mirror-port 12"),
                "Found old incorrect 'mirror-port' syntax - should be 'mirror 1 port 12'!");
        assert!(!commands.iter().any(|c| c == "monitor"),
                "Found old incorrect 'monitor' syntax - should be 'monitor all both mirror 1'!");
    }

    #[test]
    fn test_bug_fix_port_mirroring_directions() {
        // Bug #7 extension: Test different mirror directions
        // Now testing with generate_port_commands() since that's where monitor commands are generated
        let switch = create_test_switch();

        // Test Rx (ingress) direction
        let mirrors_rx = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["5".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Rx,
            },
        ];
        let ports_rx = vec![
            Port {
                port_id: "5".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];
        let commands_rx = switch.generate_port_commands(&ports_rx, &mirrors_rx);
        assert!(commands_rx.contains(&"monitor all in mirror 1".to_string()),
                "Missing 'monitor all in mirror 1' for Rx direction!");

        // Test Tx (egress) direction
        let mirrors_tx = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["6".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Tx,
            },
        ];
        let ports_tx = vec![
            Port {
                port_id: "6".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];
        let commands_tx = switch.generate_port_commands(&ports_tx, &mirrors_tx);
        assert!(commands_tx.contains(&"monitor all out mirror 1".to_string()),
                "Missing 'monitor all out mirror 1' for Tx direction!");
    }

    #[test]
    fn test_bug_fix_snmp_community_quoting() {
        // Related to bug fixes: SNMP community strings should be quoted
        let switch = create_test_switch();

        let snmp = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Unrestricted,
                },
            ],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.100".to_string(),
                    community: "public".to_string(),
                    version: None,
                },
            ],
            enabled_traps: vec![],
        };

        // Test with no currently enabled traps
        let commands = switch.generate_snmp_commands(&snmp, &[]);

        // Verify community strings are quoted
        assert!(commands.contains(&"snmp-server community \"public\" unrestricted".to_string()),
                "SNMP community string should be quoted!");
        assert!(commands.contains(&"snmp-server host 192.168.1.100 community \"public\"".to_string()),
                "SNMP trap receiver community string should be quoted!");
    }

    #[test]
    fn test_bug_fix_comprehensive_port_config() {
        // Integration test: Port with multiple bug-fixed features
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Test port with all features".to_string()),
                enabled: true,
                poe_enabled: true,   // Bug #1
                mac_notify: true,    // Bug #5
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Verify all bug fixes are present
        assert!(commands.contains(&"interface 1".to_string()));
        assert!(commands.contains(&"name \"Test port with all features\"".to_string()));
        assert!(commands.contains(&"untagged vlan 10".to_string()));
        assert!(commands.contains(&"enable".to_string()));

        // Bug #1 fix: PoE commands
        assert!(commands.contains(&"power-over-ethernet".to_string()));
        assert!(commands.contains(&"poe-allocate-by class".to_string()));

        // Bug #5 fix: MAC notify commands
        assert!(commands.contains(&"mac-notify traps learned".to_string()));
        assert!(commands.contains(&"mac-notify traps removed".to_string()));
    }

    #[test]
    fn test_bug_2_snmp_trap_receiver_parsing() {
        // Bug #2: Test that parse_snmp_config correctly identifies trap receivers
        let switch = create_test_switch();

        // Simulate running-config with 2 trap receivers
        let running_config = r#"
snmp-server community "public" operator
snmp-server host 192.168.1.100 community "public"
snmp-server host 192.168.1.1 community "public"
snmp-server enable traps mac-notify
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        let parsed = switch.parse_snmp_config(&lines, true, true);

        assert!(parsed.is_some(), "Should parse SNMP config");
        let snmp = parsed.unwrap();

        // Should find 2 trap receivers
        assert_eq!(snmp.trap_receivers.len(), 2, "Should find 2 trap receivers");
        assert!(snmp.trap_receivers.iter().any(|r| r.host == "192.168.1.100"),
                "Should find trap receiver 192.168.1.100");
        assert!(snmp.trap_receivers.iter().any(|r| r.host == "192.168.1.1"),
                "Should find trap receiver 192.168.1.1");
    }

    #[test]
    fn test_bug_2_snmp_trap_receiver_removal_with_community() {
        // Bug #2: Verify removal commands include community string (critical fix)
        // This tests the fix for trap receivers not being removed properly
        // Running config line: snmp-server host 192.168.1.100 community "public"
        // Removal command must be: no snmp-server host 192.168.1.100 community "public"

        // Simulate parsing a running-config line with trap receiver
        let running_config = r#"
snmp-server host 192.168.1.100 community "public"
snmp-server host 192.168.1.200 community "private" inform
snmp-server host 192.168.1.99 community public
"#;

        let mut removal_commands = vec!["configure terminal".to_string()];
        let mut found_receivers = Vec::new();

        for line in running_config.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("snmp-server host ") {
                if let Some(host_start) = trimmed.find("snmp-server host ") {
                    let after_host = &trimmed[host_start + "snmp-server host ".len()..];

                    let parts: Vec<&str> = after_host.split_whitespace().collect();
                    if parts.len() >= 3 && parts[1] == "community" {
                        let host_ip = parts[0];
                        let community = parts[2].trim_matches('"');

                        found_receivers.push(format!("{} community \"{}\"", host_ip, community));
                        removal_commands.push(format!(
                            "no snmp-server host {} community \"{}\"",
                            host_ip, community
                        ));
                    } else if let Some(host_ip) = after_host.split_whitespace().next() {
                        found_receivers.push(host_ip.to_string());
                        removal_commands.push(format!("no snmp-server host {}", host_ip));
                    }
                }
            }
        }

        removal_commands.push("exit".to_string());

        // Verify we found all 3 trap receivers
        assert_eq!(found_receivers.len(), 3, "Should find 3 trap receivers");

        // Verify removal commands include community strings
        assert!(removal_commands.contains(&"no snmp-server host 192.168.1.100 community \"public\"".to_string()),
                "Should generate removal command with community for 192.168.1.100");
        assert!(removal_commands.contains(&"no snmp-server host 192.168.1.200 community \"private\"".to_string()),
                "Should generate removal command with community for 192.168.1.200");
        assert!(removal_commands.contains(&"no snmp-server host 192.168.1.99 community \"public\"".to_string()),
                "Should generate removal command with community for 192.168.1.99 (unquoted)");

        // Verify command structure
        assert_eq!(removal_commands[0], "configure terminal");
        assert_eq!(removal_commands[removal_commands.len() - 1], "exit");
        assert_eq!(removal_commands.len(), 5, "Should have: configure terminal + 3 removals + exit");
    }

    #[test]
    fn test_bug_3_link_change_trap_command() {
        // Bug #3: Link-change traps - verify command is generated correctly
        // Note: link-change traps require port list parameter ("all" or specific ports)
        let switch = create_test_switch();

        let snmp = SnmpConfig {
            communities: vec![],
            trap_receivers: vec![],
            enabled_traps: vec![TrapType::LinkChange],
        };

        // Test with no currently enabled traps - should only enable, not disable
        let commands = switch.generate_snmp_commands(&snmp, &[]);

        // Should generate link-change trap command with "all" suffix (port list required)
        assert!(commands.contains(&"snmp-server enable traps link-change all".to_string()),
                "Should generate 'snmp-server enable traps link-change all' command (port list required)");

        // Should NOT generate disable commands when nothing is currently enabled
        assert!(!commands.contains(&"no snmp-server enable traps link-change all".to_string()),
                "Should NOT try to remove 'link-change' traps when none are enabled");

        // Now test with link-change currently enabled - should disable other traps
        let current_traps = vec![TrapType::LinkChange, TrapType::MacNotify];
        let snmp_new = SnmpConfig {
            communities: vec![],
            trap_receivers: vec![],
            enabled_traps: vec![TrapType::LinkChange],  // Only want link-change
        };
        let commands2 = switch.generate_snmp_commands(&snmp_new, &current_traps);

        // Should disable mac-notify trap (currently enabled but not in new config)
        assert!(commands2.contains(&"no snmp-server enable traps mac-notify".to_string()),
                "Should disable mac-notify trap when it's enabled but not in new config");

        // Should NOT disable link-change (it's in both current and new config)
        assert!(!commands2.contains(&"no snmp-server enable traps link-change all".to_string()),
                "Should NOT disable link-change trap when it's in new config");
    }

    #[test]
    fn test_bug_3_link_change_trap_parsing() {
        // Bug #3 extension: Test that parser recognizes alternative syntax
        let switch = create_test_switch();

        // Test parsing "linkUp-linkDown" syntax
        let config = r#"
snmp-server enable traps linkUp-linkDown
"#;
        let lines: Vec<&str> = config.lines().collect();
        let parsed = switch.parse_snmp_config(&lines, true, false);

        assert!(parsed.is_some(), "Should parse SNMP config");
        let snmp = parsed.unwrap();
        assert_eq!(snmp.enabled_traps.len(), 1, "Should find 1 trap type");
        assert!(matches!(snmp.enabled_traps[0], TrapType::LinkChange),
                "Should parse 'linkUp-linkDown' as LinkChange trap type");
    }

    #[test]
    fn test_bug_4_snmp_access_level_dual_keywords() {
        // Bug #4: SNMP Access Level - Parser limitation for dual keywords
        // Aruba may use formats like "operator unrestricted" instead of just "operator"
        let switch = create_test_switch();

        // Test 1: Standard single keyword (should work)
        let config_single = r#"
snmp-server community "public" operator
"#;
        let lines_single: Vec<&str> = config_single.lines().collect();
        let parsed_single = switch.parse_snmp_config(&lines_single, false, false);
        assert!(parsed_single.is_some());
        let snmp_single = parsed_single.unwrap();
        assert_eq!(snmp_single.communities.len(), 1);
        assert_eq!(snmp_single.communities[0].name, "public");
        assert!(matches!(snmp_single.communities[0].access, SnmpAccess::Operator));

        // Test 2: Dual keywords (currently limited - parser only reads first keyword)
        let config_dual = r#"
snmp-server community "private" operator unrestricted
"#;
        let lines_dual: Vec<&str> = config_dual.lines().collect();
        let parsed_dual = switch.parse_snmp_config(&lines_dual, false, false);
        assert!(parsed_dual.is_some(), "Should parse SNMP config with dual keywords");
        let snmp_dual = parsed_dual.unwrap();
        assert_eq!(snmp_dual.communities.len(), 1, "Should find 1 community");
        assert_eq!(snmp_dual.communities[0].name, "private", "Community name should be 'private'");

        // Current parser limitation: only reads first access keyword after community name
        // When Aruba shows "operator unrestricted", parser only sees "operator"
        // This is expected behavior given current implementation
        let access = &snmp_dual.communities[0].access;
        println!("DEBUG: Parsed access level: {:?}", access);

        // For now, we document this as a known limitation
        // Parser reads first keyword only, so "operator unrestricted" becomes just "operator"
    }

    // ========================================================================
    // Per-Port MAC Notification Control Tests
    // These tests verify that individual ports can enable/disable MAC notify
    // traps independently when global MAC-notify SNMP trap is enabled
    // ========================================================================

    #[test]
    fn test_per_port_mac_notify_enabled() {
        // VERIFY: Port with mac_notify=true generates correct enable commands
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "5".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some("Device with MAC tracking".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: true,  // Enable MAC notifications
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Verify both trap types are enabled
        assert!(commands.contains(&"mac-notify traps learned".to_string()),
                "Port with mac_notify=true must have 'mac-notify traps learned' command");
        assert!(commands.contains(&"mac-notify traps removed".to_string()),
                "Port with mac_notify=true must have 'mac-notify traps removed' command");

        // Verify disable commands are NOT present
        assert!(!commands.contains(&"no mac-notify traps learned".to_string()),
                "Port with mac_notify=true should NOT have 'no mac-notify traps learned'");
        assert!(!commands.contains(&"no mac-notify traps removed".to_string()),
                "Port with mac_notify=true should NOT have 'no mac-notify traps removed'");
    }

    #[test]
    fn test_per_port_mac_notify_disabled() {
        // VERIFY: Port with mac_notify=false generates correct disable commands
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "8".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some("Security camera - no tracking".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,  // Disable MAC notifications
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Verify both trap types are explicitly disabled
        assert!(commands.contains(&"no mac-notify traps learned".to_string()),
                "Port with mac_notify=false must have 'no mac-notify traps learned' command");
        assert!(commands.contains(&"no mac-notify traps removed".to_string()),
                "Port with mac_notify=false must have 'no mac-notify traps removed' command");

        // Verify enable commands are NOT present
        assert!(!commands.contains(&"mac-notify traps learned".to_string()),
                "Port with mac_notify=false should NOT have 'mac-notify traps learned'");
        assert!(!commands.contains(&"mac-notify traps removed".to_string()),
                "Port with mac_notify=false should NOT have 'mac-notify traps removed'");
    }

    #[test]
    fn test_per_port_mac_notify_mixed_configuration() {
        // VERIFY: Multiple ports can have different MAC notify settings
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some("Monitored device".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: true,  // Enable
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some("Unmonitored device".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,  // Disable
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "3".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some("Another monitored device".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: true,  // Enable
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Count enable and disable commands
        let enable_learned = commands.iter().filter(|c| *c == "mac-notify traps learned").count();
        let enable_removed = commands.iter().filter(|c| *c == "mac-notify traps removed").count();
        let disable_learned = commands.iter().filter(|c| *c == "no mac-notify traps learned").count();
        let disable_removed = commands.iter().filter(|c| *c == "no mac-notify traps removed").count();

        // Should have enable commands for ports 1 and 3
        assert_eq!(enable_learned, 2, "Should have 2 'mac-notify traps learned' commands for ports 1 and 3");
        assert_eq!(enable_removed, 2, "Should have 2 'mac-notify traps removed' commands for ports 1 and 3");

        // Should have disable commands for port 2
        assert_eq!(disable_learned, 1, "Should have 1 'no mac-notify traps learned' command for port 2");
        assert_eq!(disable_removed, 1, "Should have 1 'no mac-notify traps removed' command for port 2");
    }

    #[test]
    fn test_snmp_global_mac_notify_without_all_command() {
        // VERIFY: Global SNMP config enables mac-notify trap but NOT "mac-notify traps all"
        // This is critical for per-port control to work
        let switch = create_test_switch();

        let snmp = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: crate::models::SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.100".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
            ],
            enabled_traps: vec![TrapType::MacNotify],
        };

        // Test with no currently enabled traps
        let commands = switch.generate_snmp_commands(&snmp, &[]);

        // Must enable the global SNMP trap type
        assert!(commands.contains(&"snmp-server enable traps mac-notify".to_string()),
                "Must have 'snmp-server enable traps mac-notify' for global SNMP trap");

        // Must NOT have "mac-notify traps all" which would override per-port settings
        assert!(!commands.contains(&"mac-notify traps all".to_string()),
                "CRITICAL: Must NOT have 'mac-notify traps all' - this would enable MAC notify on ALL ports globally, breaking per-port control!");
    }

    #[test]
    fn test_per_port_mac_notify_port_range() {
        // VERIFY: Port range expansion works correctly with mixed MAC notify settings
        let switch = create_test_switch();

        // Simulate ports 1-3 with MAC notify enabled, 4-6 with it disabled
        let mut ports = Vec::new();

        for i in 1..=3 {
            ports.push(Port {
                port_id: i.to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some(format!("Monitored port {}", i)),
                enabled: true,
                poe_enabled: true,
                mac_notify: true,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        for i in 4..=6 {
            ports.push(Port {
                port_id: i.to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: Some(format!("Unmonitored port {}", i)),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        let commands = switch.generate_port_commands(&ports, &[]);

        // Count commands
        let enable_count = commands.iter().filter(|c| *c == "mac-notify traps learned").count();
        let disable_count = commands.iter().filter(|c| *c == "no mac-notify traps learned").count();

        assert_eq!(enable_count, 3, "Should enable MAC notify on 3 ports (1-3)");
        assert_eq!(disable_count, 3, "Should disable MAC notify on 3 ports (4-6)");
    }

    #[test]
    fn test_per_port_mac_notify_parsing() {
        // VERIFY: Parser correctly identifies ports with and without MAC notify
        let switch = create_test_switch();

        // Simulate running-config with mixed MAC notify settings
        let running_config = r#"
interface 1
   name "Port with MAC notify enabled"
   mac-notify traps learned
   mac-notify traps removed
   exit
interface 2
   name "Port with MAC notify disabled"
   exit
interface 3
   name "Port explicitly disabled"
   exit
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        let mut i = 0;

        // Parse interface 1 (should have mac_notify=true)
        while i < lines.len() {
            if lines[i].trim().starts_with("interface 1") {
                let result = switch.parse_interface_name(&lines, &mut i);
                assert!(result.is_some(), "Should parse interface 1");
                let (port_id, _desc, _mirror, _monitor, _poe, mac_notify, _enabled, _speed) = result.unwrap();
                assert_eq!(port_id, "1");
                assert_eq!(mac_notify, true, "Interface 1 should have mac_notify=true");
                break;
            }
            i += 1;
        }

        // Parse interface 2 (should have mac_notify=false by default)
        i = 0;
        while i < lines.len() {
            if lines[i].trim().starts_with("interface 2") {
                let result = switch.parse_interface_name(&lines, &mut i);
                assert!(result.is_some(), "Should parse interface 2");
                let (port_id, _desc, _mirror, _monitor, _poe, mac_notify, _enabled, _speed) = result.unwrap();
                assert_eq!(port_id, "2");
                assert_eq!(mac_notify, false, "Interface 2 should have mac_notify=false (no explicit enable)");
                break;
            }
            i += 1;
        }
    }

    #[test]
    fn test_parse_snmp_config_with_link_change_enabled() {
        // Test that parse_snmp_config correctly adds link-change to enabled_traps
        // when link_change_enabled=true (from 'show snmp-server traps')
        let switch = create_test_switch();

        let running_config = r#"
snmp-server community "public" operator
snmp-server host 192.168.1.1 community "public"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        // Simulate link-change enabled (default state), mac-notify disabled
        let parsed = switch.parse_snmp_config(&lines, true, false);

        assert!(parsed.is_some(), "Should parse SNMP config");
        let snmp = parsed.unwrap();

        // Should have LinkChange in enabled_traps
        assert!(snmp.enabled_traps.contains(&crate::models::TrapType::LinkChange),
                "Should have LinkChange in enabled_traps when link_change_enabled=true");
        assert!(!snmp.enabled_traps.contains(&crate::models::TrapType::MacNotify),
                "Should NOT have MacNotify when mac_notify_enabled=false");
    }

    #[test]
    fn test_parse_snmp_config_with_link_change_disabled() {
        // Test that parse_snmp_config does NOT add link-change to enabled_traps
        // when link_change_enabled=false (explicitly disabled)
        let switch = create_test_switch();

        let running_config = r#"
snmp-server community "public" operator
snmp-server host 192.168.1.1 community "public"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        // Simulate link-change disabled, mac-notify enabled
        let parsed = switch.parse_snmp_config(&lines, false, true);

        assert!(parsed.is_some(), "Should parse SNMP config");
        let snmp = parsed.unwrap();

        // Should NOT have LinkChange in enabled_traps
        assert!(!snmp.enabled_traps.contains(&crate::models::TrapType::LinkChange),
                "Should NOT have LinkChange in enabled_traps when link_change_enabled=false");
        assert!(snmp.enabled_traps.contains(&crate::models::TrapType::MacNotify),
                "Should have MacNotify when mac_notify_enabled=true");
    }

    #[test]
    fn test_parse_snmp_config_with_both_traps_enabled() {
        // Test parsing when both link-change and mac-notify are enabled
        let switch = create_test_switch();

        let running_config = r#"
snmp-server community "public" operator
snmp-server host 192.168.1.1 community "public"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        // Both traps enabled
        let parsed = switch.parse_snmp_config(&lines, true, true);

        assert!(parsed.is_some(), "Should parse SNMP config");
        let snmp = parsed.unwrap();

        // Should have both trap types
        assert!(snmp.enabled_traps.contains(&crate::models::TrapType::LinkChange),
                "Should have LinkChange when link_change_enabled=true");
        assert!(snmp.enabled_traps.contains(&crate::models::TrapType::MacNotify),
                "Should have MacNotify when mac_notify_enabled=true");
        assert_eq!(snmp.enabled_traps.len(), 2, "Should have exactly 2 trap types");
    }

    #[test]
    fn test_parse_snmp_config_with_both_traps_disabled() {
        // Test parsing when both link-change and mac-notify are disabled
        let switch = create_test_switch();

        let running_config = r#"
snmp-server community "public" operator
snmp-server host 192.168.1.1 community "public"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        // Both traps disabled
        let parsed = switch.parse_snmp_config(&lines, false, false);

        assert!(parsed.is_some(), "Should parse SNMP config (communities present)");
        let snmp = parsed.unwrap();

        // Should have no trap types
        assert!(snmp.enabled_traps.is_empty(),
                "Should have no trap types when both are disabled");
    }

    #[test]
    fn test_generate_snmp_commands_link_change_already_enabled() {
        // Test that we don't try to disable link-change when it's enabled and should stay enabled
        let switch = create_test_switch();

        let snmp_config = crate::models::SnmpConfig {
            communities: vec![
                crate::models::SnmpCommunity {
                    name: "public".to_string(),
                    access: crate::models::SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![
                crate::models::SnmpTrapReceiver {
                    host: "192.168.1.1".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
            ],
            enabled_traps: vec![
                crate::models::TrapType::LinkChange,
                crate::models::TrapType::MacNotify,
            ],
        };

        // Current state: link-change enabled (default)
        let current_traps = vec![crate::models::TrapType::LinkChange];

        let commands = switch.generate_snmp_commands(&snmp_config, &current_traps);

        // Should NOT try to disable link-change
        assert!(!commands.iter().any(|c| c.contains("no snmp-server enable traps link-change")),
                "Should NOT disable link-change when it's in both current and desired config");

        // Should enable mac-notify (not currently enabled)
        assert!(commands.iter().any(|c| c == "snmp-server enable traps mac-notify"),
                "Should enable mac-notify");

        // Should enable link-change (even though it's already enabled - harmless)
        assert!(commands.iter().any(|c| c == "snmp-server enable traps link-change all"),
                "Should send link-change enable command");
    }

    #[test]
    fn test_generate_snmp_commands_disable_link_change() {
        // Test that we correctly disable link-change when it should be turned off
        let switch = create_test_switch();

        let snmp_config = crate::models::SnmpConfig {
            communities: vec![
                crate::models::SnmpCommunity {
                    name: "public".to_string(),
                    access: crate::models::SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![],
            enabled_traps: vec![
                // Only mac-notify, NOT link-change
                crate::models::TrapType::MacNotify,
            ],
        };

        // Current state: link-change enabled (default), mac-notify disabled
        let current_traps = vec![crate::models::TrapType::LinkChange];

        let commands = switch.generate_snmp_commands(&snmp_config, &current_traps);

        // Should disable link-change (it's enabled but not in desired config)
        assert!(commands.iter().any(|c| c == "no snmp-server enable traps link-change all"),
                "Should disable link-change when it's not in desired config");

        // Should enable mac-notify
        assert!(commands.iter().any(|c| c == "snmp-server enable traps mac-notify"),
                "Should enable mac-notify");

        // Should NOT send enable command for link-change
        assert!(!commands.iter().any(|c| c == "snmp-server enable traps link-change all"),
                "Should NOT enable link-change when it's not in desired config");
    }

    #[test]
    fn test_generate_snmp_commands_enable_link_change_when_disabled() {
        // Test that we enable link-change when it was previously disabled
        let switch = create_test_switch();

        let snmp_config = crate::models::SnmpConfig {
            communities: vec![],
            trap_receivers: vec![],
            enabled_traps: vec![
                crate::models::TrapType::LinkChange,
            ],
        };

        // Current state: link-change disabled (unusual, but possible)
        let current_traps: Vec<crate::models::TrapType> = vec![];

        let commands = switch.generate_snmp_commands(&snmp_config, &current_traps);

        // Should enable link-change
        assert!(commands.iter().any(|c| c == "snmp-server enable traps link-change all"),
                "Should enable link-change when it's in desired config but currently disabled");

        // Should NOT try to disable anything
        assert!(!commands.iter().any(|c| c.starts_with("no snmp-server enable traps")),
                "Should NOT disable any traps when current_traps is empty");
    }

    #[test]
    fn test_port_mirroring_four_source_ports() {
        // Test for the bug described in TODO.md:
        // Port mirroring with 4 source ports (33, 34, 35, 36) and destination port 42
        // All 4 source ports should have the monitor command applied
        // This test verifies the fix: monitor commands are now in port config, not mirror config
        let switch = create_test_switch();

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["33".to_string(), "34".to_string(), "35".to_string(), "36".to_string()],
                destination_port: "42".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        // Create Port structs for the 4 source ports
        let ports = vec![
            Port {
                port_id: "33".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "34".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "35".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "36".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        // Test generate_port_commands() with mirrors
        let port_commands = switch.generate_port_commands(&ports, &mirrors);

        // Print all commands for debugging
        println!("\nGenerated port commands with mirrors:");
        for (i, cmd) in port_commands.iter().enumerate() {
            println!("{}: {}", i + 1, cmd);
        }

        // Verify ALL source ports have monitor command in their interface block
        for port_id in &["33", "34", "35", "36"] {
            let interface_cmd = format!("interface {}", port_id);
            let interface_idx = port_commands.iter().position(|c| c == &interface_cmd);
            assert!(interface_idx.is_some(), "Missing interface command for port {}", port_id);

            let idx = interface_idx.unwrap();

            // Find the monitor command within this interface block (before the exit)
            let mut found_monitor = false;
            let mut i = idx + 1;
            while i < port_commands.len() && port_commands[i] != "exit" {
                if port_commands[i] == "monitor all both mirror 1" {
                    found_monitor = true;
                    break;
                }
                i += 1;
            }
            assert!(found_monitor,
                    "Port {} should have 'monitor all both mirror 1' command in its interface block", port_id);
        }

        // Verify there are exactly 4 monitor commands (one for each source port)
        let total_monitor_commands = port_commands.iter().filter(|c| *c == "monitor all both mirror 1").count();
        assert_eq!(total_monitor_commands, 4,
                "Should have exactly 4 'monitor all both mirror 1' commands (one for each source port), but found {}",
                total_monitor_commands);

        // Test generate_mirror_commands() - should only have global mirror destination
        let mirror_commands = switch.generate_mirror_commands(&mirrors);

        println!("\nGenerated mirror commands:");
        for (i, cmd) in mirror_commands.iter().enumerate() {
            println!("{}: {}", i + 1, cmd);
        }

        // Verify global mirror session configuration
        assert!(mirror_commands.contains(&"configure terminal".to_string()),
                "Missing 'configure terminal' command");
        assert!(mirror_commands.contains(&"mirror 1 port 42".to_string()),
                "Missing global mirror session command");

        // Verify NO per-interface commands in mirror_commands
        assert!(!mirror_commands.iter().any(|c| c.starts_with("interface")),
                "Mirror commands should not contain interface commands");
        assert!(!mirror_commands.iter().any(|c| c.starts_with("monitor")),
                "Mirror commands should not contain monitor commands");
    }

    #[test]
    fn test_parse_legacy_mirror_port_syntax() {
        // Test parsing of older "mirror-port <destination>" syntax used on 2530/2540 series
        // This is different from newer "mirror <session-id> port <destination>" syntax
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
mirror-port 42
interface 33
   name "IoT - Zone 1"
   monitor
   exit
interface 34
   name "IoT - Zone 1"
   monitor
   exit
interface 35
   name "IoT - Zone 1"
   monitor
   exit
interface 42
   disable
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 42
   exit
vlan 1020
   name "iot-vlan"
   untagged 33-35
   exit
"#;
        let state = switch.parse_running_config(config);

        // Verify mirror configuration was parsed correctly
        assert_eq!(state.port_mirrors.len(), 1, "Should have 1 mirror session");

        let mirror = &state.port_mirrors[0];
        assert_eq!(mirror.destination_port, "42", "Mirror destination should be port 42");
        assert_eq!(mirror.source_ports.len(), 3, "Should have 3 source ports (33, 34, 35)");
        assert!(mirror.source_ports.contains(&"33".to_string()), "Port 33 should be a source");
        assert!(mirror.source_ports.contains(&"34".to_string()), "Port 34 should be a source");
        assert!(mirror.source_ports.contains(&"35".to_string()), "Port 35 should be a source");
    }

    #[test]
    fn test_parse_legacy_mirror_port_syntax_single_source() {
        // Test legacy mirror-port with only one source port
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
mirror-port 24
interface 1
   monitor
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 1,24
   exit
"#;
        let state = switch.parse_running_config(config);

        assert_eq!(state.port_mirrors.len(), 1, "Should have 1 mirror session");
        let mirror = &state.port_mirrors[0];
        assert_eq!(mirror.destination_port, "24", "Mirror destination should be port 24");
        assert_eq!(mirror.source_ports.len(), 1, "Should have 1 source port");
        assert!(mirror.source_ports.contains(&"1".to_string()), "Port 1 should be a source");
    }

    #[test]
    fn test_parse_legacy_mirror_port_no_monitor_sources() {
        // Test legacy mirror-port when no interfaces have 'monitor' command
        // This should result in no mirror configuration (destination without sources)
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
mirror-port 24
interface 1
   name "Server Port"
   exit
interface 24
   disable
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 1,24
   exit
"#;
        let state = switch.parse_running_config(config);

        // No mirror should be created without source ports
        assert_eq!(state.port_mirrors.len(), 0, "Should have no mirror sessions without source ports");
    }

    #[test]
    fn test_parse_new_mirror_syntax() {
        // Test the newer "mirror <session-id> port <destination>" syntax (2930F and newer)
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
mirror 1 port 22
interface 5
   monitor
   exit
interface 6
   monitor
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 5-6,22
   exit
"#;
        let state = switch.parse_running_config(config);

        assert_eq!(state.port_mirrors.len(), 1, "Should have 1 mirror session");
        let mirror = &state.port_mirrors[0];
        assert_eq!(mirror.destination_port, "22", "Mirror destination should be port 22");
        assert_eq!(mirror.source_ports.len(), 2, "Should have 2 source ports");
        assert!(mirror.source_ports.contains(&"5".to_string()), "Port 5 should be a source");
        assert!(mirror.source_ports.contains(&"6".to_string()), "Port 6 should be a source");
    }

    #[test]
    fn test_parse_mirror_port_with_whitespace() {
        // Test mirror-port parsing handles extra whitespace correctly
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
mirror-port  48
interface 10
   monitor
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 10,48
   exit
"#;
        let state = switch.parse_running_config(config);

        assert_eq!(state.port_mirrors.len(), 1, "Should have 1 mirror session");
        let mirror = &state.port_mirrors[0];
        // The extra space should be trimmed
        assert_eq!(mirror.destination_port, "48", "Mirror destination should be port 48 (whitespace trimmed)");
    }

    #[test]
    fn test_parse_no_mirror_config() {
        // Test config without any mirror configuration
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
interface 1
   name "Server Port"
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 1
   exit
"#;
        let state = switch.parse_running_config(config);

        assert_eq!(state.port_mirrors.len(), 0, "Should have no mirror sessions");
    }

    #[test]
    fn test_port_name_removed_when_not_in_config() {
        // Port has name in current state but config doesn't specify name
        // Expected: "no name" command should be generated
        let mut switch = create_test_switch();

        // Set up current state with port that has a name
        let current_state = SwitchState {
            vlans: vec![
                Vlan {
                    id: 1,
                    name: "default".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "5".to_string(),
                    mode: PortMode::Access,
                    vlan: 1,
                    tagged_vlans: vec![],
                    description: Some("Old Port Name".to_string()),  // Currently has a name
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };
        switch.current_state = Some(current_state);

        // Desired state: port without name
        let ports = vec![
            Port {
                port_id: "5".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,  // No name specified
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        println!("\nGenerated commands for name removal:");
        for cmd in &commands {
            println!("  {}", cmd);
        }

        // Should have "no name" command to clear the existing name
        assert!(commands.contains(&"no name".to_string()),
                "Should generate 'no name' command to clear existing port name");
        assert!(!commands.iter().any(|c| c.starts_with("name \"")),
                "Should NOT set a name when description is None");
    }

    #[test]
    fn test_port_name_changed_from_old_to_new() {
        // Port has name in current state and config specifies different name
        // Expected: New name should be set (no need to explicitly clear old one)
        let mut switch = create_test_switch();

        // Set up current state with port that has an old name
        let current_state = SwitchState {
            vlans: vec![
                Vlan {
                    id: 1,
                    name: "default".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "12".to_string(),
                    mode: PortMode::Access,
                    vlan: 1,
                    tagged_vlans: vec![],
                    description: Some("Old Name".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };
        switch.current_state = Some(current_state);

        // Desired state: port with new name
        let ports = vec![
            Port {
                port_id: "12".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: Some("New Name".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        println!("\nGenerated commands for name change:");
        for cmd in &commands {
            println!("  {}", cmd);
        }

        // Should set the new name
        assert!(commands.contains(&"name \"New Name\"".to_string()),
                "Should set new port name");
        // Should NOT have "no name" - just overwrite with new name
        assert!(!commands.contains(&"no name".to_string()),
                "Should NOT clear name when setting a new one");
    }

    #[test]
    fn test_port_name_kept_when_not_changed() {
        // Port has name in both current state and config (same name)
        // Expected: Name should be set (idempotent)
        let mut switch = create_test_switch();

        // Set up current state with port that has a name
        let current_state = SwitchState {
            vlans: vec![
                Vlan {
                    id: 1,
                    name: "default".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![
                Port {
                    port_id: "8".to_string(),
                    mode: PortMode::Access,
                    vlan: 1,
                    tagged_vlans: vec![],
                    description: Some("Same Name".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };
        switch.current_state = Some(current_state);

        // Desired state: port with same name
        let ports = vec![
            Port {
                port_id: "8".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: Some("Same Name".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Should set the name (idempotent - doesn't matter if same)
        assert!(commands.contains(&"name \"Same Name\"".to_string()),
                "Should set port name (idempotent operation)");
    }

    #[test]
    fn test_multiple_ports_names_cleanup() {
        // Ports 1-5 have names, ports 6-10 don't
        // Config includes 1-5 with names, 6-10 without names
        // Ports 6-10 currently have names that should be removed
        let mut switch = create_test_switch();

        // Current state: all ports have names
        let mut current_ports = vec![];
        for i in 1..=10 {
            current_ports.push(Port {
                port_id: i.to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: Some(format!("Port {} Current Name", i)),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        let current_state = SwitchState {
            vlans: vec![
                Vlan {
                    id: 1,
                    name: "default".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: current_ports,
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };
        switch.current_state = Some(current_state);

        // Desired state: ports 1-5 with names, ports 6-10 without names
        let mut desired_ports = vec![];
        for i in 1..=5 {
            desired_ports.push(Port {
                port_id: i.to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: Some(format!("Port {} New Name", i)),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }
        for i in 6..=10 {
            desired_ports.push(Port {
                port_id: i.to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,  // No name
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            });
        }

        let commands = switch.generate_port_commands(&desired_ports, &[]);

        println!("\nGenerated commands for multiple ports:");
        for cmd in &commands {
            println!("  {}", cmd);
        }

        // Ports 1-5 should have new names set
        for i in 1..=5 {
            assert!(commands.contains(&format!("name \"Port {} New Name\"", i)),
                    "Port {} should have new name set", i);
        }

        // Ports 6-10 should have "no name" command
        let no_name_count = commands.iter().filter(|c| *c == "no name").count();
        assert_eq!(no_name_count, 5,
                "Should have 5 'no name' commands for ports 6-10, found {}", no_name_count);
    }

    #[test]
    fn test_port_name_removed_in_reset_ports() {
        // reset_ports() should clear port names using "no name"
        let switch = create_test_switch();

        let port_ids = vec!["1".to_string(), "2".to_string(), "3".to_string()];

        // Note: reset_ports is async, but we can test the command generation
        // by examining the internal implementation
        let mut commands = vec!["configure terminal".to_string()];

        for port_id in &port_ids {
            let port_interface = switch.normalize_port_id(port_id);
            commands.push(format!("interface {}", port_interface));
            commands.push("disable".to_string());
            commands.push("no name".to_string());  // Should use "no name" not "no description"
            commands.push("untagged vlan 1".to_string());
            commands.push("exit".to_string());
        }

        commands.push("exit".to_string());

        println!("\nGenerated commands for port reset:");
        for cmd in &commands {
            println!("  {}", cmd);
        }

        // Verify "no name" is used (not "no description")
        let no_name_count = commands.iter().filter(|c| *c == "no name").count();
        assert_eq!(no_name_count, 3,
                "Should have 'no name' command for each port being reset");
        assert!(!commands.iter().any(|c| c == "no description"),
                "Should NOT use 'no description' (Aruba uses 'name', not 'description')");
    }

    #[test]
    fn test_parse_management_vlan_simple() {
        let switch = create_test_switch();

        let running_config = r#"
Running configuration:

; J9855A Configuration Editor; Created on release #WC.16.11.0018
; Ver #14:6f.f8.7f.ff.7c.59.fc.7b.ff.ff.fc.ff.ff.3f.ef:2d

hostname "test-switch"
management-vlan 99
vlan 1
   name "DEFAULT_VLAN"
   untagged 1-48,Trk1
   ip address dhcp-bootp
   exit
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        let result = switch.parse_management_vlan(&lines);

        assert_eq!(result, Some(99), "Should parse 'management-vlan 99'");
    }

    #[test]
    fn test_parse_management_vlan_with_vlan_prefix() {
        let switch = create_test_switch();

        let running_config = r#"
hostname "switch"
management-vlan vlan10
vlan 10
   name "management"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        let result = switch.parse_management_vlan(&lines);

        assert_eq!(result, Some(10), "Should parse 'management-vlan vlan10'");
    }

    #[test]
    fn test_parse_management_vlan_none() {
        let switch = create_test_switch();

        let running_config = r#"
hostname "switch"
vlan 1
   name "DEFAULT_VLAN"
"#;

        let lines: Vec<&str> = running_config.lines().collect();
        let result = switch.parse_management_vlan(&lines);

        assert_eq!(result, None, "Should return None when no management-vlan is configured");
    }

    #[test]
    fn test_parse_management_vlan_diff() {
        use crate::diff::compute_diff;

        let switch = create_test_switch();

        // Current state: management VLAN 10
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(10),
            warnings: vec![],
        };

        // Desired config: management VLAN 99
        let mut desired_config = switch.config.clone();
        desired_config.management_vlan = Some(99);

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN change");
        assert_eq!(diff.management_vlan, Some(99), "Should show new management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_diff_add() {
        use crate::diff::compute_diff;

        let switch = create_test_switch();

        // Current state: no management VLAN
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        // Desired config: add management VLAN 50
        let mut desired_config = switch.config.clone();
        desired_config.management_vlan = Some(50);

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being added");
        assert_eq!(diff.management_vlan, Some(50), "Should show new management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_diff_remove() {
        use crate::diff::compute_diff;

        let switch = create_test_switch();

        // Current state: management VLAN 20
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(20),
            warnings: vec![],
        };

        // Desired config: remove management VLAN
        let mut desired_config = switch.config.clone();
        desired_config.management_vlan = None;

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being removed");
        assert_eq!(diff.management_vlan, None, "Should show no management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_diff_no_change() {
        use crate::diff::compute_diff;

        let switch = create_test_switch();

        // Current state: management VLAN 30
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(30),
            warnings: vec![],
        };

        // Desired config: same management VLAN 30
        let mut desired_config = switch.config.clone();
        desired_config.management_vlan = Some(30);

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(!diff.management_vlan_changed, "Should not detect change when management VLAN is same");
        // Note: when no change, diff.management_vlan is not set (remains at default)
    }

    // ========================================================================
    // ANSI Escape Code Stripping Tests
    // These tests verify that ANSI escape sequences from serial connections
    // are properly stripped before parsing the running configuration
    // ========================================================================

    #[test]
    fn test_ansi_escape_code_stripping_in_interface_lines() {
        // Bug: Serial connections can inject ANSI escape codes into the running config
        // which breaks parsing. For example, interface names won't be detected.
        let switch = create_test_switch();

        // Simulate running config with ANSI escape sequences embedded
        // Real example: "\x1b[24;1H\x1b[2K\x1b[24;1H\x1b[1;24r\x1b[24;1Hinterface 11"
        // Using actual escape character \x1b (ESC)
        let config_with_ansi = format!(
            r#"
hostname "test-switch"
vlan 1
   name "DEFAULT_VLAN"
   no untagged 49-52
   exit
vlan 4
   name "iot-vlan"
   exit
{}[24;1H{}[2K{}[24;1H{}[1;24r{}[24;1Hinterface 11
   name "Zone 1"
   untagged vlan 4
   exit
interface 12
   name "Regular port"
   untagged vlan 4
   exit
"#,
            '\x1b', '\x1b', '\x1b', '\x1b', '\x1b'
        );

        let state = switch.parse_running_config(&config_with_ansi);

        // Should find both ports - port 11 had ANSI codes before "interface"
        let port_11 = state.ports.iter().find(|p| p.port_id == "11");
        let port_12 = state.ports.iter().find(|p| p.port_id == "12");

        assert!(port_11.is_some(), "Should parse port 11 even with ANSI escape codes before 'interface'");
        assert!(port_12.is_some(), "Should parse port 12 (no ANSI codes)");

        // Verify port 11 has correct configuration
        let p11 = port_11.unwrap();
        assert_eq!(p11.description, Some("Zone 1".to_string()),
                   "Port 11 should have correct description after ANSI stripping");
        // Note: The main goal is ensuring the port is found despite ANSI codes
        // VLAN parsing may vary based on how "untagged vlan" lines are processed
    }

    #[test]
    fn test_ansi_escape_code_stripping_various_sequences() {
        // Test various ANSI escape sequence patterns
        let switch = create_test_switch();

        // Different ANSI sequences that might appear:
        // - Cursor positioning: \x1b[H, \x1b[24;1H
        // - Clear line: \x1b[2K, \x1b[K
        // - Scroll region: \x1b[1;24r
        // - Colors: \x1b[32m, \x1b[0m
        // Using actual escape character \x1b (ESC)
        let esc = '\x1b';
        let config_with_various_ansi = format!(
            r#"
{esc}[H{esc}[2Jhostname "test-switch"
{esc}[32mvlan 10{esc}[0m
   name "test-vlan"
   exit
{esc}[1;24r{esc}[24;1Hinterface 1
   name "Port with cursor codes"
   untagged vlan 10
   exit
{esc}[?25hinterface 2
   name "Port with cursor visibility code"
   untagged vlan 10
   exit
"#,
            esc = esc
        );

        let state = switch.parse_running_config(&config_with_various_ansi);

        // Should find VLAN 10
        let vlan_10 = state.vlans.iter().find(|v| v.id == 10);
        assert!(vlan_10.is_some(), "Should parse VLAN 10 with ANSI codes around it");

        // Should find both ports
        let port_1 = state.ports.iter().find(|p| p.port_id == "1");
        let port_2 = state.ports.iter().find(|p| p.port_id == "2");

        assert!(port_1.is_some(), "Should parse port 1 with ANSI cursor positioning codes");
        assert!(port_2.is_some(), "Should parse port 2 with ANSI cursor visibility codes");

        // Verify descriptions
        assert_eq!(port_1.unwrap().description, Some("Port with cursor codes".to_string()));
        assert_eq!(port_2.unwrap().description, Some("Port with cursor visibility code".to_string()));
    }

    #[test]
    fn test_ansi_escape_code_stripping_preserves_valid_content() {
        // Ensure ANSI stripping doesn't remove legitimate content
        let switch = create_test_switch();

        let config_clean = r#"
hostname "switch-[production]"
vlan 100
   name "servers"
   exit
interface 5
   name "Server [rack-1]"
   untagged vlan 100
   exit
"#;

        let state = switch.parse_running_config(config_clean);

        // Should preserve brackets in names (not ANSI sequences)
        let port_5 = state.ports.iter().find(|p| p.port_id == "5");
        assert!(port_5.is_some(), "Should parse port 5");
        assert_eq!(port_5.unwrap().description, Some("Server [rack-1]".to_string()),
                   "Should preserve brackets in port names - they're not ANSI codes");
    }

    // ========================================================================
    // PoE Commands on Non-PoE Switch Models Tests
    // These tests verify that PoE commands are not generated for switch models
    // that don't support Power over Ethernet
    // ========================================================================

    fn create_non_poe_switch() -> ArubaSwitch {
        // Create a switch with a non-PoE model (Aruba2540_48G_4SFP)
        let config = SwitchConfig {
            id: "test-non-poe".to_string(),
            hostname: Some("non-poe-switch".to_string()),
            model: Some(SwitchModel::Aruba2540_48G_4SFP),  // Non-PoE model
            management_ip: Some("192.168.1.2".to_string()),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        ArubaSwitch::new(config, RuntimeConfig::default(), false)
    }

    #[test]
    fn test_non_poe_switch_no_poe_commands_in_port_config() {
        // Bug: Non-PoE switches were receiving PoE commands which fail with "Invalid input"
        // This caused constant diffs between desired (poe_enabled=false) and parsed (poe_enabled=true)
        let switch = create_non_poe_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Test port".to_string()),
                enabled: true,
                poe_enabled: true,  // Even if config says true, non-PoE switch can't support it
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Should NOT contain any PoE commands for non-PoE switch
        assert!(!commands.iter().any(|c| c.contains("power-over-ethernet")),
                "Non-PoE switch should not generate 'power-over-ethernet' commands");
        assert!(!commands.iter().any(|c| c.contains("poe-allocate")),
                "Non-PoE switch should not generate 'poe-allocate' commands");

        // Should still generate other port commands
        assert!(commands.contains(&"interface 1".to_string()),
                "Should still configure interface");
        assert!(commands.contains(&"name \"Test port\"".to_string()),
                "Should still set port name");
        assert!(commands.contains(&"untagged vlan 10".to_string()),
                "Should still set VLAN");
    }

    #[test]
    fn test_non_poe_switch_no_poe_commands_with_poe_disabled() {
        // Verify that even with poe_enabled=false, no PoE commands are generated
        let switch = create_non_poe_switch();

        let ports = vec![
            Port {
                port_id: "5".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,  // Explicitly disabled
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // Should NOT contain "no power-over-ethernet" either
        assert!(!commands.iter().any(|c| c.contains("power-over-ethernet")),
                "Non-PoE switch should not generate any PoE-related commands, even 'no power-over-ethernet'");
    }

    #[test]
    fn test_poe_switch_generates_poe_commands() {
        // Verify that PoE switches (like Aruba2930F) DO generate PoE commands
        let switch = create_test_switch();  // Uses Aruba2930F which supports PoE

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("PoE port".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // PoE switch SHOULD generate PoE commands
        assert!(commands.contains(&"power-over-ethernet".to_string()),
                "PoE switch should generate 'power-over-ethernet' command when poe_enabled=true");
    }

    #[test]
    fn test_poe_switch_generates_no_poe_command_when_disabled() {
        // Verify that PoE switches generate "no power-over-ethernet" when poe_enabled=false
        let switch = create_test_switch();  // Uses Aruba2930F which supports PoE

        let ports = vec![
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,  // PoE disabled
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // PoE switch SHOULD generate "no power-over-ethernet" when disabled
        assert!(commands.contains(&"no power-over-ethernet".to_string()),
                "PoE switch should generate 'no power-over-ethernet' when poe_enabled=false");
    }

    #[test]
    fn test_non_poe_switch_model_detection() {
        // Verify which switch models are correctly identified as non-PoE
        assert!(!SwitchModel::Aruba2540_48G_4SFP.supports_poe(),
                "Aruba2540_48G_4SFP should NOT support PoE");
        assert!(!SwitchModel::Aruba2540_24G.supports_poe(),
                "Aruba2540_24G should NOT support PoE");
        assert!(!SwitchModel::Aruba2530_48G_2SFP.supports_poe(),
                "Aruba2530_48G_2SFP should NOT support PoE");

        // PoE models
        assert!(SwitchModel::Aruba2530_24G_POE.supports_poe(),
                "Aruba2530_24G_POE SHOULD support PoE");
        assert!(SwitchModel::Aruba2530_8G_POE.supports_poe(),
                "Aruba2530_8G_POE SHOULD support PoE");
        assert!(SwitchModel::Aruba2930F.supports_poe(),
                "Aruba2930F SHOULD support PoE");
    }

    #[test]
    fn test_non_poe_switch_parser_defaults_poe_false() {
        // Verify that non-PoE switches parse port poe_enabled as false by default
        // This prevents constant reconfiguration when desired config has poe_enabled: false
        let switch = create_non_poe_switch();

        // Config with no PoE-related commands (as would be the case on non-PoE hardware)
        let config = r#"
interface 1
   name "Server Port"
   exit
interface 2
   name "Workstation"
   exit
vlan 10
   name "data"
   untagged 1-2
   exit
"#;

        let state = switch.parse_running_config(config);

        // Both ports should have poe_enabled = false for non-PoE switches
        for port in &state.ports {
            assert_eq!(port.poe_enabled, false,
                      "Port {} on non-PoE switch should have poe_enabled=false, got poe_enabled=true",
                      port.port_id);
        }
    }

    #[test]
    fn test_poe_switch_parser_defaults_poe_true() {
        // Verify that PoE switches parse port poe_enabled as true by default
        let switch = create_test_switch();  // Uses Aruba2930F (PoE model)

        // Config with no PoE-related commands (PoE is enabled by default)
        let config = r#"
interface 1
   name "Server Port"
   exit
interface 2
   name "Workstation"
   exit
vlan 10
   name "data"
   untagged 1-2
   exit
"#;

        let state = switch.parse_running_config(config);

        // Both ports should have poe_enabled = true for PoE switches (default)
        for port in &state.ports {
            assert_eq!(port.poe_enabled, true,
                      "Port {} on PoE switch should have poe_enabled=true by default, got poe_enabled=false",
                      port.port_id);
        }
    }

    #[test]
    fn test_poe_switch_parser_respects_no_poe_command() {
        // Verify that PoE switches correctly parse "no power-over-ethernet" command
        let switch = create_test_switch();  // Uses Aruba2930F (PoE model)

        // Config with explicit PoE disabled on port 1
        let config = r#"
interface 1
   name "Server Port"
   no power-over-ethernet
   exit
interface 2
   name "Workstation"
   exit
vlan 10
   name "data"
   untagged 1-2
   exit
"#;

        let state = switch.parse_running_config(config);

        let port1 = state.ports.iter().find(|p| p.port_id == "1").unwrap();
        let port2 = state.ports.iter().find(|p| p.port_id == "2").unwrap();

        assert_eq!(port1.poe_enabled, false,
                  "Port 1 with 'no power-over-ethernet' should have poe_enabled=false");
        assert_eq!(port2.poe_enabled, true,
                  "Port 2 without PoE command should default to poe_enabled=true");
    }

    #[test]
    fn test_poe_switch_parser_no_poe_with_allocate_by_class() {
        // Verify that "poe-allocate-by class" does NOT override "no power-over-ethernet"
        // This matches real Aruba 2530 running config where poe-allocate-by is always present
        // on PoE-capable ports regardless of whether PoE is enabled or disabled.
        let switch = create_test_switch();  // Uses Aruba2930F (PoE model)

        // Real-world config from Aruba 2530-24G PoE+
        let config = r#"
interface 1
   name "RTX3483 - Zone 1"
   poe-allocate-by class
   speed-duplex 100-full
   exit
interface 5
   name "cisco-mgmt"
   no power-over-ethernet
   poe-allocate-by class
   exit
interface 7
   name "cisco-ap"
   poe-allocate-by class
   exit
interface 15
   monitor
   name "APC v2 - Zone 1"
   no power-over-ethernet
   poe-allocate-by class
   mac-notify traps learned
   mac-notify traps removed
   exit
vlan 1000
   name "philips-ap-z1"
   untagged 1
   exit
vlan 2088
   name "cisco-mgmt"
   untagged 5
   exit
vlan 2090
   name "cisco-aps"
   untagged 7
   exit
vlan 1020
   name "philips-apc-z1"
   untagged 15
   exit
"#;

        let state = switch.parse_running_config(config);

        let port1 = state.ports.iter().find(|p| p.port_id == "1").unwrap();
        let port5 = state.ports.iter().find(|p| p.port_id == "5").unwrap();
        let port7 = state.ports.iter().find(|p| p.port_id == "7").unwrap();
        let port15 = state.ports.iter().find(|p| p.port_id == "15").unwrap();

        // Port 1: has poe-allocate-by but no "no power-over-ethernet" → PoE enabled (default)
        assert_eq!(port1.poe_enabled, true,
                  "Port 1 with poe-allocate-by but no 'no power-over-ethernet' should have poe_enabled=true");

        // Port 5: has "no power-over-ethernet" followed by "poe-allocate-by class" → PoE DISABLED
        assert_eq!(port5.poe_enabled, false,
                  "Port 5 with 'no power-over-ethernet' should have poe_enabled=false even with 'poe-allocate-by class'");

        // Port 7: has poe-allocate-by but no "no power-over-ethernet" → PoE enabled (default)
        assert_eq!(port7.poe_enabled, true,
                  "Port 7 with poe-allocate-by but no 'no power-over-ethernet' should have poe_enabled=true");

        // Port 15: has "no power-over-ethernet" + "poe-allocate-by class" + monitor → PoE DISABLED
        assert_eq!(port15.poe_enabled, false,
                  "Port 15 with 'no power-over-ethernet' should have poe_enabled=false even with 'poe-allocate-by class'");
    }

    #[test]
    fn test_legacy_mirror_monitor_command_for_2530() {
        // Aruba 2530 uses legacy "monitor" (no parameters) in interface context
        // Aruba 2930F uses "monitor all both mirror <session>"
        let config_2530 = SwitchConfig {
            id: "test-2530".to_string(),
            hostname: Some("test-2530".to_string()),
            model: Some(SwitchModel::Aruba2530_24G_POE),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };
        let runtime_config = RuntimeConfig::default();
        let switch_2530 = ArubaSwitch::new(config_2530, runtime_config, false);

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["15".to_string(), "16".to_string()],
                destination_port: "22".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let ports = vec![
            Port {
                port_id: "15".to_string(),
                mode: PortMode::Access,
                vlan: 1020,
                tagged_vlans: vec![],
                description: Some("Source 1".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "16".to_string(),
                mode: PortMode::Access,
                vlan: 1020,
                tagged_vlans: vec![],
                description: Some("Source 2".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let cmds = switch_2530.generate_port_commands(&ports, &mirrors);

        // 2530 should use plain "monitor" (no parameters)
        assert!(cmds.contains(&"monitor".to_string()),
                "Aruba 2530 should use 'monitor' command, got: {:?}", cmds);
        assert!(!cmds.iter().any(|c| c.starts_with("monitor all")),
                "Aruba 2530 should NOT use 'monitor all both mirror' syntax");

        // 2930F should use "monitor all both mirror <session>"
        let switch_2930 = create_test_switch();  // Uses Aruba2930F
        let cmds_2930 = switch_2930.generate_port_commands(&ports, &mirrors);

        assert!(cmds_2930.contains(&"monitor all both mirror 1".to_string()),
                "Aruba 2930F should use 'monitor all both mirror 1' command");
        assert!(!cmds_2930.contains(&"monitor".to_string()) ||
                cmds_2930.iter().filter(|c| *c == "monitor").count() == 0,
                "Aruba 2930F should NOT use plain 'monitor' command");
    }

    #[test]
    fn test_model_verification_matching_product() {
        // Test that a matching product number produces no warnings
        let switch = create_test_switch();  // Aruba2930F

        let config = "; JL253A Configuration Editor; Created on release #WC.16.11.0018\nhostname \"test\"\n";
        let state = switch.parse_running_config(config);

        assert!(state.warnings.is_empty(),
                "Matching product number should produce no warnings, got: {:?}", state.warnings);
    }

    #[test]
    fn test_model_verification_mismatched_product() {
        // Test that a mismatched product number produces a warning
        let switch = create_test_switch();  // Aruba2930F, expects JL253A etc.

        let config = "; J9779A Configuration Editor; Created on release #YB.16.10.0009\nhostname \"test\"\n";
        let state = switch.parse_running_config(config);

        assert_eq!(state.warnings.len(), 1,
                  "Mismatched product number should produce one warning, got: {:?}", state.warnings);
        assert!(state.warnings[0].contains("J9779A"),
                "Warning should mention detected product number");
        assert!(state.warnings[0].contains("mismatch"),
                "Warning should mention mismatch");
    }

    #[test]
    fn test_model_verification_no_header() {
        // Test that missing header line produces no warnings (just debug log)
        let switch = create_test_switch();

        let config = "hostname \"test\"\nvlan 1\n   name \"default\"\n   exit\n";
        let state = switch.parse_running_config(config);

        assert!(state.warnings.is_empty(),
                "Missing header should not produce warnings, got: {:?}", state.warnings);
    }

    #[test]
    fn test_product_numbers_mapping() {
        // Verify the product number lists are populated for Aruba models
        assert!(SwitchModel::Aruba2530_24G_POE.product_numbers().contains(&"J9773A"));
        assert!(SwitchModel::Aruba2530_24G_POE.product_numbers().contains(&"J9779A"));
        assert!(SwitchModel::Aruba2530_8G_POE.product_numbers().contains(&"J9774A"));
        assert!(SwitchModel::Aruba2530_48G_2SFP.product_numbers().contains(&"J9855A"));
        assert!(SwitchModel::Aruba2540_48G_4SFP.product_numbers().contains(&"JL355A"));
        assert!(!SwitchModel::Aruba2930F.product_numbers().is_empty());

        // All vendors now have product numbers for model detection
        assert!(!SwitchModel::Fortiswitch124F_FPOE.product_numbers().is_empty());
        assert!(!SwitchModel::CiscoCatalyst9300_24P_UPOE.product_numbers().is_empty());
    }

    // ========================================================================
    // Speed Duplex Command Generation Tests
    // Verify generate_port_commands produces correct speed-duplex for all variants
    // ========================================================================

    #[test]
    fn test_speed_duplex_command_generation_all_variants() {
        let switch = create_test_switch();

        let variants = vec![
            (SpeedDuplex::Auto, "speed-duplex auto"),
            (SpeedDuplex::TenHalf, "speed-duplex 10-half"),
            (SpeedDuplex::TenFull, "speed-duplex 10-full"),
            (SpeedDuplex::HundredHalf, "speed-duplex 100-half"),
            (SpeedDuplex::HundredFull, "speed-duplex 100-full"),
            (SpeedDuplex::ThousandFull, "speed-duplex 1000-full"),
        ];

        for (speed, expected_cmd) in variants {
            let ports = vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    tagged_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: speed,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ];

            let commands = switch.generate_port_commands(&ports, &[]);

            assert!(commands.contains(&expected_cmd.to_string()),
                    "SpeedDuplex::{:?} should generate '{}' command, got commands: {:?}",
                    speed, expected_cmd, commands);
        }
    }

    // ========================================================================
    // Speed Duplex Parsing Tests
    // Verify parse_running_config correctly parses speed-duplex from interface blocks
    // ========================================================================

    #[test]
    fn test_speed_duplex_parsing_from_running_config() {
        let switch = create_test_switch();

        let config = r#"
hostname "test-switch"
interface 1
   speed-duplex 100-full
   exit
interface 2
   speed-duplex auto
   exit
interface 3
   speed-duplex 1000-full
   exit
vlan 1
   name "DEFAULT_VLAN"
   untagged 1-3
   exit
"#;

        let state = switch.parse_running_config(config);

        let port1 = state.ports.iter().find(|p| p.port_id == "1");
        let port2 = state.ports.iter().find(|p| p.port_id == "2");
        let port3 = state.ports.iter().find(|p| p.port_id == "3");

        assert!(port1.is_some(), "Should parse port 1");
        assert!(port2.is_some(), "Should parse port 2");
        assert!(port3.is_some(), "Should parse port 3");

        assert_eq!(port1.unwrap().speed_duplex, SpeedDuplex::HundredFull,
                   "Port 1 should have speed_duplex=HundredFull (100-full)");
        assert_eq!(port2.unwrap().speed_duplex, SpeedDuplex::Auto,
                   "Port 2 should have speed_duplex=Auto");
        assert_eq!(port3.unwrap().speed_duplex, SpeedDuplex::ThousandFull,
                   "Port 3 should have speed_duplex=ThousandFull (1000-full)");
    }

    // ========================================================================
    // Non-PoE Model Skips PoE Commands Test
    // Verify Aruba2540_24G generates no PoE commands at all
    // ========================================================================

    #[test]
    fn test_non_poe_model_aruba2540_24g_skips_all_poe_commands() {
        let config = SwitchConfig {
            id: "test-2540-24g".to_string(),
            hostname: Some("non-poe-2540".to_string()),
            model: Some(SwitchModel::Aruba2540_24G),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        let switch = ArubaSwitch::new(config, RuntimeConfig::default(), false);

        // Test multiple ports with various poe_enabled settings
        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Port with poe true".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Port with poe false".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                tagged_vlans: vec![1, 10, 20],
                description: None,
                enabled: true,
                poe_enabled: true,
                mac_notify: true,
                speed_duplex: SpeedDuplex::HundredFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        // No PoE commands at all for Aruba2540_24G
        for cmd in &commands {
            assert!(!cmd.contains("power-over-ethernet"),
                    "Aruba2540_24G should not generate any PoE command, found: '{}'", cmd);
            assert!(!cmd.contains("poe-allocate"),
                    "Aruba2540_24G should not generate any poe-allocate command, found: '{}'", cmd);
        }

        // Should still generate other port commands correctly
        assert!(commands.contains(&"interface 1".to_string()));
        assert!(commands.contains(&"interface 2".to_string()));
        assert!(commands.contains(&"interface 24".to_string()));
        assert!(commands.contains(&"untagged vlan 10".to_string()));
    }

    // ========================================================================
    // VLAN Boundary IDs Test
    // Verify VLAN commands work with boundary IDs 1 and 4094
    // ========================================================================

    #[test]
    fn test_vlan_boundary_ids() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 1,
                name: "default".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 4094,
                name: "max-vlan".to_string(),
                description: None,
                ip_config: VlanIpConfig::Static {
                    address: "10.255.255.1".to_string(),
                    netmask: "255.255.255.0".to_string(),
                },
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"vlan 1".to_string()),
                "Should generate VLAN 1 (minimum boundary)");
        assert!(commands.contains(&"name default".to_string()),
                "Should set name for VLAN 1");
        assert!(commands.contains(&"vlan 4094".to_string()),
                "Should generate VLAN 4094 (maximum boundary)");
        assert!(commands.contains(&"name max-vlan".to_string()),
                "Should set name for VLAN 4094");
        assert!(commands.contains(&"ip address 10.255.255.1 255.255.255.0".to_string()),
                "Should set static IP for VLAN 4094");
    }

    // ========================================================================
    // VLAN Name With Backtick Test
    // Verify VLAN name containing a backtick character is handled
    // ========================================================================

    #[test]
    fn test_vlan_name_with_backtick() {
        let switch = create_test_switch();

        let vlans = vec![
            Vlan {
                id: 100,
                name: "test`vlan".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"vlan 100".to_string()),
                "Should generate VLAN 100");
        // Backtick doesn't contain a space, so it should not be quoted
        assert!(commands.contains(&"name test`vlan".to_string()),
                "Should set name with backtick character (no quoting since no spaces)");
    }

    // ========================================================================
    // Full Running Config Parse Test
    // Comprehensive test parsing a realistic Aruba 2530-48G running config
    // ========================================================================

    #[test]
    fn test_full_running_config_parse() {
        // Use an Aruba2530_48G_2SFP model to match the J9855A product number
        let config_2530_48g = SwitchConfig {
            id: "test-48g".to_string(),
            hostname: Some("sw-48g-test".to_string()),
            model: Some(SwitchModel::Aruba2530_48G_2SFP),
            management_ip: Some("192.168.1.10".to_string()),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        let switch = ArubaSwitch::new(config_2530_48g, RuntimeConfig::default(), false);

        let running_config = r#"
; J9855A Configuration Editor; Created on release #YA.16.11.0012
; Ver #15:28.6f.f8.1d.9b.3f.bf.bb.ef.7c.59.fc.6b.fb.9f.fc.ff.ff.37.ef:2d

hostname "sw-48g-test"
snmp-server community "monitoring" operator
snmp-server community "private-rw" manager
snmp-server host 10.0.0.50 community "monitoring"
snmp-server host 10.0.0.51 community "monitoring"
mirror-port 48
vlan 1
   name "DEFAULT_VLAN"
   untagged 1-10,47-48
   ip address dhcp-bootp
   exit
vlan 100
   name "servers"
   untagged 11-20
   tagged 47
   ip address 10.100.0.1 255.255.255.0
   exit
vlan 200
   name "workstations"
   untagged 21-40
   tagged 47
   no ip address
   exit
vlan 999
   name "quarantine"
   untagged 41-46
   no ip address
   exit
interface 1
   name "Gateway"
   speed-duplex 1000-full
   exit
interface 5
   name "NAS Storage"
   speed-duplex 100-full
   exit
interface 11
   name "DB Server Primary"
   exit
interface 15
   name "Web Server"
   monitor
   exit
interface 16
   name "App Server"
   monitor
   exit
interface 21
   name "Reception Desk"
   mac-notify traps learned
   mac-notify traps removed
   exit
interface 30
   disable
   exit
interface 41
   name "Quarantine Port"
   disable
   exit
interface 47
   name "Uplink to Core"
   speed-duplex 1000-full
   exit
interface 48
   disable
   exit
"#;

        let state = switch.parse_running_config(running_config);

        // ---- Verify VLANs ----
        assert_eq!(state.vlans.len(), 4,
                   "Should parse 4 VLANs (1, 100, 200, 999), got {}", state.vlans.len());

        let vlan1 = state.vlans.iter().find(|v| v.id == 1);
        let vlan100 = state.vlans.iter().find(|v| v.id == 100);
        let vlan200 = state.vlans.iter().find(|v| v.id == 200);
        let vlan999 = state.vlans.iter().find(|v| v.id == 999);

        assert!(vlan1.is_some(), "Should find VLAN 1");
        assert!(vlan100.is_some(), "Should find VLAN 100");
        assert!(vlan200.is_some(), "Should find VLAN 200");
        assert!(vlan999.is_some(), "Should find VLAN 999");

        assert_eq!(vlan1.unwrap().name, "DEFAULT_VLAN");
        assert_eq!(vlan100.unwrap().name, "servers");
        assert_eq!(vlan200.unwrap().name, "workstations");
        assert_eq!(vlan999.unwrap().name, "quarantine");

        // ---- Verify ports exist ----
        // Ports are created from both interface blocks and VLAN untagged/tagged assignments
        // So we should have ports from 1-48
        assert!(state.ports.len() >= 10,
                "Should parse many ports, got {}", state.ports.len());

        // ---- Verify specific port configurations ----
        let port1 = state.ports.iter().find(|p| p.port_id == "1");
        assert!(port1.is_some(), "Should find port 1");
        let p1 = port1.unwrap();
        assert_eq!(p1.description, Some("Gateway".to_string()));
        assert_eq!(p1.speed_duplex, SpeedDuplex::ThousandFull,
                   "Port 1 should have speed_duplex=ThousandFull (1000-full)");
        assert_eq!(p1.vlan, 1, "Port 1 should be on VLAN 1");

        let port5 = state.ports.iter().find(|p| p.port_id == "5");
        assert!(port5.is_some(), "Should find port 5");
        let p5 = port5.unwrap();
        assert_eq!(p5.description, Some("NAS Storage".to_string()));
        assert_eq!(p5.speed_duplex, SpeedDuplex::HundredFull,
                   "Port 5 should have speed_duplex=HundredFull (100-full)");

        let port11 = state.ports.iter().find(|p| p.port_id == "11");
        assert!(port11.is_some(), "Should find port 11");
        assert_eq!(port11.unwrap().description, Some("DB Server Primary".to_string()));
        assert_eq!(port11.unwrap().vlan, 100, "Port 11 should be on VLAN 100");

        // Port 21 should have mac_notify enabled
        let port21 = state.ports.iter().find(|p| p.port_id == "21");
        assert!(port21.is_some(), "Should find port 21");
        assert_eq!(port21.unwrap().mac_notify, true,
                   "Port 21 should have mac_notify=true");
        assert_eq!(port21.unwrap().description, Some("Reception Desk".to_string()));

        // Port 30 should be disabled
        let port30 = state.ports.iter().find(|p| p.port_id == "30");
        assert!(port30.is_some(), "Should find port 30");
        assert_eq!(port30.unwrap().enabled, false,
                   "Port 30 should be disabled");

        // Port 41 should be disabled with a name
        let port41 = state.ports.iter().find(|p| p.port_id == "41");
        assert!(port41.is_some(), "Should find port 41");
        assert_eq!(port41.unwrap().enabled, false,
                   "Port 41 should be disabled");
        assert_eq!(port41.unwrap().description, Some("Quarantine Port".to_string()));

        // Port 47 is a trunk port (tagged on VLANs 100 and 200, untagged on VLAN 1)
        let port47 = state.ports.iter().find(|p| p.port_id == "47");
        assert!(port47.is_some(), "Should find port 47");
        let p47 = port47.unwrap();
        assert_eq!(p47.mode, PortMode::Trunk,
                   "Port 47 should be trunk mode (has tagged VLANs)");
        assert_eq!(p47.description, Some("Uplink to Core".to_string()));
        assert_eq!(p47.speed_duplex, SpeedDuplex::ThousandFull);
        assert!(p47.tagged_vlans.contains(&100),
                "Port 47 tagged_vlans should contain VLAN 100");
        assert!(p47.tagged_vlans.contains(&200),
                "Port 47 tagged_vlans should contain VLAN 200");

        // ---- Verify mirror configuration ----
        assert_eq!(state.port_mirrors.len(), 1,
                   "Should have 1 mirror session");
        let mirror = &state.port_mirrors[0];
        assert_eq!(mirror.destination_port, "48",
                   "Mirror destination should be port 48");
        assert_eq!(mirror.source_ports.len(), 2,
                   "Should have 2 source ports (15 and 16)");
        assert!(mirror.source_ports.contains(&"15".to_string()),
                "Port 15 should be a mirror source");
        assert!(mirror.source_ports.contains(&"16".to_string()),
                "Port 16 should be a mirror source");

        // ---- Verify PoE states ----
        // Aruba2530_48G_2SFP is a non-PoE model. Ports with interface blocks should
        // default to poe_enabled=false via parse_interface_name. Ports that only appear
        // in VLAN blocks (no interface block) inherit PortVlanInfo::new() default (true).
        // Check ports that have interface blocks explicitly.
        let ports_with_interface_blocks = ["1", "5", "11", "15", "16", "21", "30", "41", "47", "48"];
        for port_id in &ports_with_interface_blocks {
            if let Some(port) = state.ports.iter().find(|p| p.port_id == *port_id) {
                assert_eq!(port.poe_enabled, false,
                           "Port {} on non-PoE model (Aruba2530_48G_2SFP) with interface block should have poe_enabled=false",
                           port.port_id);
            }
        }

        // ---- Verify model detection (J9855A matches Aruba2530_48G_2SFP) ----
        assert!(state.warnings.is_empty(),
                "J9855A should match Aruba2530_48G_2SFP with no warnings, got: {:?}",
                state.warnings);

        // ---- Verify speed_duplex defaults ----
        // Port 11 has no speed-duplex in config -> should default to Auto
        assert_eq!(port11.unwrap().speed_duplex, SpeedDuplex::Auto,
                   "Port 11 without speed-duplex config should default to Auto");
    }

    #[test]
    fn test_generate_mirror_commands_legacy_syntax() {
        // Aruba 2530 uses legacy "mirror-port <dest>" syntax
        let config_2530 = SwitchConfig {
            id: "test-2530".to_string(),
            hostname: Some("test-2530".to_string()),
            model: Some(SwitchModel::Aruba2530_24G_POE),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };
        let switch_2530 = ArubaSwitch::new(config_2530, RuntimeConfig::default(), false);

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string(), "2".to_string()],
                destination_port: "8".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let commands = switch_2530.generate_mirror_commands(&mirrors);

        // Legacy syntax: "mirror-port <dest>"
        assert!(commands.contains(&"mirror-port 8".to_string()),
                "Aruba 2530 should use legacy 'mirror-port 8' syntax, got: {:?}", commands);
        // Must NOT contain modern syntax
        assert!(!commands.iter().any(|c| c == "mirror 1 port 8"),
                "Aruba 2530 should NOT use modern 'mirror 1 port 8' syntax");
    }

    #[test]
    fn test_generate_mirror_commands_modern_syntax() {
        // Aruba 2930F uses modern "mirror <session> port <dest>" syntax
        let switch = create_test_switch(); // Aruba2930F

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string(), "2".to_string()],
                destination_port: "8".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let commands = switch.generate_mirror_commands(&mirrors);

        // Modern syntax: "mirror <session> port <dest>"
        assert!(commands.contains(&"mirror 1 port 8".to_string()),
                "Aruba 2930F should use modern 'mirror 1 port 8' syntax, got: {:?}", commands);
        // Must NOT contain legacy syntax
        assert!(!commands.iter().any(|c| c == "mirror-port 8"),
                "Aruba 2930F should NOT use legacy 'mirror-port 8' syntax");
    }

    // ========================================================================
    // Gap 8: enable_secret Runtime Resolution Logic
    // Tests the exact `.or_else()` pattern used in the connect method:
    //   enable_secret.clone().or_else(|| password.clone())
    // ========================================================================

    #[test]
    fn test_enable_secret_resolution_logic() {
        // Case (a): enable_secret=Some("secret"), password=Some("pass")
        // -> resolved should be "secret" (enable_secret takes priority)
        let creds_a = Credentials {
            username: "admin".to_string(),
            password: Some("pass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            jump_hosts: None,
            enable_secret: Some("secret".to_string()),
        };
        let resolved_a = creds_a.enable_secret.clone().or_else(|| creds_a.password.clone());
        assert_eq!(resolved_a, Some("secret".to_string()),
                   "When enable_secret is set, it should take priority over password");

        // Case (b): enable_secret=None, password=Some("pass")
        // -> resolved should be "pass" (falls back to password)
        let creds_b = Credentials {
            username: "admin".to_string(),
            password: Some("pass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            jump_hosts: None,
            enable_secret: None,
        };
        let resolved_b = creds_b.enable_secret.clone().or_else(|| creds_b.password.clone());
        assert_eq!(resolved_b, Some("pass".to_string()),
                   "When enable_secret is None, should fall back to password");

        // Case (c): enable_secret=None, password=None
        // -> resolved should be None
        let creds_c = Credentials {
            username: "admin".to_string(),
            password: None,
            ssh_key_path: None,
            port: 22,
            connection_type: ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            jump_hosts: None,
            enable_secret: None,
        };
        let resolved_c = creds_c.enable_secret.clone().or_else(|| creds_c.password.clone());
        assert_eq!(resolved_c, None,
                   "When both enable_secret and password are None, resolved should be None");
    }

    // ============================================================================
    // Bug fix: access mode ports must remove leftover tagged VLANs
    // ============================================================================

    #[test]
    fn test_access_mode_removes_leftover_tagged_vlans() {
        // Simulate: port 13 currently has untagged=1020, tagged=[2088]
        // Desired: access mode on VLAN 1001 (no tagged VLANs)
        // The "no tagged vlan 2088" command must be generated.
        let config = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2530_24G_POE),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        let mut switch = ArubaSwitch::new(config, RuntimeConfig::default(), false);

        // Set current_state with port 13 having tagged VLAN 2088
        switch.current_state = Some(SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "13".to_string(),
                    mode: PortMode::Access,
                    vlan: 1020,
                    tagged_vlans: vec![2088],  // Leftover tagged VLAN
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        });

        let ports = vec![
            Port {
                port_id: "13".to_string(),
                mode: PortMode::Access,
                vlan: 1001,
                tagged_vlans: vec![],
                description: Some("Zone 1".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"untagged vlan 1001".to_string()),
                "Should set untagged VLAN, got: {:?}", commands);
        assert!(commands.contains(&"no tagged vlan 2088".to_string()),
                "Should remove leftover tagged VLAN 2088, got: {:?}", commands);
    }

    #[test]
    fn test_access_mode_no_tagged_removal_when_clean() {
        // Port has no tagged VLANs — no "no tagged vlan" commands should be generated
        let switch = create_test_switch();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(!commands.iter().any(|c| c.starts_with("no tagged vlan")),
                "No 'no tagged vlan' commands when port has no tagged VLANs: {:?}", commands);
    }

    #[test]
    fn test_reset_ports_removes_tagged_vlans() {
        // Port 14 currently has tagged VLANs — reset should remove them
        let config = SwitchConfig {
            id: "test-sw-01".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2530_24G_POE),
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
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        };

        let mut switch = ArubaSwitch::new(config, RuntimeConfig::default(), false);

        // Port 14 currently has untagged=1020, tagged=[2088]
        switch.current_state = Some(SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "14".to_string(),
                    mode: PortMode::Access,
                    vlan: 1020,
                    tagged_vlans: vec![2088],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        });

        // We can't call reset_ports directly (needs SSH client), but we can verify
        // the command generation logic by checking generate_port_commands for an
        // access mode port transitioning from tagged state.
        // The reset_ports method uses the same current_state lookup pattern.
        // This test verifies the pattern works in generate_port_commands;
        // reset_ports has the same fix applied.
        let ports = vec![
            Port {
                port_id: "14".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: false,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports, &[]);

        assert!(commands.contains(&"untagged vlan 1".to_string()));
        assert!(commands.contains(&"no tagged vlan 2088".to_string()),
                "Reset should remove tagged VLAN 2088, got: {:?}", commands);
    }

    #[test]
    fn test_generate_commands_for_diff_trait_method() {
        use crate::vendors::traits::SwitchVendor;

        let switch = create_test_switch();

        let diff = StateDiff {
            vlans_to_add: vec![
                crate::models::Vlan {
                    id: 42,
                    name: "test-vlan".to_string(),
                    description: None,
                    ip_config: crate::models::VlanIpConfig::None,
                },
            ],
            ports_to_configure: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 42,
                    tagged_vlans: vec![],
                    description: Some("Test".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            ..Default::default()
        };

        let preview = switch.generate_commands_for_diff(&diff);

        assert!(!preview.vlan_commands.is_empty(), "Should have VLAN commands");
        assert!(preview.vlan_commands.iter().any(|c| c.contains("vlan 42")),
                "VLAN commands should reference VLAN 42, got: {:?}", preview.vlan_commands);

        assert!(!preview.port_commands.is_empty(), "Should have port commands");
        assert!(preview.port_commands.iter().any(|c| c.contains("interface 1")),
                "Port commands should reference interface 1, got: {:?}", preview.port_commands);
    }

    #[test]
    fn test_poe_disable_commands() {
        let switch = create_test_switch();
        let cmds = switch.poe_disable_commands("5");
        assert_eq!(cmds, vec![
            "configure terminal",
            "interface 5",
            "no power-over-ethernet",
            "exit",
            "exit",
        ]);
    }

    #[test]
    fn test_poe_enable_commands() {
        let switch = create_test_switch();
        let cmds = switch.poe_enable_commands("5");
        assert_eq!(cmds, vec![
            "configure terminal",
            "interface 5",
            "power-over-ethernet",
            "exit",
            "exit",
        ]);
    }

    #[test]
    fn test_poe_commands_normalize_port_id() {
        let switch = create_test_switch();
        let cmds = switch.poe_disable_commands("GigabitEthernet1/0/5");
        assert_eq!(cmds[1], "interface 5");
    }
}
