use super::traits::{SwitchVendor, VendorError};
use crate::config::RuntimeConfig;
use crate::models::{
    ConfigResult, MirrorDirection, Port, PortMirror, PortMode, SnmpAccess, SnmpCommunity,
    SnmpConfig, SnmpTrapReceiver, SpeedDuplex, StateDiff, SwitchConfig, SwitchState, TrapType,
    Vlan, VlanIpConfig, ConnectionType,
};
use crate::ssh::{ConnectionClient, SerialClient, SshClient};
use async_trait::async_trait;
use tracing::{debug, info, trace, warn};

pub struct FortiswitchSwitch {
    config: SwitchConfig,
    runtime_config: RuntimeConfig,
    client: Option<ConnectionClient>,
    enforce_port_config: bool,
    current_state: Option<SwitchState>,
    /// Community name -> numeric `edit <N>` index, from the most recent
    /// `parse_current_state`. FortiOS's `config system snmp community` is
    /// index-based; a community's name is just a `set name` field inside
    /// the entry, not something `delete` accepts. Confirmed on real
    /// hardware: `delete <name>` is silently a no-op — the community is
    /// still there on the next read. `apply_snmp_diff` needs this to issue
    /// `delete <index>` instead.
    snmp_community_indices: std::collections::HashMap<String, u16>,
}

/// Raw `show` output for every FortiOS CLI block `parse_current_state` reads.
struct FortiSwitchConfigBlocks {
    system_interface: String,
    switch_vlan: String,
    switch_interface: String,
    physical_port: String,
    switch_mirror: String,
    snmp_community: String,
}

/// A VLAN's Layer-3 interface details, when it has one (see `parse_svi_details`).
#[derive(Debug, Clone, Default, PartialEq)]
struct SviDetails {
    ip_config: VlanIpConfig,
    description: Option<String>,
}

/// One port's VLAN membership, from `config switch interface` (see
/// `parse_switch_interface_ports`).
#[derive(Debug, Clone, Default, PartialEq)]
struct PortInterfaceInfo {
    description: Option<String>,
    native_vlan: Option<u16>,
    allowed_vlans: Vec<u16>,
}

/// One port's physical properties, from `config switch physical-port` (see
/// `parse_physical_ports`). Each field is `None` when its `set` line simply
/// never appears in the output — confirmed on real hardware (FortiSwitch
/// 124F-FPOE): FortiOS's `show` only prints settings that differ from the
/// device's own default, so "not mentioned" means "at whatever this
/// switch's default is", not "off". `build_ports` resolves each `None` to
/// that default rather than to a fixed fallback.
#[derive(Debug, Clone, Default, PartialEq)]
struct PhysicalPortInfo {
    enabled: Option<bool>,
    poe_enabled: Option<bool>,
    speed_duplex: Option<SpeedDuplex>,
}

impl FortiswitchSwitch {
    pub fn new(config: SwitchConfig, runtime_config: RuntimeConfig, enforce_port_config: bool) -> Self {
        Self {
            config,
            runtime_config,
            client: None,
            enforce_port_config,
            current_state: None,
            snmp_community_indices: std::collections::HashMap::new(),
        }
    }

    fn generate_vlan_commands(&self, vlans: &[Vlan]) -> Vec<String> {
        let mut commands = Vec::new();

        // Phase 1: Create VLANs in VLAN database (Layer 2)
        // FortiSwitch does NOT support 'set name' or 'set description' in this context
        // VLANs are created by ID only
        commands.push("config switch vlan".to_string());
        for vlan in vlans {
            commands.push(format!("edit {}", vlan.id));
            // No additional configuration needed - VLAN created by edit alone
            commands.push("next".to_string());
        }
        commands.push("end".to_string());

        // Phase 2: Create VLAN interfaces (SVIs) for Layer 3 functionality if IP config exists
        // This gives VLANs names and allows IP configuration
        for vlan in vlans {
            let needs_svi = match &vlan.ip_config {
                crate::models::VlanIpConfig::None => false,
                _ => true,
            };

            if needs_svi {
                commands.push("config system interface".to_string());
                commands.push(format!("edit vlan{}", vlan.id));
                commands.push(format!("set vlanid {}", vlan.id));

                if let Some(desc) = &vlan.description {
                    commands.push(format!("set description \"{}\"", desc));
                }

                commands.push("set type vlan".to_string());
                commands.push("set interface internal".to_string());

                // Configure IP based on ip_config
                match &vlan.ip_config {
                    crate::models::VlanIpConfig::Dhcp => {
                        commands.push("set mode dhcp".to_string());
                        commands.push("set allowaccess ping".to_string());
                    }
                    crate::models::VlanIpConfig::Static { address, netmask } => {
                        commands.push(format!("set ip {} {}", address, netmask));
                        commands.push("set allowaccess ping".to_string());
                    }
                    crate::models::VlanIpConfig::None => {
                        // No IP configuration
                    }
                }

                commands.push("next".to_string());
                commands.push("end".to_string());
            }
        }

        commands
    }

    fn generate_port_commands(&self, ports: &[Port]) -> Vec<String> {
        let mut commands = Vec::new();

        // Phase 1: VLAN assignments (config switch interface)
        commands.push("config switch interface".to_string());
        for port in ports {
            let interface = self.normalize_port_id(&port.port_id);
            commands.push(format!("edit {}", interface));

            if let Some(desc) = &port.description {
                commands.push(format!("set description \"{}\"", desc));
            }

            match port.inferred_mode() {
                PortMode::Access => {
                    // Access port: native VLAN is untagged, only that VLAN allowed
                    commands.push(format!("set native-vlan {}", port.vlan));
                    commands.push(format!("set allowed-vlans {}", port.vlan));
                    commands.push(format!("set untagged-vlans {}", port.vlan));
                }
                PortMode::Trunk => {
                    // Trunk port: native VLAN + multiple allowed VLANs
                    commands.push(format!("set native-vlan {}", port.vlan));
                    if !port.tagged_vlans.is_empty() {
                        let vlans: Vec<String> =
                            port.tagged_vlans.iter().map(|v| v.to_string()).collect();
                        commands.push(format!("set allowed-vlans {}", vlans.join(" ")));
                    }
                    // Native VLAN is untagged
                    commands.push(format!("set untagged-vlans {}", port.vlan));
                }
            }

            commands.push("next".to_string());
        }
        commands.push("end".to_string());

        // Phase 2: Physical port properties (config switch physical-port)
        commands.push("config switch physical-port".to_string());
        for port in ports {
            let interface = self.normalize_port_id(&port.port_id);
            commands.push(format!("edit {}", interface));

            // Port status (up/down)
            if port.enabled {
                commands.push("set status up".to_string());
            } else {
                commands.push("set status down".to_string());
            }

            // PoE configuration
            if port.poe_enabled {
                commands.push("set poe-status enable".to_string());
            } else {
                commands.push("set poe-status disable".to_string());
            }

            // Speed and duplex
            let speed = self.convert_speed_to_fortiswitch(&port.speed_duplex);
            commands.push(format!("set speed {}", speed));

            commands.push("next".to_string());
        }
        commands.push("end".to_string());

        commands
    }

    fn convert_speed_to_fortiswitch(&self, speed_duplex: &crate::models::SpeedDuplex) -> String {
        use crate::models::SpeedDuplex;
        match speed_duplex {
            SpeedDuplex::Auto => "auto".to_string(),
            SpeedDuplex::TenHalf => "10half".to_string(),
            SpeedDuplex::TenFull => "10full".to_string(),
            SpeedDuplex::HundredHalf => "100half".to_string(),
            SpeedDuplex::HundredFull => "100full".to_string(),
            SpeedDuplex::ThousandFull => "1000full".to_string(),
            SpeedDuplex::TenGFull => "10000full".to_string(),
        }
    }

    /// Parse management VLAN from FortiSwitch running config
    /// Detects VLAN interfaces with IP configuration and allowaccess settings
    /// Returns the VLAN ID if a VLAN interface with IP and management access is found
    fn parse_management_vlan(&self, lines: &[&str]) -> Option<u16> {
        let mut in_system_interface = false;
        let mut in_vlan_interface = false;
        let mut current_vlan_id: Option<u16> = None;
        let mut has_ip = false;
        let mut has_allowaccess = false;
        let mut nesting_depth = 0;  // Track nested config blocks

        for line in lines {
            let trimmed = line.trim();

            // Detect "config system interface"
            if trimmed == "config system interface" {
                in_system_interface = true;
                nesting_depth = 1;  // Start at depth 1
                debug!("  Entering system interface config block (depth=1)");
                continue;
            }

            if in_system_interface {
                // Track nested config blocks (e.g., "config secondaryip")
                if trimmed.starts_with("config ") {
                    nesting_depth += 1;
                    debug!("  Entering nested config block (depth={})", nesting_depth);
                    continue;
                }

                // Detect "end" - exiting a config block
                if trimmed == "end" {
                    nesting_depth -= 1;
                    debug!("  Exiting config block (depth={})", nesting_depth);

                    // Only exit in_system_interface when we exit the top-level block
                    if nesting_depth == 0 {
                        // Check if we found a complete management VLAN before exiting
                        if in_vlan_interface && has_ip && has_allowaccess && current_vlan_id.is_some() {
                            debug!("  Detected management VLAN at end: {:?}", current_vlan_id);
                            return current_vlan_id;
                        }
                        in_system_interface = false;
                        in_vlan_interface = false;
                        current_vlan_id = None;
                        has_ip = false;
                        has_allowaccess = false;
                    }
                    continue;
                }
            }

            if in_system_interface {
                // Detect "edit vlan<id>" or "edit "vlan<id>""
                // Try unquoted format first: edit vlan77
                if let Some(rest) = trimmed.strip_prefix("edit vlan") {
                    if let Some(vlan_str) = rest.split_whitespace().next() {
                        if let Ok(vlan_id) = vlan_str.parse::<u16>() {
                            in_vlan_interface = true;
                            current_vlan_id = Some(vlan_id);
                            has_ip = false;
                            has_allowaccess = false;
                            debug!("  Found VLAN interface: vlan{}", vlan_id);
                        }
                    }
                }
                // Try quoted format: edit "vlan77"
                else if let Some(rest) = trimmed.strip_prefix("edit ") {
                    debug!("    After 'edit ': rest='{}'", rest);
                    // Check if rest starts with quote: "vlan77"
                    if rest.starts_with("\"vlan") {
                        debug!("    Starts with '\"vlan'");
                        // Extract number from "vlan77"
                        if let Some(vlan_start) = rest.strip_prefix("\"vlan") {
                            debug!("    vlan_start='{}'", vlan_start);
                            if let Some(vlan_end) = vlan_start.find('"') {
                                let vlan_str = &vlan_start[..vlan_end];
                                debug!("    vlan_str='{}', trying to parse", vlan_str);
                                if let Ok(vlan_id) = vlan_str.parse::<u16>() {
                                    in_vlan_interface = true;
                                    current_vlan_id = Some(vlan_id);
                                    has_ip = false;
                                    has_allowaccess = false;
                                    debug!("  Found VLAN interface (quoted): vlan{} - will look for IP", vlan_id);
                                } else {
                                    debug!("    Parse failed for '{}'", vlan_str);
                                }
                            }
                        }
                    }
                } else if in_vlan_interface {
                    // DEBUG: Log what we see inside vlan interface
                    if current_vlan_id == Some(77) {
                        debug!("    vlan77 line: {}", trimmed);
                    }

                    // Check for IP configuration
                    // Accept: "set ip ...", "set mode dhcp", or "set mode static"
                    if trimmed.starts_with("set ip ") || trimmed == "set mode dhcp" || trimmed == "set mode static" {
                        has_ip = true;
                        debug!("    Found IP configuration on vlan{:?}", current_vlan_id);
                    }
                    // Check for allowaccess configuration with management access (SSH/HTTPS)
                    else if trimmed.starts_with("set allowaccess ") {
                        // Only consider it management access if it includes SSH or HTTPS
                        if trimmed.contains("ssh") || trimmed.contains("https") {
                            has_allowaccess = true;
                            debug!("    Found management allowaccess on vlan{:?}: {}", current_vlan_id, trimmed);
                        }
                    }
                    // Detect "next" - end of current interface
                    else if trimmed == "next" {
                        // Management VLAN must have both IP and management-level access (SSH/HTTPS)
                        if has_ip && has_allowaccess && current_vlan_id.is_some() {
                            debug!("  Detected management VLAN: {:?}", current_vlan_id);
                            return current_vlan_id;
                        }
                        // Reset for next interface
                        in_vlan_interface = false;
                        current_vlan_id = None;
                        has_ip = false;
                        has_allowaccess = false;
                    }
                }
            }
        }

        // Check if the last interface had IP (in case it's at the end)
        if in_vlan_interface && has_ip && current_vlan_id.is_some() {
            debug!("  Detected management VLAN (at end): {:?}", current_vlan_id);
            return current_vlan_id;
        }

        None
    }

    fn generate_mirror_commands(&self, mirrors: &[PortMirror]) -> Vec<String> {
        let mut commands = vec!["config switch mirror".to_string()];

        for mirror in mirrors {
            commands.push(format!("edit {}", mirror.session_id));

            // Set status
            commands.push("set status active".to_string());

            // Configure destination
            let dest = self.normalize_port_id(&mirror.destination_port);
            commands.push(format!("set dst {}", dest));

            // Configure source ports
            let sources: Vec<String> = mirror
                .source_ports
                .iter()
                .map(|s| self.normalize_port_id(s))
                .collect();

            match mirror.direction {
                MirrorDirection::Rx => {
                    commands.push(format!("set src-ingress {}", sources.join(" ")));
                }
                MirrorDirection::Tx => {
                    commands.push(format!("set src-egress {}", sources.join(" ")));
                }
                MirrorDirection::Both => {
                    commands.push(format!("set src-ingress {}", sources.join(" ")));
                    commands.push(format!("set src-egress {}", sources.join(" ")));
                }
            }

            commands.push("next".to_string());
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_snmp_commands(&self, snmp_config: &crate::models::SnmpConfig) -> Vec<String> {
        let mut commands = Vec::new();

        // Configure SNMP communities with trap receivers
        commands.push("config system snmp community".to_string());

        // If we have trap receivers, configure them within the first community
        if !snmp_config.trap_receivers.is_empty() && !snmp_config.communities.is_empty() {
            // Configure first community with trap receivers
            let community = &snmp_config.communities[0];
            commands.push("edit 1".to_string());
            commands.push(format!("set name \"{}\"", community.name));

            // Enable query and trap statuses
            commands.push("set status enable".to_string());
            commands.push("set query-v1-status enable".to_string());
            commands.push("set query-v2c-status enable".to_string());
            commands.push("set trap-v1-status enable".to_string());
            commands.push("set trap-v2c-status enable".to_string());

            // Configure trap events if specified
            if !snmp_config.enabled_traps.is_empty() {
                let events: Vec<String> = snmp_config
                    .enabled_traps
                    .iter()
                    .map(|t| self.convert_trap_type_to_fortiswitch(t))
                    .collect();
                commands.push(format!("set events {}", events.join(" ")));
            }

            // Configure trap receiver hosts within this community
            commands.push("config hosts".to_string());
            for (idx, receiver) in snmp_config.trap_receivers.iter().enumerate() {
                commands.push(format!("edit {}", idx + 1));
                commands.push(format!("set ip {}", receiver.host));
                commands.push("set interface internal".to_string());
                commands.push("next".to_string());
            }
            commands.push("end".to_string()); // End config hosts

            commands.push("next".to_string());

            // Configure remaining communities (if any) without trap receivers
            for (idx, community) in snmp_config.communities.iter().skip(1).enumerate() {
                commands.push(format!("edit {}", idx + 2));
                commands.push(format!("set name \"{}\"", community.name));
                commands.push("set status enable".to_string());
                commands.push("set query-v1-status enable".to_string());
                commands.push("set query-v2c-status enable".to_string());
                commands.push("next".to_string());
            }
        } else {
            // No trap receivers - just configure communities
            for (idx, community) in snmp_config.communities.iter().enumerate() {
                commands.push(format!("edit {}", idx + 1));
                commands.push(format!("set name \"{}\"", community.name));
                commands.push("set status enable".to_string());
                commands.push("set query-v1-status enable".to_string());
                commands.push("set query-v2c-status enable".to_string());
                commands.push("next".to_string());
            }
        }

        commands.push("end".to_string());

        commands
    }

    fn convert_trap_type_to_fortiswitch(&self, trap: &crate::models::TrapType) -> String {
        use crate::models::TrapType;
        match trap {
            TrapType::MacNotify => "mac-notify".to_string(),
            TrapType::LinkChange => "link-up-down".to_string(),
            TrapType::All => "all".to_string(),
        }
    }

    fn normalize_port_id(&self, port_id: &str) -> String {
        // FortiSwitch uses formats like "port1", "port2", etc.
        if port_id.starts_with("port") {
            return port_id.to_string();
        }

        // Convert simple format "1" or "1/0/1" to FortiSwitch format
        if let Some(last) = port_id.split('/').last() {
            return format!("port{}", last);
        }

        format!("port{}", port_id)
    }

    fn generate_remove_vlan_commands(&self, vlan_ids: &[u16]) -> Vec<String> {
        let mut commands = vec!["config switch vlan".to_string()];

        for vlan_id in vlan_ids {
            commands.push(format!("delete {}", vlan_id));
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_remove_mirror_commands(&self, session_ids: &[String]) -> Vec<String> {
        let mut commands = vec!["config switch mirror".to_string()];

        for session_id in session_ids {
            commands.push(format!("delete {}", session_id));
        }

        commands.push("end".to_string());
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

    async fn configure_snmp(
        &mut self,
        snmp_config: &crate::models::SnmpConfig,
    ) -> Result<ConfigResult, VendorError> {
        let commands = self.generate_snmp_commands(snmp_config);
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
            message: "Configured SNMP settings".to_string(),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Apply granular SNMP diff - only changes what's necessary
    /// FortiSwitch SNMP uses a more complex nested structure than Aruba/Cisco,
    /// so we need to handle communities and hosts differently
    async fn apply_snmp_diff(
        &mut self,
        snmp_diff: &crate::models::SnmpStateDiff,
    ) -> Result<ConfigResult, VendorError> {
        let mut commands = Vec::new();
        let mut actions = Vec::new();

        // Only proceed if we have changes
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

        // FortiSwitch SNMP uses "config system snmp community" -> "edit X" structure
        commands.push("config system snmp community".to_string());

        // Remove communities. FortiSwitch's "config system snmp community"
        // is index-based — `delete <name>` is silently a no-op (confirmed
        // on real hardware: the community was still present on the next
        // read). Delete by the numeric index recorded from the most recent
        // parse_current_state; fall back to the name if we never saw this
        // community there (shouldn't happen for something we're trying to
        // remove, but better than sending nothing).
        for community_name in &snmp_diff.communities_to_remove {
            info!("Removing SNMP community: {}", community_name);
            let target = self.snmp_community_indices.get(community_name)
                .map(|idx| idx.to_string())
                .unwrap_or_else(|| community_name.clone());
            commands.push(format!("delete {}", target));
            actions.push(format!("removed community '{}'", community_name));
        }

        // Add new communities
        for (idx, community) in snmp_diff.communities_to_add.iter().enumerate() {
            info!("Adding SNMP community: {}", community.name);
            // Use a high index to avoid conflicts
            let edit_idx = 100 + idx;
            commands.push(format!("edit {}", edit_idx));
            commands.push(format!("set name \"{}\"", community.name));
            commands.push("set status enable".to_string());
            commands.push("set query-v1-status enable".to_string());
            commands.push("set query-v2c-status enable".to_string());
            commands.push("set trap-v1-status enable".to_string());
            commands.push("set trap-v2c-status enable".to_string());
            commands.push("next".to_string());
            actions.push(format!("added community '{}'", community.name));
        }

        // Update existing communities (mainly for trap receivers)
        for community in &snmp_diff.communities_to_update {
            info!("Updating SNMP community: {}", community.name);
            // Would need to find existing index - for now just add new entry
            commands.push("edit 1".to_string()); // Assuming first community
            commands.push(format!("set name \"{}\"", community.name));
            commands.push("set status enable".to_string());
            commands.push("next".to_string());
            actions.push(format!("updated community '{}'", community.name));
        }

        commands.push("end".to_string());

        // Handle trap receivers - these are nested under communities in FortiSwitch
        // For simplicity, we'll add new receivers to community 1
        if !snmp_diff.trap_receivers_to_add.is_empty() {
            commands.push("config system snmp community".to_string());
            commands.push("edit 1".to_string());
            commands.push("config hosts".to_string());

            for (idx, receiver) in snmp_diff.trap_receivers_to_add.iter().enumerate() {
                info!("Adding SNMP trap receiver: {}", receiver.host);
                let host_idx = 100 + idx; // Use high index to avoid conflicts
                commands.push(format!("edit {}", host_idx));
                commands.push(format!("set ip {}", receiver.host));
                commands.push("set interface internal".to_string());
                commands.push("next".to_string());
                actions.push(format!("added trap receiver '{}'", receiver.host));
            }

            commands.push("end".to_string()); // End config hosts
            commands.push("next".to_string());
            commands.push("end".to_string()); // End config snmp community
        }

        // Handle trap receiver removals
        for host in &snmp_diff.trap_receivers_to_remove {
            info!("Removing SNMP trap receiver: {}", host);
            // FortiSwitch requires finding the host index - simplified approach
            commands.push("config system snmp community".to_string());
            commands.push("edit 1".to_string());
            commands.push("config hosts".to_string());
            // Note: Would need to find the index by IP in real implementation
            commands.push(format!("delete {}", host)); // May not work directly
            commands.push("end".to_string());
            commands.push("next".to_string());
            commands.push("end".to_string());
            actions.push(format!("removed trap receiver '{}'", host));
        }

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
}

#[async_trait]
impl SwitchVendor for FortiswitchSwitch {
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

                // Exit any existing config context first (FortiSwitch may be in config mode from previous session)
                let _ = ssh_client.execute_command("end").await;
                let _ = ssh_client.execute_command("end").await; // Second end in case nested

                // FortiSwitch pagination control: a long `show` response (e.g.
                // `show switch interface` on a switch with many ports) triggers
                // a "--More--" pager prompt that the client never advances past,
                // hanging until the read times out. Confirmed on real hardware
                // (FortiSwitch 124F-FPOE) once `parse_current_state` started
                // issuing multi-block `show` commands. Best-effort: some models
                // may not support this exact command, so failures are ignored,
                // same as the "end" calls above.
                let _ = ssh_client.execute_command("config system console").await;
                let _ = ssh_client.execute_command("set output standard").await;
                let _ = ssh_client.execute_command("end").await;

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

                // Exit any existing config context first (FortiSwitch may be in config mode from previous session)
                // This ensures we're at the root prompt before attempting configuration
                let _ = serial_client.execute_command("end").await;
                let _ = serial_client.execute_command("end").await; // Second end in case nested

                // FortiSwitch pagination control: a long `show` response (e.g.
                // `show switch interface` on a switch with many ports) triggers
                // a "--More--" pager prompt that the client never advances past,
                // hanging until the read times out. Confirmed on real hardware
                // (FortiSwitch 124F-FPOE) once `parse_current_state` started
                // issuing multi-block `show` commands. Best-effort: some models
                // may not support this exact command, so failures are ignored,
                // same as the "end" calls above.
                let _ = serial_client.execute_command("config system console").await;
                let _ = serial_client.execute_command("set output standard").await;
                let _ = serial_client.execute_command("end").await;

                ConnectionClient::Serial(serial_client)
            }
        };

        self.client = Some(client);
        info!("Connected to FortiSwitch: {}", self.config.hostname());
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
        let raw = self.get_running_config_blocks().await?;

        // Serial connections commonly carry ANSI escape sequences; strip them
        // before parsing, same as the Aruba parser does.
        let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
        let clean = |s: &str| ansi_regex.replace_all(s, "").to_string();
        let system_interface = clean(&raw.system_interface);
        let switch_vlan = clean(&raw.switch_vlan);
        let switch_interface = clean(&raw.switch_interface);
        let physical_port = clean(&raw.physical_port);
        let switch_mirror = clean(&raw.switch_mirror);
        let snmp_community = clean(&raw.snmp_community);

        // TEMP DIAGNOSTIC: full raw block content, to debug real-hardware
        // parsing discrepancies. Trace level only (not printed by default).
        trace!("RAW show system interface:\n{}", system_interface);
        trace!("RAW show switch vlan:\n{}", switch_vlan);
        trace!("RAW show switch interface:\n{}", switch_interface);
        trace!("RAW show switch physical-port:\n{}", physical_port);
        trace!("RAW show switch mirror:\n{}", switch_mirror);
        trace!("RAW show system snmp community:\n{}", snmp_community);

        let system_interface_lines: Vec<&str> = system_interface.lines().collect();

        // Parse management VLAN (detect VLAN interfaces with IP and allowaccess)
        let management_vlan = self.parse_management_vlan(&system_interface_lines);

        // Every other VLAN detail (ip_config, description) the device can
        // actually tell us also comes from this same block.
        let svi_details = self.parse_svi_details(&system_interface_lines);

        // The VLAN database (`config switch vlan`) is the only place a VLAN
        // with no Layer-3 interface still shows up — and the only reliable
        // source of "does this id exist on the device at all".
        let vlan_ids = self.parse_switch_vlan_ids(&switch_vlan.lines().collect::<Vec<&str>>());

        // FortiSwitch never persists a VLAN's *name* to the device (see
        // `generate_vlan_commands`) — only its numeric id, and optionally a
        // Layer-3 interface description. So a VLAN's name always comes from
        // the desired config when we know it; the id itself is the one
        // thing genuinely read back from hardware.
        let mut vlans = Vec::new();
        for id in vlan_ids {
            let svi = svi_details.get(&id);
            let desired = self.config.vlans.iter().find(|v| v.id == id);

            let name = desired
                .map(|v| v.name.clone())
                .or_else(|| svi.and_then(|s| s.description.clone()))
                .unwrap_or_else(|| format!("vlan{}", id));
            let description = svi
                .and_then(|s| s.description.clone())
                .or_else(|| desired.and_then(|v| v.description.clone()));
            let ip_config = svi
                .map(|s| s.ip_config.clone())
                .unwrap_or(VlanIpConfig::None);

            vlans.push(Vlan { id, name, description, ip_config });
        }

        let interfaces = self.parse_switch_interface_ports(&switch_interface.lines().collect::<Vec<&str>>());
        let physical = self.parse_physical_ports(&physical_port.lines().collect::<Vec<&str>>());
        let ports = self.build_ports(interfaces, physical);

        let port_mirrors = self.parse_switch_mirrors(&switch_mirror.lines().collect::<Vec<&str>>());
        let snmp_community_lines: Vec<&str> = snmp_community.lines().collect();
        let snmp = self.parse_snmp_community(&snmp_community_lines);
        self.snmp_community_indices = Self::parse_snmp_community_indices(&snmp_community_lines);

        // Verify hardware model by running "get system status"
        // This returns lines like "Version: FortiSwitch-108F-POE v7.2.8,build0660,..."
        let warnings = self.detect_hardware_model().await;

        debug!(
            "Parsed FortiSwitch state for {}: {} VLANs, {} ports, {} mirrors, SNMP: {}, Management VLAN: {:?}",
            self.config.hostname(),
            vlans.len(),
            ports.len(),
            port_mirrors.len(),
            if snmp.is_some() { "configured" } else { "not configured" },
            management_vlan
        );

        Ok(SwitchState {
            vlans,
            ports,
            port_mirrors,
            snmp,
            management_vlan,
            warnings,
        })
    }

    async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>, VendorError> {
        let mut results = Vec::new();

        // Remove old VLANs
        if !diff.vlans_to_remove.is_empty() {
            debug!("Removing {} VLANs", diff.vlans_to_remove.len());
            results.push(self.remove_vlans(&diff.vlans_to_remove).await?);
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
                results.push(self.apply_snmp_diff(snmp_diff).await?);
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
        let commands = self.generate_port_commands(ports);
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
            preview.port_commands.extend(self.generate_port_commands(&diff.ports_to_configure));
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
        if let Some(snmp_config) = &diff.snmp_config {
            if diff.snmp_config_changed {
                preview.snmp_commands.extend(self.generate_snmp_commands(snmp_config));
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

        // Store current state for warnings retrieval
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
            .execute_command("execute backup config flash default-config")
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

        // Use "show system interface" which includes VLAN interfaces
        // (show full-configuration doesn't include dynamically created VLAN interfaces)
        let config = client
            .execute_command("show system interface")
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

                // For FortiSwitch, use 'execute reboot'
                client
                    .execute_command("execute reboot")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Reboot failed: {}", e)))?;

                info!("Reboot initiated on {}", self.config.hostname());
            }
            RollbackMethod::RestoreBackup => {
                info!("Restoring configuration from saved config");

                // For FortiSwitch, restore from last saved config
                client
                    .execute_command("execute restore config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Restore failed: {}", e)))?;

                info!("Configuration restored on {}", self.config.hostname());
            }
            RollbackMethod::RevertCommands => {
                warn!("Revert commands method not fully implemented for FortiSwitch, using restore backup instead");

                // Fallback to restore backup
                client
                    .execute_command("execute restore config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Revert failed: {}", e)))?;

                info!("Configuration reverted on {}", self.config.hostname());
            }
        }

        Ok(())
    }
}

// Additional helper methods for FortiswitchSwitch
impl FortiswitchSwitch {
    /// Every FortiOS CLI block `parse_current_state` needs, fetched with one
    /// `show` command per block. FortiOS's `show` output for a config path
    /// mirrors the `config ... edit ... set ... next ... end` syntax used to
    /// write it (already relied on by `parse_management_vlan` for `show
    /// system interface`), so each block below is parsed with the same
    /// nested-`config`/`end` walk.
    ///
    /// Distinct from the `get_running_config` trait method (a single-string
    /// "show me the raw config" used elsewhere, e.g. the dashboard's raw
    /// config view) — that one keeps its original single-command behavior
    /// unchanged; this is only for `parse_current_state`'s own use.
    async fn get_running_config_blocks(&mut self) -> Result<FortiSwitchConfigBlocks, VendorError> {
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        // "show system interface" includes VLAN Layer-3 interfaces
        // (show full-configuration doesn't include dynamically created VLAN interfaces).
        let system_interface = client
            .execute_command("show system interface")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        // The VLAN database — the only place a VLAN with no Layer-3
        // interface still shows up.
        let switch_vlan = client
            .execute_command("show switch vlan")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        // Per-port VLAN membership (native/allowed VLANs, description).
        let switch_interface = client
            .execute_command("show switch interface")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        // Per-port physical properties (status, PoE, speed).
        let physical_port = client
            .execute_command("show switch physical-port")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        // Port mirror (SPAN) sessions.
        let switch_mirror = client
            .execute_command("show switch mirror")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        // SNMP communities, trap receivers, and enabled trap events.
        let snmp_community = client
            .execute_command("show system snmp community")
            .await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(FortiSwitchConfigBlocks {
            system_interface,
            switch_vlan,
            switch_interface,
            physical_port,
            switch_mirror,
            snmp_community,
        })
    }

    /// Detect hardware model by running "get system status" and comparing
    /// the version string against known product identifiers.
    async fn detect_hardware_model(&mut self) -> Vec<String> {
        let client = match self.client.as_mut() {
            Some(c) => c,
            None => return Vec::new(),
        };

        match client.execute_command("get system status").await {
            Ok(output) => {
                // The output contains a line like:
                // "Version: FortiSwitch-108F-POE v7.2.8,build0660,241119 (GA.MR8)"
                // Extract the model from this line
                let pattern = regex::Regex::new(
                    r"Version:\s*(FortiSwitch-\S+)\s+v"
                ).unwrap();
                super::traits::verify_hardware_model(
                    &output,
                    &self.config.model(),
                    &pattern,
                )
            }
            Err(e) => {
                debug!("Could not get system status for model detection: {}", e);
                Vec::new()
            }
        }
    }

    /// Extract the VLAN id from a `config system interface` edit line,
    /// accepting both `edit vlan77` and `edit "vlan77"` (FortiOS quotes
    /// interface names in some config/show contexts). Duplicates the
    /// equivalent logic already inlined in `parse_management_vlan` rather
    /// than factoring it out — that function is delicate and already has
    /// extensive test coverage; a shared helper isn't worth the risk of
    /// touching it.
    fn parse_vlan_interface_edit_id(trimmed: &str) -> Option<u16> {
        if let Some(rest) = trimmed.strip_prefix("edit vlan") {
            return rest.split_whitespace().next()?.parse().ok();
        }
        if let Some(rest) = trimmed.strip_prefix("edit ") {
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix("\"vlan").and_then(|s| s.strip_suffix('"')) {
                return inner.parse().ok();
            }
        }
        None
    }

    /// Parse every VLAN Layer-3 interface declared under `config system
    /// interface`, returning ip_config/description per VLAN id — the same
    /// block `parse_management_vlan` walks, but collecting details for every
    /// VLAN interface found, not just the one that looks like a management
    /// VLAN.
    fn parse_svi_details(&self, lines: &[&str]) -> std::collections::HashMap<u16, SviDetails> {
        let mut result = std::collections::HashMap::new();
        let mut in_system_interface = false;
        let mut nesting_depth = 0;
        let mut current_vlan_id: Option<u16> = None;
        let mut current = SviDetails::default();

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config system interface" {
                in_system_interface = true;
                nesting_depth = 1;
                continue;
            }

            if !in_system_interface {
                continue;
            }

            if trimmed.starts_with("config ") {
                nesting_depth += 1;
                continue;
            }

            if trimmed == "end" {
                nesting_depth -= 1;
                if nesting_depth == 0 {
                    if let Some(id) = current_vlan_id.take() {
                        result.insert(id, current.clone());
                    }
                    in_system_interface = false;
                }
                continue;
            }

            if nesting_depth != 1 {
                continue;
            }

            if let Some(vlan_id) = Self::parse_vlan_interface_edit_id(trimmed) {
                if let Some(prev_id) = current_vlan_id.take() {
                    result.insert(prev_id, current.clone());
                }
                current_vlan_id = Some(vlan_id);
                current = SviDetails::default();
                continue;
            }

            if current_vlan_id.is_none() {
                continue;
            }

            if let Some(desc) = trimmed.strip_prefix("set description ") {
                current.description = Some(desc.trim_matches('"').to_string());
            } else if let Some(rest) = trimmed.strip_prefix("set ip ") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() == 2 {
                    current.ip_config = VlanIpConfig::Static {
                        address: parts[0].to_string(),
                        netmask: parts[1].to_string(),
                    };
                }
            } else if trimmed == "set mode dhcp" {
                current.ip_config = VlanIpConfig::Dhcp;
            } else if trimmed == "next" {
                if let Some(id) = current_vlan_id.take() {
                    result.insert(id, current.clone());
                }
                current = SviDetails::default();
            }
        }

        if let Some(id) = current_vlan_id.take() {
            result.insert(id, current);
        }

        result
    }

    /// Parse the VLAN database (`config switch vlan`) and return every VLAN
    /// id declared there. This is the only place FortiSwitch persists a
    /// VLAN's existence when it has no Layer-3 interface — VLAN names are
    /// never written to the device at all (see `generate_vlan_commands`), so
    /// a VLAN's name always comes from the desired config, never from here.
    fn parse_switch_vlan_ids(&self, lines: &[&str]) -> Vec<u16> {
        let mut ids = Vec::new();
        let mut in_block = false;
        let mut depth = 0;

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config switch vlan" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                continue;
            }

            if depth == 1 {
                if let Some(rest) = trimmed.strip_prefix("edit ") {
                    if let Ok(id) = rest.trim().trim_matches('"').parse::<u16>() {
                        ids.push(id);
                    }
                }
            }
        }

        ids
    }

    /// Parse per-port VLAN membership from `config switch interface`
    /// (description, native VLAN, allowed VLANs). `set untagged-vlans` is
    /// intentionally not parsed — `generate_port_commands` always sets it to
    /// match the native VLAN, so it carries no information `native-vlan`
    /// doesn't already have.
    fn parse_switch_interface_ports(&self, lines: &[&str]) -> std::collections::HashMap<String, PortInterfaceInfo> {
        let mut result = std::collections::HashMap::new();
        let mut in_block = false;
        let mut depth = 0;
        let mut current_port: Option<String> = None;
        let mut current = PortInterfaceInfo::default();

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config switch interface" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if depth == 0 {
                    if let Some(port_id) = current_port.take() {
                        result.insert(port_id, current.clone());
                    }
                    in_block = false;
                }
                continue;
            }

            if depth != 1 {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("edit ") {
                if let Some(prev) = current_port.take() {
                    result.insert(prev, current.clone());
                }
                current_port = Some(rest.trim().trim_matches('"').to_string());
                current = PortInterfaceInfo::default();
                continue;
            }

            if current_port.is_none() {
                continue;
            }

            if let Some(desc) = trimmed.strip_prefix("set description ") {
                current.description = Some(desc.trim_matches('"').to_string());
            } else if let Some(rest) = trimmed.strip_prefix("set native-vlan ") {
                current.native_vlan = rest.trim().parse().ok();
            } else if let Some(rest) = trimmed.strip_prefix("set allowed-vlans ") {
                current.allowed_vlans = Self::parse_vlan_list(rest);
            } else if trimmed == "next" {
                if let Some(port_id) = current_port.take() {
                    result.insert(port_id, current.clone());
                }
                current = PortInterfaceInfo::default();
            }
        }

        if let Some(port_id) = current_port.take() {
            result.insert(port_id, current);
        }

        result
    }

    /// Parse per-port physical properties from `config switch physical-port`
    /// (link status, PoE, speed/duplex).
    fn parse_physical_ports(&self, lines: &[&str]) -> std::collections::HashMap<String, PhysicalPortInfo> {
        let mut result = std::collections::HashMap::new();
        let mut in_block = false;
        let mut depth = 0;
        let mut current_port: Option<String> = None;
        let mut current = PhysicalPortInfo::default();

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config switch physical-port" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if depth == 0 {
                    if let Some(port_id) = current_port.take() {
                        result.insert(port_id, current.clone());
                    }
                    in_block = false;
                }
                continue;
            }

            if depth != 1 {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("edit ") {
                if let Some(prev) = current_port.take() {
                    result.insert(prev, current.clone());
                }
                current_port = Some(rest.trim().trim_matches('"').to_string());
                current = PhysicalPortInfo::default();
                continue;
            }

            if current_port.is_none() {
                continue;
            }

            if trimmed == "set status up" {
                current.enabled = Some(true);
            } else if trimmed == "set status down" {
                current.enabled = Some(false);
            } else if trimmed == "set poe-status enable" {
                current.poe_enabled = Some(true);
            } else if trimmed == "set poe-status disable" {
                current.poe_enabled = Some(false);
            } else if let Some(rest) = trimmed.strip_prefix("set speed ") {
                current.speed_duplex = Some(Self::parse_speed_from_fortiswitch(rest.trim()));
            } else if trimmed == "next" {
                if let Some(port_id) = current_port.take() {
                    result.insert(port_id, current.clone());
                }
                current = PhysicalPortInfo::default();
            }
        }

        if let Some(port_id) = current_port.take() {
            result.insert(port_id, current);
        }

        result
    }

    /// Merge per-port VLAN membership and physical properties into the
    /// `Port` list the diff engine expects. A port present in only one of
    /// the two blocks (shouldn't normally happen — both are written
    /// together by `generate_port_commands`) falls back to that struct's
    /// defaults for the missing half.
    ///
    /// `PhysicalPortInfo`'s fields are `Option` because FortiOS's `show`
    /// only prints settings that differ from this switch's own default —
    /// confirmed on real hardware, where `set poe-status`/`set status` never
    /// appeared at all for ports already at the (PoE-enabled, up) default.
    /// A bare `unwrap_or(false)` for `poe_enabled` would misread every such
    /// port as PoE-disabled forever, so the fallback is model-aware: `true`
    /// on a port the model itself says is PoE-capable, `false` otherwise.
    fn build_ports(
        &self,
        interfaces: std::collections::HashMap<String, PortInterfaceInfo>,
        physical: std::collections::HashMap<String, PhysicalPortInfo>,
    ) -> Vec<Port> {
        let mut port_ids: std::collections::BTreeSet<String> = interfaces.keys().cloned().collect();
        port_ids.extend(physical.keys().cloned());

        port_ids
            .into_iter()
            // `show switch interface` lists FortiSwitch's own special
            // interfaces (e.g. "internal", its CPU/management port)
            // alongside the real numbered front-panel ports — confirmed on
            // real hardware. Those aren't configurable ports at all; with
            // enforce_port_config on, treating "internal" as a stray port
            // made the diff engine want to "reset" it every single cycle.
            .filter(|interface_id| Self::is_numbered_port(interface_id))
            .map(|interface_id| {
                let iface = interfaces.get(&interface_id).cloned().unwrap_or_default();
                let phys = physical.get(&interface_id).cloned().unwrap_or_default();
                let port_id = Self::denormalize_port_id(&interface_id);

                let native = iface.native_vlan.unwrap_or(1);
                let tagged: Vec<u16> = iface
                    .allowed_vlans
                    .iter()
                    .copied()
                    .filter(|&v| v != native)
                    .collect();

                let poe_capable = self.config.model().port_supports_poe(&port_id);

                Port {
                    port_id,
                    mode: if tagged.is_empty() { PortMode::Access } else { PortMode::Trunk },
                    vlan: native,
                    tagged_vlans: tagged,
                    description: iface.description,
                    enabled: phys.enabled.unwrap_or(true),
                    poe_enabled: phys.poe_enabled.unwrap_or(poe_capable),
                    mac_notify: false,
                    speed_duplex: phys.speed_duplex.unwrap_or(crate::models::SpeedDuplex::Auto),
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                }
            })
            .collect()
    }

    /// Parse port mirror (SPAN) sessions from `config switch mirror`.
    fn parse_switch_mirrors(&self, lines: &[&str]) -> Vec<PortMirror> {
        let mut mirrors = Vec::new();
        let mut in_block = false;
        let mut depth = 0;
        let mut session_id: Option<String> = None;
        let mut dst: Option<String> = None;
        let mut ingress: Vec<String> = Vec::new();
        let mut egress: Vec<String> = Vec::new();

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config switch mirror" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if depth == 0 {
                    Self::flush_mirror(&mut session_id, &mut dst, &mut ingress, &mut egress, &mut mirrors);
                    in_block = false;
                }
                continue;
            }

            if depth != 1 {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("edit ") {
                Self::flush_mirror(&mut session_id, &mut dst, &mut ingress, &mut egress, &mut mirrors);
                session_id = Some(rest.trim().trim_matches('"').to_string());
                continue;
            }

            if session_id.is_none() {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("set dst ") {
                dst = Some(rest.trim().to_string());
            } else if let Some(rest) = trimmed.strip_prefix("set src-ingress ") {
                ingress = rest.split_whitespace().map(|s| s.to_string()).collect();
            } else if let Some(rest) = trimmed.strip_prefix("set src-egress ") {
                egress = rest.split_whitespace().map(|s| s.to_string()).collect();
            } else if trimmed == "next" {
                Self::flush_mirror(&mut session_id, &mut dst, &mut ingress, &mut egress, &mut mirrors);
            }
        }

        Self::flush_mirror(&mut session_id, &mut dst, &mut ingress, &mut egress, &mut mirrors);

        mirrors
    }

    /// Emit one `PortMirror` from an in-progress mirror-session edit block,
    /// if it has both a destination and at least one source direction, then
    /// reset the per-session accumulators for the next `edit`.
    fn flush_mirror(
        session_id: &mut Option<String>,
        dst: &mut Option<String>,
        ingress: &mut Vec<String>,
        egress: &mut Vec<String>,
        mirrors: &mut Vec<PortMirror>,
    ) {
        if let (Some(id), Some(d)) = (session_id.take(), dst.take()) {
            if !ingress.is_empty() || !egress.is_empty() {
                let direction = match (!ingress.is_empty(), !egress.is_empty()) {
                    (true, true) => MirrorDirection::Both,
                    (true, false) => MirrorDirection::Rx,
                    (false, true) => MirrorDirection::Tx,
                    (false, false) => MirrorDirection::Both,
                };
                let source_ports = if !ingress.is_empty() { ingress.clone() } else { egress.clone() };
                mirrors.push(PortMirror {
                    session_id: id,
                    source_ports: source_ports.iter().map(|p| Self::denormalize_port_id(p)).collect(),
                    destination_port: Self::denormalize_port_id(&d),
                    direction,
                });
            }
        }
        ingress.clear();
        egress.clear();
    }

    /// Parse SNMP communities, trap receivers, and enabled trap events from
    /// `config system snmp community`.
    fn parse_snmp_community(&self, lines: &[&str]) -> Option<SnmpConfig> {
        let mut communities = Vec::new();
        let mut trap_receivers = Vec::new();
        let mut enabled_traps: Vec<TrapType> = Vec::new();

        let mut in_block = false;
        let mut depth = 0;
        let mut in_community = false;
        let mut community_name: Option<String> = None;
        let mut in_hosts = false;
        let mut current_host_ip: Option<String> = None;

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config system snmp community" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed == "config hosts" {
                in_hosts = true;
                depth += 1;
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if in_hosts && depth == 1 {
                    in_hosts = false;
                    continue;
                }
                if depth == 0 {
                    in_block = false;
                }
                continue;
            }

            if in_hosts {
                if let Some(rest) = trimmed.strip_prefix("set ip ") {
                    current_host_ip = Some(rest.trim().to_string());
                } else if trimmed == "next" {
                    if let (Some(ip), Some(name)) = (current_host_ip.take(), community_name.clone()) {
                        trap_receivers.push(SnmpTrapReceiver {
                            host: ip,
                            community: name,
                            version: None,
                        });
                    }
                }
                continue;
            }

            if depth != 1 {
                continue;
            }

            if trimmed.starts_with("edit ") {
                in_community = true;
                community_name = None;
                continue;
            }

            if !in_community {
                continue;
            }

            if let Some(name) = trimmed.strip_prefix("set name ") {
                community_name = Some(name.trim_matches('"').to_string());
            } else if let Some(rest) = trimmed.strip_prefix("set events ") {
                enabled_traps = rest
                    .split_whitespace()
                    .filter_map(Self::parse_trap_type_from_fortiswitch)
                    .collect();
            } else if trimmed == "next" {
                if let Some(name) = community_name.take() {
                    communities.push(SnmpCommunity { name, access: SnmpAccess::default() });
                }
                in_community = false;
            }
        }

        if communities.is_empty() && trap_receivers.is_empty() {
            return None;
        }

        Some(SnmpConfig { communities, trap_receivers, enabled_traps })
    }

    /// Map each SNMP community's name to its numeric `edit <N>` index in
    /// `config system snmp community`. A sibling of `parse_snmp_community`
    /// (same block walk, duplicated rather than factored in) so removal
    /// commands can `delete <index>` — FortiOS's own index, not the name —
    /// without touching that already real-hardware-verified parser.
    fn parse_snmp_community_indices(lines: &[&str]) -> std::collections::HashMap<String, u16> {
        let mut indices = std::collections::HashMap::new();
        let mut in_block = false;
        let mut depth = 0;
        let mut in_hosts = false;
        let mut current_index: Option<u16> = None;

        for line in lines {
            let trimmed = line.trim();

            if trimmed == "config system snmp community" {
                in_block = true;
                depth = 1;
                continue;
            }

            if !in_block {
                continue;
            }

            if trimmed == "config hosts" {
                in_hosts = true;
                depth += 1;
                continue;
            }

            if trimmed.starts_with("config ") {
                depth += 1;
                continue;
            }

            if trimmed == "end" {
                depth -= 1;
                if in_hosts && depth == 1 {
                    in_hosts = false;
                }
                if depth == 0 {
                    in_block = false;
                }
                continue;
            }

            if in_hosts || depth != 1 {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("edit ") {
                current_index = rest.trim().parse().ok();
                continue;
            }

            if let Some(name) = trimmed.strip_prefix("set name ") {
                if let Some(idx) = current_index {
                    indices.insert(name.trim_matches('"').to_string(), idx);
                }
            } else if trimmed == "next" {
                current_index = None;
            }
        }

        indices
    }

    /// Reverse of `convert_trap_type_to_fortiswitch`.
    fn parse_trap_type_from_fortiswitch(s: &str) -> Option<TrapType> {
        match s {
            "mac-notify" => Some(TrapType::MacNotify),
            "link-up-down" => Some(TrapType::LinkChange),
            "all" => Some(TrapType::All),
            _ => None,
        }
    }

    /// Reverse of `convert_speed_to_fortiswitch`.
    fn parse_speed_from_fortiswitch(s: &str) -> SpeedDuplex {
        match s {
            "10half" => SpeedDuplex::TenHalf,
            "10full" => SpeedDuplex::TenFull,
            "100half" => SpeedDuplex::HundredHalf,
            "100full" => SpeedDuplex::HundredFull,
            "1000full" => SpeedDuplex::ThousandFull,
            "10000full" => SpeedDuplex::TenGFull,
            _ => SpeedDuplex::Auto,
        }
    }

    /// Reverse of `normalize_port_id`: `port1` -> `1`. Anything without the
    /// `port` prefix is returned unchanged.
    fn denormalize_port_id(interface: &str) -> String {
        interface.strip_prefix("port").unwrap_or(interface).to_string()
    }

    /// Whether an interface name from `show switch interface`/`show switch
    /// physical-port` is a real numbered front-panel port (`port1`,
    /// `port24`, ...) rather than one of FortiSwitch's own special
    /// interfaces (e.g. `internal`, its CPU/management port — confirmed
    /// present in that same listing on real hardware).
    fn is_numbered_port(interface: &str) -> bool {
        interface.strip_prefix("port")
            .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    }

    /// Parse a FortiOS VLAN list, expanding `a-b` ranges. Confirmed on real
    /// hardware (FortiSwitch 124F-FPOE): `show switch interface` compresses
    /// `allowed-vlans` into comma-separated ranges, e.g. `42,101-104,501` —
    /// a bare whitespace split parses that as one invalid token and silently
    /// drops the entire list. The write side (`generate_port_commands`)
    /// instead space-separates individual ids; both forms are accepted here.
    fn parse_vlan_list(s: &str) -> Vec<u16> {
        s.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .flat_map(|part| {
                if let Some((start, end)) = part.split_once('-') {
                    if let (Ok(start), Ok(end)) = (start.trim().parse::<u16>(), end.trim().parse::<u16>()) {
                        return (start..=end).collect::<Vec<u16>>();
                    }
                }
                part.trim().parse::<u16>().ok().into_iter().collect()
            })
            .collect()
    }

    /// Reset ports to default state (disabled, VLAN 1, access mode, no description)
    async fn reset_ports(&mut self, port_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let mut commands = vec!["config switch interface".to_string()];

        for port_id in port_ids {
            let interface = self.normalize_port_id(port_id);
            debug!("  Resetting port {} to default state", port_id);

            commands.push(format!("edit {}", interface));
            commands.push("unset description".to_string());  // Remove description
            commands.push("set native-vlan 1".to_string());  // Set to default VLAN
            commands.push("set allowed-vlans 1".to_string());  // Only allow default VLAN
            commands.push("set default-cos 0".to_string());  // Reset CoS
            commands.push("set poe-status enable".to_string());  // Enable PoE
            commands.push("set admin down".to_string());  // Disable the port
            commands.push("next".to_string());
        }

        commands.push("end".to_string());

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

    /// Configure mirror destination ports with baseline settings (VLAN 1, enabled).
    async fn configure_mirror_dest_ports(&mut self, port_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let mut commands = vec!["config switch physical-port".to_string()];

        for port_id in port_ids {
            let interface = self.normalize_port_id(port_id);
            debug!("  Configuring mirror dest port {} with baseline settings", port_id);

            commands.push(format!("edit {}", interface));
            commands.push("set status up".to_string());
            commands.push("next".to_string());
        }

        commands.push("end".to_string());

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

    /// Configure management VLAN on FortiSwitch
    /// This creates a VLAN interface with allowaccess for management services
    async fn configure_management_vlan(&mut self, vlan_id: u16) -> Result<ConfigResult, VendorError> {
        info!("Configuring FortiSwitch management VLAN: {}", vlan_id);

        // Find the VLAN configuration to get IP settings
        let vlan_config = self.config.vlans.iter()
            .find(|v| v.id == vlan_id)
            .ok_or_else(|| VendorError::ValidationError(
                format!("Management VLAN {} not found in VLAN configuration", vlan_id)
            ))?;

        let mut commands = vec![
            "config system interface".to_string(),
            format!("edit vlan{}", vlan_id),
            format!("set vlanid {}", vlan_id),
            format!("set description \"Management VLAN with allowaccess\""),
            "set type vlan".to_string(),
            "set interface internal".to_string(),
        ];

        // Configure IP address based on VLAN IP configuration
        match &vlan_config.ip_config {
            VlanIpConfig::Static { address, netmask } => {
                commands.push(format!("set ip {} {}", address, netmask));
                info!("  Configured static IP: {} {}", address, netmask);
            }
            VlanIpConfig::Dhcp => {
                commands.push("set mode dhcp".to_string());
                info!("  Configured DHCP for management VLAN");
            }
            VlanIpConfig::None => {
                warn!("  Management VLAN {} has no IP configuration - switch may not be reachable", vlan_id);
            }
        }

        // Set allowaccess for management services (ping, https, ssh, snmp)
        commands.push("set allowaccess ping https ssh snmp".to_string());

        commands.push("next".to_string());
        commands.push("end".to_string());

        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client.execute_commands(&commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured management VLAN {} with allowaccess", vlan_id),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Remove management VLAN configuration on FortiSwitch
    /// This removes the VLAN interface
    async fn remove_management_vlan(&mut self) -> Result<ConfigResult, VendorError> {
        info!("Removing FortiSwitch management VLAN configuration");

        // Since we don't know which VLAN was the management VLAN without parsing state,
        // we can't remove a specific interface. For now, return a warning.
        // A proper implementation would parse the current state first.

        warn!("FortiSwitch management VLAN removal requires knowing which VLAN to remove");
        warn!("This operation should be implemented after state parsing is complete");

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: "Management VLAN removal not fully implemented for FortiSwitch".to_string(),
            commands_executed: vec![],
            timestamp: chrono::Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use crate::models::{
        SnmpAccess, SnmpCommunity, SnmpConfig, SpeedDuplex, SnmpTrapReceiver, TrapType,
        VlanIpConfig,
    };

    fn create_test_config() -> SwitchConfig {
        use crate::models::{ConnectionType, Credentials, SwitchModel};

        SwitchConfig {
            id: "test-fortiswitch".to_string(),
            hostname: Some("fortiswitch-test".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.168.1.100".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("adminadmin".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Serial,
                serial_device: Some("/dev/ttyUSB0".to_string()),
                baud_rate: 115200,
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
        }
    }

    fn create_test_switch() -> FortiswitchSwitch {
        FortiswitchSwitch::new(create_test_config(), RuntimeConfig::default(), false)
    }

    // ========== Port ID Normalization Tests ==========

    #[test]
    fn test_normalize_port_id_simple() {
        let switch = create_test_switch();
        assert_eq!(switch.normalize_port_id("1"), "port1");
        assert_eq!(switch.normalize_port_id("24"), "port24");
    }

    #[test]
    fn test_normalize_port_id_already_normalized() {
        let switch = create_test_switch();
        assert_eq!(switch.normalize_port_id("port1"), "port1");
        assert_eq!(switch.normalize_port_id("port24"), "port24");
    }

    #[test]
    fn test_normalize_port_id_cisco_format() {
        let switch = create_test_switch();
        assert_eq!(switch.normalize_port_id("GigabitEthernet1/0/1"), "port1");
        assert_eq!(switch.normalize_port_id("1/0/24"), "port24");
    }

    // ========== Speed Conversion Tests ==========

    #[test]
    fn test_convert_speed_to_fortiswitch() {
        let switch = create_test_switch();
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::Auto),
            "auto"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::TenHalf),
            "10half"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::TenFull),
            "10full"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::HundredHalf),
            "100half"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::HundredFull),
            "100full"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::ThousandFull),
            "1000full"
        );
        assert_eq!(
            switch.convert_speed_to_fortiswitch(&SpeedDuplex::TenGFull),
            "10000full"
        );
    }

    // ========== Trap Type Conversion Tests ==========

    #[test]
    fn test_convert_trap_type_to_fortiswitch() {
        let switch = create_test_switch();
        assert_eq!(
            switch.convert_trap_type_to_fortiswitch(&TrapType::MacNotify),
            "mac-notify"
        );
        assert_eq!(
            switch.convert_trap_type_to_fortiswitch(&TrapType::LinkChange),
            "link-up-down"
        );
        assert_eq!(
            switch.convert_trap_type_to_fortiswitch(&TrapType::All),
            "all"
        );
    }

    // ========== VLAN Command Generation Tests ==========

    #[test]
    fn test_generate_vlan_commands_no_ip() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 1,
            name: "default".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        // Should only have Layer 2 VLAN creation (no SVI)
        assert_eq!(commands.len(), 4);
        assert_eq!(commands[0], "config switch vlan");
        assert_eq!(commands[1], "edit 1");
        assert_eq!(commands[2], "next");
        assert_eq!(commands[3], "end");
    }

    #[test]
    fn test_generate_vlan_commands_with_dhcp() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 10,
            name: "management".to_string(),
            description: Some("Management VLAN".to_string()),
            ip_config: VlanIpConfig::Dhcp,
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        // Should have Layer 2 VLAN + Layer 3 SVI with DHCP
        assert!(commands.contains(&"config switch vlan".to_string()));
        assert!(commands.contains(&"edit 10".to_string()));
        assert!(commands.contains(&"config system interface".to_string()));
        assert!(commands.contains(&"edit vlan10".to_string()));
        assert!(commands.contains(&"set vlanid 10".to_string()));
        assert!(commands.contains(&"set description \"Management VLAN\"".to_string()));
        assert!(commands.contains(&"set type vlan".to_string()));
        assert!(commands.contains(&"set interface internal".to_string()));
        assert!(commands.contains(&"set mode dhcp".to_string()));
        assert!(commands.contains(&"set allowaccess ping".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_with_static_ip() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 20,
            name: "users".to_string(),
            description: None,
            ip_config: VlanIpConfig::Static {
                address: "10.0.20.1".to_string(),
                netmask: "255.255.255.0".to_string(),
            },
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        // Should have Layer 2 VLAN + Layer 3 SVI with static IP
        assert!(commands.contains(&"config switch vlan".to_string()));
        assert!(commands.contains(&"edit 20".to_string()));
        assert!(commands.contains(&"config system interface".to_string()));
        assert!(commands.contains(&"edit vlan20".to_string()));
        assert!(commands.contains(&"set vlanid 20".to_string()));
        assert!(commands.contains(&"set type vlan".to_string()));
        assert!(commands.contains(&"set interface internal".to_string()));
        assert!(commands.contains(&"set ip 10.0.20.1 255.255.255.0".to_string()));
        assert!(commands.contains(&"set allowaccess ping".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_multiple_vlans() {
        let switch = create_test_switch();
        let vlans = vec![
            Vlan {
                id: 1,
                name: "default".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 10,
                name: "management".to_string(),
                description: Some("Mgmt".to_string()),
                ip_config: VlanIpConfig::Dhcp,
            },
            Vlan {
                id: 20,
                name: "users".to_string(),
                description: None,
                ip_config: VlanIpConfig::Static {
                    address: "10.0.20.1".to_string(),
                    netmask: "255.255.255.0".to_string(),
                },
            },
        ];

        let commands = switch.generate_vlan_commands(&vlans);

        // Verify Layer 2 VLANs created for all
        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"edit 10".to_string()));
        assert!(commands.contains(&"edit 20".to_string()));

        // Verify SVIs created only for VLANs with IP config
        assert!(commands.contains(&"edit vlan10".to_string()));
        assert!(commands.contains(&"edit vlan20".to_string()));
        assert!(!commands.contains(&"edit vlan1".to_string())); // No SVI for VLAN 1 (no IP)
    }

    // ========== Port Command Generation Tests ==========

    #[test]
    fn test_generate_port_commands_access_mode() {
        let switch = create_test_switch();
        let ports = vec![Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            tagged_vlans: vec![],
            description: Some("Test Port".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let commands = switch.generate_port_commands(&ports);

        // Phase 1: Switch interface (VLAN config)
        assert!(commands.contains(&"config switch interface".to_string()));
        assert!(commands.contains(&"edit port1".to_string()));
        assert!(commands.contains(&"set description \"Test Port\"".to_string()));
        assert!(commands.contains(&"set native-vlan 10".to_string()));
        assert!(commands.contains(&"set allowed-vlans 10".to_string()));
        assert!(commands.contains(&"set untagged-vlans 10".to_string()));

        // Phase 2: Physical port (status, PoE, speed)
        assert!(commands.contains(&"config switch physical-port".to_string()));
        assert!(commands.contains(&"set status up".to_string()));
        assert!(commands.contains(&"set poe-status disable".to_string()));
        assert!(commands.contains(&"set speed auto".to_string()));
    }

    #[test]
    fn test_generate_port_commands_trunk_mode() {
        let switch = create_test_switch();
        let ports = vec![Port {
            port_id: "24".to_string(),
            mode: PortMode::Trunk,
            vlan: 1,
            tagged_vlans: vec![1, 10, 20, 30],
            description: Some("Uplink".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::ThousandFull,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let commands = switch.generate_port_commands(&ports);

        // Phase 1: Switch interface (VLAN config)
        assert!(commands.contains(&"config switch interface".to_string()));
        assert!(commands.contains(&"edit port24".to_string()));
        assert!(commands.contains(&"set description \"Uplink\"".to_string()));
        assert!(commands.contains(&"set native-vlan 1".to_string()));
        assert!(commands.contains(&"set allowed-vlans 1 10 20 30".to_string()));
        assert!(commands.contains(&"set untagged-vlans 1".to_string()));

        // Phase 2: Physical port
        assert!(commands.contains(&"config switch physical-port".to_string()));
        assert!(commands.contains(&"set status up".to_string()));
        assert!(commands.contains(&"set poe-status disable".to_string()));
        assert!(commands.contains(&"set speed 1000full".to_string()));
    }

    #[test]
    fn test_generate_port_commands_disabled_port() {
        let switch = create_test_switch();
        let ports = vec![Port {
            port_id: "8".to_string(),
            mode: PortMode::Access,
            vlan: 1,
            tagged_vlans: vec![],
            description: Some("Disabled Port".to_string()),
            enabled: false,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let commands = switch.generate_port_commands(&ports);

        assert!(commands.contains(&"set status down".to_string()));
    }

    #[test]
    fn test_generate_port_commands_poe_enabled() {
        let switch = create_test_switch();
        let ports = vec![Port {
            port_id: "2".to_string(),
            mode: PortMode::Access,
            vlan: 20,
            tagged_vlans: vec![],
            description: Some("PoE Port".to_string()),
            enabled: true,
            poe_enabled: true,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let commands = switch.generate_port_commands(&ports);

        assert!(commands.contains(&"set poe-status enable".to_string()));
    }

    #[test]
    fn test_generate_port_commands_speed_variants() {
        let switch = create_test_switch();
        let ports = vec![
            Port {
                port_id: "1".to_string(),
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
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "3".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                tagged_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::ThousandFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        let commands = switch.generate_port_commands(&ports);

        // Verify all three speed settings appear
        assert!(commands.contains(&"set speed auto".to_string()));
        assert!(commands.contains(&"set speed 100full".to_string()));
        assert!(commands.contains(&"set speed 1000full".to_string()));
    }

    // ========== Port Mirror Command Generation Tests ==========

    #[test]
    fn test_generate_mirror_commands_rx_direction() {
        let switch = create_test_switch();
        let mirrors = vec![PortMirror {
            session_id: "1".to_string(),
            source_ports: vec!["1".to_string(), "2".to_string()],
            destination_port: "10".to_string(),
            direction: MirrorDirection::Rx,
        }];

        let commands = switch.generate_mirror_commands(&mirrors);

        assert_eq!(commands[0], "config switch mirror");
        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"set status active".to_string()));
        assert!(commands.contains(&"set dst port10".to_string()));
        assert!(commands.contains(&"set src-ingress port1 port2".to_string()));
        assert!(!commands.iter().any(|c| c.contains("src-egress")));
        assert_eq!(commands.last().unwrap(), "end");
    }

    #[test]
    fn test_generate_mirror_commands_tx_direction() {
        let switch = create_test_switch();
        let mirrors = vec![PortMirror {
            session_id: "1".to_string(),
            source_ports: vec!["1".to_string()],
            destination_port: "10".to_string(),
            direction: MirrorDirection::Tx,
        }];

        let commands = switch.generate_mirror_commands(&mirrors);

        assert!(commands.contains(&"set src-egress port1".to_string()));
        assert!(!commands.iter().any(|c| c.contains("src-ingress")));
    }

    #[test]
    fn test_generate_mirror_commands_both_direction() {
        let switch = create_test_switch();
        let mirrors = vec![PortMirror {
            session_id: "1".to_string(),
            source_ports: vec!["1".to_string()],
            destination_port: "10".to_string(),
            direction: MirrorDirection::Both,
        }];

        let commands = switch.generate_mirror_commands(&mirrors);

        // Both directions should be configured
        assert!(commands.contains(&"set src-ingress port1".to_string()));
        assert!(commands.contains(&"set src-egress port1".to_string()));
    }

    #[test]
    fn test_generate_mirror_commands_multiple_sessions() {
        let switch = create_test_switch();
        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Both,
            },
            PortMirror {
                session_id: "2".to_string(),
                source_ports: vec!["5".to_string(), "6".to_string()],
                destination_port: "11".to_string(),
                direction: MirrorDirection::Rx,
            },
        ];

        let commands = switch.generate_mirror_commands(&mirrors);

        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"edit 2".to_string()));
        assert!(commands.contains(&"set dst port10".to_string()));
        assert!(commands.contains(&"set dst port11".to_string()));
    }

    // ========== SNMP Command Generation Tests ==========

    #[test]
    fn test_generate_snmp_commands_single_community_no_traps() {
        let switch = create_test_switch();
        let snmp_config = SnmpConfig {
            communities: vec![SnmpCommunity {
                name: "public".to_string(),
                access: SnmpAccess::Operator,
            }],
            trap_receivers: vec![],
            enabled_traps: vec![],
        };

        let commands = switch.generate_snmp_commands(&snmp_config);

        assert_eq!(commands[0], "config system snmp community");
        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"set name \"public\"".to_string()));
        assert!(commands.contains(&"set status enable".to_string()));
        assert!(commands.contains(&"set query-v1-status enable".to_string()));
        assert!(commands.contains(&"set query-v2c-status enable".to_string()));
        assert_eq!(commands.last().unwrap(), "end");
    }

    #[test]
    fn test_generate_snmp_commands_with_trap_receiver() {
        let switch = create_test_switch();
        let snmp_config = SnmpConfig {
            communities: vec![SnmpCommunity {
                name: "public".to_string(),
                access: SnmpAccess::Operator,
            }],
            trap_receivers: vec![SnmpTrapReceiver {
                host: "192.168.1.200".to_string(),
                community: "public".to_string(),
                version: Some("2c".to_string()),
            }],
            enabled_traps: vec![TrapType::MacNotify, TrapType::LinkChange],
        };

        let commands = switch.generate_snmp_commands(&snmp_config);

        // Verify trap receiver configuration
        assert!(commands.contains(&"set trap-v1-status enable".to_string()));
        assert!(commands.contains(&"set trap-v2c-status enable".to_string()));
        assert!(commands.contains(&"set events mac-notify link-up-down".to_string()));
        assert!(commands.contains(&"config hosts".to_string()));
        assert!(commands.contains(&"set ip 192.168.1.200".to_string()));
        assert!(commands.contains(&"set interface internal".to_string()));
    }

    #[test]
    fn test_generate_snmp_commands_multiple_communities() {
        let switch = create_test_switch();
        let snmp_config = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Operator,
                },
                SnmpCommunity {
                    name: "private".to_string(),
                    access: SnmpAccess::Unrestricted,
                },
            ],
            trap_receivers: vec![SnmpTrapReceiver {
                host: "192.168.1.200".to_string(),
                community: "public".to_string(),
                version: Some("2c".to_string()),
            }],
            enabled_traps: vec![],
        };

        let commands = switch.generate_snmp_commands(&snmp_config);

        // First community gets trap receivers
        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"set name \"public\"".to_string()));
        assert!(commands.contains(&"config hosts".to_string()));

        // Second community without trap receivers
        assert!(commands.contains(&"edit 2".to_string()));
        assert!(commands.contains(&"set name \"private\"".to_string()));
    }

    #[test]
    fn test_generate_snmp_commands_multiple_trap_receivers() {
        let switch = create_test_switch();
        let snmp_config = SnmpConfig {
            communities: vec![SnmpCommunity {
                name: "public".to_string(),
                access: SnmpAccess::Operator,
            }],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.200".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
                SnmpTrapReceiver {
                    host: "192.168.1.201".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
            ],
            enabled_traps: vec![],
        };

        let commands = switch.generate_snmp_commands(&snmp_config);

        // Verify both receivers are configured
        assert!(commands.contains(&"config hosts".to_string()));
        assert!(commands.contains(&"set ip 192.168.1.200".to_string()));
        assert!(commands.contains(&"set ip 192.168.1.201".to_string()));
    }

    #[test]
    fn test_generate_snmp_commands_all_trap_types() {
        let switch = create_test_switch();
        let snmp_config = SnmpConfig {
            communities: vec![SnmpCommunity {
                name: "public".to_string(),
                access: SnmpAccess::Operator,
            }],
            trap_receivers: vec![SnmpTrapReceiver {
                host: "192.168.1.200".to_string(),
                community: "public".to_string(),
                version: Some("2c".to_string()),
            }],
            enabled_traps: vec![TrapType::MacNotify, TrapType::LinkChange, TrapType::All],
        };

        let commands = switch.generate_snmp_commands(&snmp_config);

        // Verify all trap types are converted correctly
        assert!(commands.contains(&"set events mac-notify link-up-down all".to_string()));
    }

    // ========== Remove Commands Tests ==========

    #[test]
    fn test_generate_remove_vlan_commands() {
        let switch = create_test_switch();
        let vlan_ids = vec![10, 20, 30];

        let commands = switch.generate_remove_vlan_commands(&vlan_ids);

        assert_eq!(commands[0], "config switch vlan");
        assert!(commands.contains(&"delete 10".to_string()));
        assert!(commands.contains(&"delete 20".to_string()));
        assert!(commands.contains(&"delete 30".to_string()));
        assert_eq!(commands.last().unwrap(), "end");
    }

    #[test]
    fn test_generate_remove_mirror_commands() {
        let switch = create_test_switch();
        let session_ids = vec!["1".to_string(), "2".to_string()];

        let commands = switch.generate_remove_mirror_commands(&session_ids);

        assert_eq!(commands[0], "config switch mirror");
        assert!(commands.contains(&"delete 1".to_string()));
        assert!(commands.contains(&"delete 2".to_string()));
        assert_eq!(commands.last().unwrap(), "end");
    }

    // ========== Validation Tests ==========

    #[test]
    fn test_validate_configuration_valid() {
        let mut config = create_test_config();
        config.vlans = vec![Vlan {
            id: 10,
            name: "management".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        }];
        config.ports = vec![Port {
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
        }];

        let switch = FortiswitchSwitch::new(config, RuntimeConfig::default(), false);
        assert!(switch.validate_configuration().is_ok());
    }

    #[test]
    fn test_validate_configuration_invalid_vlan_id() {
        let mut config = create_test_config();
        config.vlans = vec![Vlan {
            id: 5000, // Invalid: > 4094
            name: "invalid".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        }];

        let switch = FortiswitchSwitch::new(config, RuntimeConfig::default(), false);
        let result = switch.validate_configuration();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VendorError::ValidationError(_)));
    }

    #[test]
    fn test_validate_configuration_invalid_port_vlan() {
        let mut config = create_test_config();
        config.ports = vec![Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 9999, // Invalid: > 4094
            tagged_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let switch = FortiswitchSwitch::new(config, RuntimeConfig::default(), false);
        let result = switch.validate_configuration();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VendorError::ValidationError(_)));
    }

    // ========== Integration Tests - Full Command Flow ==========

    #[test]
    fn test_full_vlan_port_mirror_snmp_commands() {
        let mut config = create_test_config();
        config.vlans = vec![
            Vlan {
                id: 1,
                name: "default".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 10,
                name: "management".to_string(),
                description: Some("Management VLAN".to_string()),
                ip_config: VlanIpConfig::Dhcp,
            },
        ];
        config.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Management Port".to_string()),
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
                tagged_vlans: vec![1, 10],
                description: Some("Uplink".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::ThousandFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];
        config.port_mirrors = vec![PortMirror {
            session_id: "1".to_string(),
            source_ports: vec!["1".to_string()],
            destination_port: "10".to_string(),
            direction: MirrorDirection::Both,
        }];
        config.snmp = Some(SnmpConfig {
            communities: vec![SnmpCommunity {
                name: "public".to_string(),
                access: SnmpAccess::Operator,
            }],
            trap_receivers: vec![SnmpTrapReceiver {
                host: "192.168.1.200".to_string(),
                community: "public".to_string(),
                version: Some("2c".to_string()),
            }],
            enabled_traps: vec![TrapType::MacNotify, TrapType::LinkChange],
        });

        let switch = FortiswitchSwitch::new(config.clone(), RuntimeConfig::default(), false);

        // Test VLAN commands
        let vlan_commands = switch.generate_vlan_commands(&config.vlans);
        assert!(vlan_commands.contains(&"config switch vlan".to_string()));
        assert!(vlan_commands.contains(&"edit 1".to_string()));
        assert!(vlan_commands.contains(&"edit 10".to_string()));
        assert!(vlan_commands.contains(&"config system interface".to_string()));
        assert!(vlan_commands.contains(&"edit vlan10".to_string()));

        // Test port commands
        let port_commands = switch.generate_port_commands(&config.ports);
        assert!(port_commands.contains(&"config switch interface".to_string()));
        assert!(port_commands.contains(&"edit port1".to_string()));
        assert!(port_commands.contains(&"edit port24".to_string()));
        assert!(port_commands.contains(&"config switch physical-port".to_string()));

        // Test mirror commands
        let mirror_commands = switch.generate_mirror_commands(&config.port_mirrors);
        assert!(mirror_commands.contains(&"config switch mirror".to_string()));
        assert!(mirror_commands.contains(&"edit 1".to_string()));

        // Test SNMP commands
        let snmp_commands = switch.generate_snmp_commands(config.snmp.as_ref().unwrap());
        assert!(snmp_commands.contains(&"config system snmp community".to_string()));
        assert!(snmp_commands.contains(&"set events mac-notify link-up-down".to_string()));
    }

    #[test]
    fn test_management_vlan_diff_add() {
        use crate::diff::compute_diff;

        let mut config = create_test_config();

        // Current state: no management VLAN
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        // Desired config: add management VLAN 77
        config.management_vlan = Some(77);
        config.vlans.push(Vlan {
            id: 77,
            name: "mgmt".to_string(),
            description: Some("Management VLAN".to_string()),
            ip_config: VlanIpConfig::Static {
                address: "192.168.77.1".to_string(),
                netmask: "255.255.255.0".to_string(),
            },
        });

        let diff = compute_diff(&current_state, &config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being added");
        assert_eq!(diff.management_vlan, Some(77), "Should show new management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_change() {
        use crate::diff::compute_diff;

        let mut config = create_test_config();

        // Current state: management VLAN 10
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(10),
            warnings: vec![],
        };

        // Desired config: change to management VLAN 88
        config.management_vlan = Some(88);
        config.vlans.push(Vlan {
            id: 88,
            name: "management".to_string(),
            description: None,
            ip_config: VlanIpConfig::Dhcp,
        });

        let diff = compute_diff(&current_state, &config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN change");
        assert_eq!(diff.management_vlan, Some(88), "Should show new management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_remove() {
        use crate::diff::compute_diff;

        let config = create_test_config();

        // Current state: management VLAN 60
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(60),
            warnings: vec![],
        };

        // Desired config: remove management VLAN (defaults to None)
        let diff = compute_diff(&current_state, &config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being removed");
        assert_eq!(diff.management_vlan, None, "Should show no management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_no_change() {
        use crate::diff::compute_diff;

        let mut config = create_test_config();

        // Current state: management VLAN 25
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(25),
            warnings: vec![],
        };

        // Desired config: same management VLAN 25
        config.management_vlan = Some(25);

        let diff = compute_diff(&current_state, &config, false);

        assert!(!diff.management_vlan_changed, "Should not detect change when management VLAN is same");
    }

    #[test]
    fn test_parse_management_vlan_static_ip() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit vlan77",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh snmp",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should parse VLAN interface with static IP as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_quoted_format() {
        let switch = create_test_switch();

        // FortiSwitch uses quoted interface names in show full-configuration
        let running_config = vec![
            "config system interface",
            "    edit \"vlan77\"",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh snmp",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should parse VLAN interface with quoted name");
    }

    #[test]
    fn test_parse_management_vlan_dhcp() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit vlan88",
            "        set mode dhcp",
            "        set allowaccess ping https ssh",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(88), "Should parse VLAN interface with DHCP as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_multiple_vlans() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit vlan10",
            "        set description \"Data VLAN\"",
            "    next",
            "    edit vlan77",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh snmp",
            "    next",
            "    edit vlan99",
            "        set ip 192.168.99.1 255.255.255.0",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should return first VLAN with IP and allowaccess");
    }

    #[test]
    fn test_parse_management_vlan_no_allowaccess() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit vlan77",
            "        set ip 192.168.77.1 255.255.255.0",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        // Parser requires BOTH IP and management-level allowaccess (SSH/HTTPS)
        // A VLAN with only IP but no SSH/HTTPS access is not a management VLAN
        assert_eq!(result, None, "Should return None when VLAN has IP but no SSH/HTTPS allowaccess");
    }

    #[test]
    fn test_parse_management_vlan_with_nested_config() {
        let switch = create_test_switch();

        // Test parsing when there are nested config blocks (like config secondaryip)
        let running_config = vec![
            "config system interface",
            "    edit \"internal\"",
            "        set mode dhcp",
            "        config secondaryip",  // Nested config block
            "            edit 1",
            "                set ip 192.168.1.99 255.255.255.0",
            "            next",
            "        end",  // This should NOT exit the system interface block
            "    next",
            "    edit \"vlan77\"",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh snmp",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should correctly parse management VLAN even with nested config blocks");
    }

    #[test]
    fn test_parse_management_vlan_only_ping_allowaccess() {
        let switch = create_test_switch();

        // VLAN with IP but only ping allowaccess should NOT be detected as management VLAN
        let running_config = vec![
            "config system interface",
            "    edit \"vlan10\"",
            "        set ip 10.0.10.1 255.255.255.0",
            "        set allowaccess ping",  // Only ping, no SSH/HTTPS
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, None, "Should return None for VLAN with only ping allowaccess");
    }

    #[test]
    fn test_parse_management_vlan_https_only() {
        let switch = create_test_switch();

        // VLAN with HTTPS allowaccess should be detected as management VLAN
        let running_config = vec![
            "config system interface",
            "    edit \"vlan99\"",
            "        set ip 192.168.99.1 255.255.255.0",
            "        set allowaccess ping https",  // Has HTTPS
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(99), "Should detect VLAN with HTTPS allowaccess as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_ssh_only() {
        let switch = create_test_switch();

        // VLAN with SSH allowaccess should be detected as management VLAN
        let running_config = vec![
            "config system interface",
            "    edit \"vlan100\"",
            "        set ip 10.0.100.1 255.255.255.0",
            "        set allowaccess ssh ping",  // Has SSH
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(100), "Should detect VLAN with SSH allowaccess as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_multiple_ping_only_vlans() {
        let switch = create_test_switch();

        // Multiple VLANs with IP + ping but no management access, then one with SSH
        let running_config = vec![
            "config system interface",
            "    edit \"vlan10\"",
            "        set ip 10.0.10.1 255.255.255.0",
            "        set allowaccess ping",
            "    next",
            "    edit \"vlan20\"",
            "        set ip 10.0.20.1 255.255.255.0",
            "        set allowaccess ping",
            "    next",
            "    edit \"vlan77\"",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh",  // First one with SSH/HTTPS
            "    next",
            "    edit \"vlan30\"",
            "        set ip 10.0.30.1 255.255.255.0",
            "        set allowaccess ping",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should return first VLAN with SSH/HTTPS, ignoring ping-only VLANs");
    }

    #[test]
    fn test_parse_management_vlan_deeply_nested_config() {
        let switch = create_test_switch();

        // Test with multiple levels of nested config blocks
        let running_config = vec![
            "config system interface",
            "    edit \"internal\"",
            "        set mode dhcp",
            "        config secondaryip",  // Nested level 1
            "            edit 1",
            "                set ip 192.168.1.99 255.255.255.0",
            "                config some-nested-block",  // Nested level 2
            "                    edit 1",
            "                        set something value",
            "                    next",
            "                end",  // Exit level 2
            "            next",
            "        end",  // Exit level 1
            "    next",
            "    edit \"vlan77\"",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess https",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should handle deeply nested config blocks correctly");
    }

    #[test]
    fn test_parse_management_vlan_no_ip() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit vlan77",
            "        set allowaccess ping https ssh snmp",
            "    next",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, None, "Should return None when VLAN has allowaccess but no IP");
    }

    #[test]
    fn test_parse_management_vlan_none() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system global",
            "    set hostname \"fortiswitch-01\"",
            "end",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, None, "Should return None when no VLAN interfaces exist");
    }

    #[test]
    fn test_parse_management_vlan_at_end() {
        let switch = create_test_switch();

        let running_config = vec![
            "config system interface",
            "    edit port1",
            "        set mode static",
            "    next",
            "    edit vlan77",
            "        set ip 192.168.77.1 255.255.255.0",
            "        set allowaccess ping https ssh snmp",
        ];

        let result = switch.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should parse VLAN at end of config");
    }

    // ========== VLAN Boundary ID Tests ==========

    #[test]
    fn test_generate_vlan_commands_boundary_id_1() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 1,
            name: "default".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        assert!(commands.contains(&"config switch vlan".to_string()));
        assert!(commands.contains(&"edit 1".to_string()));
        assert!(commands.contains(&"next".to_string()));
        assert!(commands.contains(&"end".to_string()));
        // No SVI should be created for VlanIpConfig::None
        assert!(!commands.contains(&"edit vlan1".to_string()));
    }

    #[test]
    fn test_generate_vlan_commands_boundary_id_4094() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 4094,
            name: "max-vlan".to_string(),
            description: Some("Maximum VLAN ID".to_string()),
            ip_config: VlanIpConfig::Static {
                address: "10.40.94.1".to_string(),
                netmask: "255.255.255.0".to_string(),
            },
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        // Layer 2 VLAN creation
        assert!(commands.contains(&"config switch vlan".to_string()));
        assert!(commands.contains(&"edit 4094".to_string()));

        // Layer 3 SVI creation
        assert!(commands.contains(&"config system interface".to_string()));
        assert!(commands.contains(&"edit vlan4094".to_string()));
        assert!(commands.contains(&"set vlanid 4094".to_string()));
        assert!(commands.contains(&"set description \"Maximum VLAN ID\"".to_string()));
        assert!(commands.contains(&"set ip 10.40.94.1 255.255.255.0".to_string()));
        assert!(commands.contains(&"set allowaccess ping".to_string()));
    }

    // ========== VLAN Name with Special Characters ==========

    #[test]
    fn test_generate_vlan_commands_name_with_backtick() {
        let switch = create_test_switch();
        let vlans = vec![Vlan {
            id: 50,
            name: "test`vlan".to_string(),
            description: Some("VLAN with `backtick` in desc".to_string()),
            ip_config: VlanIpConfig::Dhcp,
        }];

        let commands = switch.generate_vlan_commands(&vlans);

        // Layer 2 VLAN creation (no name is set here for FortiSwitch)
        assert!(commands.contains(&"config switch vlan".to_string()));
        assert!(commands.contains(&"edit 50".to_string()));

        // Layer 3 SVI - description should be passed through as-is
        assert!(commands.contains(&"config system interface".to_string()));
        assert!(commands.contains(&"edit vlan50".to_string()));
        assert!(commands.contains(&"set description \"VLAN with `backtick` in desc\"".to_string()));
        assert!(commands.contains(&"set mode dhcp".to_string()));
    }

    // ========== mac_notify Handling ==========

    #[test]
    fn test_generate_port_commands_mac_notify_not_supported() {
        let switch = create_test_switch();
        let ports = vec![Port {
            port_id: "5".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            tagged_vlans: vec![],
            description: Some("Test Port".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: true,
            speed_duplex: SpeedDuplex::Auto,
            vlan_name: None,
            tagged_vlan_refs: vec![],
        }];

        let commands = switch.generate_port_commands(&ports);

        // FortiSwitch does not support per-port mac_notify, so no
        // mac-notification commands should appear anywhere in the output.
        // Skip description commands since they are user-provided strings.
        for cmd in &commands {
            if cmd.starts_with("set description") {
                continue;
            }
            assert!(
                !cmd.to_lowercase().contains("mac-notif"),
                "FortiSwitch should not generate mac-notification commands, but found: {}",
                cmd
            );
            assert!(
                !cmd.to_lowercase().contains("mac_notif"),
                "FortiSwitch should not generate mac_notify commands, but found: {}",
                cmd
            );
        }

        // Verify it still generates normal port commands correctly
        assert!(commands.contains(&"config switch interface".to_string()));
        assert!(commands.contains(&"edit port5".to_string()));
        assert!(commands.contains(&"set native-vlan 10".to_string()));
        assert!(commands.contains(&"config switch physical-port".to_string()));
        assert!(commands.contains(&"set status up".to_string()));
        assert!(commands.contains(&"set speed auto".to_string()));
    }

    // ========== Full State Parsing Tests ==========
    //
    // Regression coverage for the incident on IT-02876: `parse_current_state`
    // used to only ever extract `management_vlan`, so it always reported an
    // empty state — tripping the "parsed state completely empty but desired
    // config is not" safety check on every single reconcile, forever. These
    // tests exercise the parsers against hand-built FortiOS CLI output
    // matching this file's own command generators exactly (see
    // `generate_vlan_commands`/`generate_port_commands`/etc.) — the same
    // round-trip already relied on by `parse_management_vlan`.

    fn create_test_switch_with_vlans(vlans: Vec<Vlan>) -> FortiswitchSwitch {
        let mut config = create_test_config();
        config.vlans = vlans;
        FortiswitchSwitch::new(config, RuntimeConfig::default(), false)
    }

    #[test]
    fn test_parse_switch_vlan_ids_multiple() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch vlan",
            "    edit 1",
            "    next",
            "    edit 101",
            "    next",
            "    edit 102",
            "    next",
            "end",
        ];
        assert_eq!(switch.parse_switch_vlan_ids(&lines), vec![1, 101, 102]);
    }

    #[test]
    fn test_parse_switch_vlan_ids_empty_block() {
        let switch = create_test_switch();
        let lines = vec!["config switch vlan", "end"];
        assert!(switch.parse_switch_vlan_ids(&lines).is_empty());
    }

    #[test]
    fn test_parse_switch_vlan_ids_no_block() {
        let switch = create_test_switch();
        let lines = vec!["config system interface", "end"];
        assert!(switch.parse_switch_vlan_ids(&lines).is_empty());
    }

    #[test]
    fn test_parse_svi_details_static_ip_and_description() {
        let switch = create_test_switch();
        let lines = vec![
            "config system interface",
            "    edit vlan101",
            "        set vlanid 101",
            "        set description \"Setup 1\"",
            "        set ip 192.168.101.1 255.255.255.0",
            "    next",
            "end",
        ];
        let details = switch.parse_svi_details(&lines);
        let d = details.get(&101).expect("vlan101 should be present");
        assert_eq!(d.description, Some("Setup 1".to_string()));
        assert_eq!(d.ip_config, VlanIpConfig::Static {
            address: "192.168.101.1".to_string(),
            netmask: "255.255.255.0".to_string(),
        });
    }

    #[test]
    fn test_parse_svi_details_dhcp() {
        let switch = create_test_switch();
        let lines = vec![
            "config system interface",
            "    edit vlan42",
            "        set mode dhcp",
            "    next",
            "end",
        ];
        let details = switch.parse_svi_details(&lines);
        assert_eq!(details.get(&42).unwrap().ip_config, VlanIpConfig::Dhcp);
    }

    #[test]
    fn test_parse_svi_details_multiple_interfaces() {
        let switch = create_test_switch();
        let lines = vec![
            "config system interface",
            "    edit vlan42",
            "        set mode dhcp",
            "    next",
            "    edit \"vlan101\"",
            "        set ip 192.168.101.1 255.255.255.0",
            "    next",
            "end",
        ];
        let details = switch.parse_svi_details(&lines);
        assert_eq!(details.len(), 2);
        assert_eq!(details.get(&42).unwrap().ip_config, VlanIpConfig::Dhcp);
        assert!(matches!(details.get(&101).unwrap().ip_config, VlanIpConfig::Static { .. }));
    }

    #[test]
    fn test_parse_svi_details_none_when_no_interfaces() {
        let switch = create_test_switch();
        let lines = vec!["config system interface", "end"];
        assert!(switch.parse_svi_details(&lines).is_empty());
    }

    #[test]
    fn test_parse_current_state_vlans_name_from_desired_config() {
        // The device never stores a VLAN's name (see generate_vlan_commands) —
        // only its id, so the name must come from the desired config when we
        // have one for that id.
        let switch = create_test_switch_with_vlans(vec![
            Vlan { id: 101, name: "setup-1".to_string(), description: None, ip_config: VlanIpConfig::None },
        ]);
        let switch_vlan_lines = vec!["config switch vlan", "    edit 101", "    next", "end"];
        let svi_details = switch.parse_svi_details(&["config system interface", "end"]);
        let vlan_ids = switch.parse_switch_vlan_ids(&switch_vlan_lines);

        assert_eq!(vlan_ids, vec![101]);
        assert!(svi_details.is_empty(), "no SVI for a VLAN with ip_config: none");
    }

    #[test]
    fn test_parse_switch_interface_ports_access_port() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch interface",
            "    edit port1",
            "        set description \"RTX3481\"",
            "        set native-vlan 101",
            "        set allowed-vlans 101",
            "        set untagged-vlans 101",
            "    next",
            "end",
        ];
        let ports = switch.parse_switch_interface_ports(&lines);
        let p = ports.get("port1").expect("port1 should be present");
        assert_eq!(p.description, Some("RTX3481".to_string()));
        assert_eq!(p.native_vlan, Some(101));
        assert_eq!(p.allowed_vlans, vec![101]);
    }

    #[test]
    fn test_parse_switch_interface_ports_trunk_port() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch interface",
            "    edit port26",
            "        set description \"Router\"",
            "        set native-vlan 666",
            "        set allowed-vlans 42 101 102 666",
            "        set untagged-vlans 666",
            "    next",
            "end",
        ];
        let ports = switch.parse_switch_interface_ports(&lines);
        let p = ports.get("port26").unwrap();
        assert_eq!(p.native_vlan, Some(666));
        assert_eq!(p.allowed_vlans, vec![42, 101, 102, 666]);
    }

    #[test]
    fn test_parse_switch_interface_ports_multiple_ports() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch interface",
            "    edit port1",
            "        set native-vlan 101",
            "        set allowed-vlans 101",
            "    next",
            "    edit port2",
            "        set native-vlan 102",
            "        set allowed-vlans 102",
            "    next",
            "end",
        ];
        let ports = switch.parse_switch_interface_ports(&lines);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports.get("port1").unwrap().native_vlan, Some(101));
        assert_eq!(ports.get("port2").unwrap().native_vlan, Some(102));
    }

    #[test]
    fn test_parse_physical_ports_enabled_poe_speed() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch physical-port",
            "    edit port1",
            "        set status up",
            "        set poe-status enable",
            "        set speed auto",
            "    next",
            "    edit port2",
            "        set status down",
            "        set poe-status disable",
            "        set speed 1000full",
            "    next",
            "end",
        ];
        let ports = switch.parse_physical_ports(&lines);
        let p1 = ports.get("port1").unwrap();
        assert_eq!(p1.enabled, Some(true));
        assert_eq!(p1.poe_enabled, Some(true));
        assert_eq!(p1.speed_duplex, Some(SpeedDuplex::Auto));

        let p2 = ports.get("port2").unwrap();
        assert_eq!(p2.enabled, Some(false));
        assert_eq!(p2.poe_enabled, Some(false));
        assert_eq!(p2.speed_duplex, Some(SpeedDuplex::ThousandFull));
    }

    #[test]
    fn test_parse_physical_ports_absent_lines_stay_none() {
        // Real hardware (FortiSwitch 124F-FPOE): show switch physical-port
        // never prints set status/set poe-status when they're already at
        // this switch's own default — only lldp-profile and speed appeared.
        let switch = create_test_switch();
        let lines = vec![
            "config switch physical-port",
            "    edit \"port1\"",
            "        set lldp-profile \"default-auto-isl\"",
            "        set speed auto",
            "    next",
            "end",
        ];
        let ports = switch.parse_physical_ports(&lines);
        let p1 = ports.get("port1").unwrap();
        assert_eq!(p1.enabled, None, "status line never appeared, must not be assumed");
        assert_eq!(p1.poe_enabled, None, "poe-status line never appeared, must not be assumed");
        assert_eq!(p1.speed_duplex, Some(SpeedDuplex::Auto));
    }

    #[test]
    fn test_parse_vlan_list_comma_separated_with_ranges() {
        // Real hardware (FortiSwitch 124F-FPOE): `show switch interface`
        // compressed a trunk's allowed-vlans into exactly this form.
        assert_eq!(
            FortiswitchSwitch::parse_vlan_list("42,101-104,501"),
            vec![42, 101, 102, 103, 104, 501]
        );
    }

    #[test]
    fn test_parse_vlan_list_space_separated_no_ranges() {
        // The write side (generate_port_commands) uses this form.
        assert_eq!(FortiswitchSwitch::parse_vlan_list("42 101 102"), vec![42, 101, 102]);
    }

    #[test]
    fn test_parse_vlan_list_single_value() {
        assert_eq!(FortiswitchSwitch::parse_vlan_list("101"), vec![101]);
    }

    #[test]
    fn test_parse_switch_interface_ports_trunk_with_range_notation() {
        // Regression: the original bug found on real hardware — a
        // whitespace-only split against "42,101-104,501" (no spaces at all)
        // parsed as a single invalid token and silently dropped everything.
        let switch = create_test_switch();
        let lines = vec![
            "config switch interface",
            "    edit \"port26\"",
            "        set native-vlan 666",
            "        set allowed-vlans 42,101-104,666",
            "    next",
            "end",
        ];
        let ports = switch.parse_switch_interface_ports(&lines);
        let p = ports.get("port26").unwrap();
        assert_eq!(p.allowed_vlans, vec![42, 101, 102, 103, 104, 666]);
    }

    #[test]
    fn test_build_ports_poe_capable_port_defaults_to_enabled_when_unmentioned() {
        // Regression: PhysicalPortInfo::poe_enabled being None (the
        // set poe-status line never appeared) used to fall back to a flat
        // `false`, permanently misreporting every already-correct PoE port
        // as needing reconfiguration. Port "1" on Fortiswitch124F_FPOE is a
        // PoE-capable copper port (see models.rs port_capabilities), so the
        // fallback must be `true`, not `false`.
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port1".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(101),
            allowed_vlans: vec![101],
        });
        let mut physical = std::collections::HashMap::new();
        physical.insert("port1".to_string(), PhysicalPortInfo {
            enabled: None,
            poe_enabled: None,
            speed_duplex: None,
        });

        let ports = switch.build_ports(interfaces, physical);
        let p = &ports[0];
        assert!(p.enabled, "absent status line should default to up");
        assert!(p.poe_enabled, "absent poe-status on a PoE-capable port should default to enabled");
        assert_eq!(p.speed_duplex, SpeedDuplex::Auto);
    }

    #[test]
    fn test_is_numbered_port() {
        assert!(FortiswitchSwitch::is_numbered_port("port1"));
        assert!(FortiswitchSwitch::is_numbered_port("port24"));
        assert!(!FortiswitchSwitch::is_numbered_port("internal"));
        assert!(!FortiswitchSwitch::is_numbered_port("port"));
        assert!(!FortiswitchSwitch::is_numbered_port("portA"));
    }

    #[test]
    fn test_build_ports_excludes_internal_interface() {
        // Regression: real hardware's `show switch interface` lists
        // FortiSwitch's own "internal" CPU/management interface alongside
        // the real numbered ports. Treating it as a stray configurable port
        // made the diff engine want to "reset" it every reconcile cycle
        // (enforce_port_config was on) even though it was never a real port.
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port1".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(101),
            allowed_vlans: vec![101],
        });
        interfaces.insert("internal".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(42),
            allowed_vlans: vec![42],
        });

        let ports = switch.build_ports(interfaces, std::collections::HashMap::new());
        assert_eq!(ports.len(), 1, "the 'internal' interface must be excluded");
        assert_eq!(ports[0].port_id, "1");
    }

    #[test]
    fn test_build_ports_non_poe_port_defaults_to_disabled_when_unmentioned() {
        // Port "25" on Fortiswitch124F_FPOE is an SFP+ uplink — not
        // PoE-capable at all — so an absent poe-status line must default to
        // false there, unlike a copper port.
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port25".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(666),
            allowed_vlans: vec![666],
        });
        let mut physical = std::collections::HashMap::new();
        physical.insert("port25".to_string(), PhysicalPortInfo::default());

        let ports = switch.build_ports(interfaces, physical);
        assert!(!ports[0].poe_enabled);
    }

    #[test]
    fn test_build_ports_merges_interface_and_physical_access_mode() {
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port1".to_string(), PortInterfaceInfo {
            description: Some("RTX3481".to_string()),
            native_vlan: Some(101),
            allowed_vlans: vec![101],
        });
        let mut physical = std::collections::HashMap::new();
        physical.insert("port1".to_string(), PhysicalPortInfo {
            enabled: Some(true),
            poe_enabled: Some(true),
            speed_duplex: Some(SpeedDuplex::Auto),
        });

        let ports = switch.build_ports(interfaces, physical);
        assert_eq!(ports.len(), 1);
        let p = &ports[0];
        assert_eq!(p.port_id, "1");
        assert_eq!(p.vlan, 101);
        assert!(p.tagged_vlans.is_empty());
        assert_eq!(p.mode, PortMode::Access);
        assert!(p.enabled);
        assert!(p.poe_enabled);
        assert_eq!(p.description, Some("RTX3481".to_string()));
    }

    #[test]
    fn test_build_ports_trunk_mode_excludes_native_from_tagged() {
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port26".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(666),
            allowed_vlans: vec![42, 101, 102, 666],
        });
        let ports = switch.build_ports(interfaces, std::collections::HashMap::new());
        let p = &ports[0];
        assert_eq!(p.port_id, "26");
        assert_eq!(p.vlan, 666);
        assert_eq!(p.tagged_vlans, vec![42, 101, 102]);
        assert_eq!(p.mode, PortMode::Trunk);
    }

    #[test]
    fn test_build_ports_missing_physical_falls_back_to_defaults() {
        // Port "5" is a PoE-capable copper port on Fortiswitch124F_FPOE, so
        // with no physical-port data at all the model-aware fallback (see
        // build_ports) defaults poe_enabled to true, not a flat false.
        let switch = create_test_switch();
        let mut interfaces = std::collections::HashMap::new();
        interfaces.insert("port5".to_string(), PortInterfaceInfo {
            description: None,
            native_vlan: Some(10),
            allowed_vlans: vec![10],
        });
        let ports = switch.build_ports(interfaces, std::collections::HashMap::new());
        let p = &ports[0];
        assert!(p.enabled, "default physical state should be enabled");
        assert!(p.poe_enabled, "port 5 is PoE-capable, so the fallback should be enabled");
        assert_eq!(p.speed_duplex, SpeedDuplex::Auto);
    }

    #[test]
    fn test_parse_switch_mirrors_both_direction() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch mirror",
            "    edit 1",
            "        set status active",
            "        set dst port22",
            "        set src-ingress port15",
            "        set src-egress port15",
            "    next",
            "end",
        ];
        let mirrors = switch.parse_switch_mirrors(&lines);
        assert_eq!(mirrors.len(), 1);
        let m = &mirrors[0];
        assert_eq!(m.session_id, "1");
        assert_eq!(m.destination_port, "22");
        assert_eq!(m.source_ports, vec!["15".to_string()]);
        assert_eq!(m.direction, MirrorDirection::Both);
    }

    #[test]
    fn test_parse_switch_mirrors_rx_only() {
        let switch = create_test_switch();
        let lines = vec![
            "config switch mirror",
            "    edit 1",
            "        set dst port22",
            "        set src-ingress port15 port16",
            "    next",
            "end",
        ];
        let mirrors = switch.parse_switch_mirrors(&lines);
        assert_eq!(mirrors[0].direction, MirrorDirection::Rx);
        assert_eq!(mirrors[0].source_ports, vec!["15".to_string(), "16".to_string()]);
    }

    #[test]
    fn test_parse_switch_mirrors_ignores_incomplete_session() {
        // A session with no dst/src configured yet (or only a status line)
        // must not be reported as a real mirror.
        let switch = create_test_switch();
        let lines = vec![
            "config switch mirror",
            "    edit 1",
            "        set status active",
            "    next",
            "end",
        ];
        assert!(switch.parse_switch_mirrors(&lines).is_empty());
    }

    #[test]
    fn test_parse_switch_mirrors_none_when_no_sessions() {
        let switch = create_test_switch();
        let lines = vec!["config switch mirror", "end"];
        assert!(switch.parse_switch_mirrors(&lines).is_empty());
    }

    #[test]
    fn test_parse_snmp_community_with_trap_receivers_and_events() {
        let switch = create_test_switch();
        let lines = vec![
            "config system snmp community",
            "    edit 1",
            "        set name \"public\"",
            "        set status enable",
            "        set query-v1-status enable",
            "        set query-v2c-status enable",
            "        set trap-v1-status enable",
            "        set trap-v2c-status enable",
            "        set events mac-notify link-up-down",
            "        config hosts",
            "            edit 1",
            "                set ip 192.168.1.1",
            "                set interface internal",
            "            next",
            "        end",
            "    next",
            "end",
        ];
        let snmp = switch.parse_snmp_community(&lines).expect("SNMP should be configured");
        assert_eq!(snmp.communities.len(), 1);
        assert_eq!(snmp.communities[0].name, "public");
        assert_eq!(snmp.trap_receivers.len(), 1);
        assert_eq!(snmp.trap_receivers[0].host, "192.168.1.1");
        assert_eq!(snmp.trap_receivers[0].community, "public");
        assert_eq!(snmp.enabled_traps, vec![TrapType::MacNotify, TrapType::LinkChange]);
    }

    #[test]
    fn test_parse_snmp_community_multiple_communities_no_traps() {
        let switch = create_test_switch();
        let lines = vec![
            "config system snmp community",
            "    edit 1",
            "        set name \"public\"",
            "        set status enable",
            "    next",
            "    edit 2",
            "        set name \"private\"",
            "        set status enable",
            "    next",
            "end",
        ];
        let snmp = switch.parse_snmp_community(&lines).unwrap();
        let names: Vec<&str> = snmp.communities.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["public", "private"]);
        assert!(snmp.trap_receivers.is_empty());
    }

    #[test]
    fn test_parse_snmp_community_none_when_not_configured() {
        let switch = create_test_switch();
        let lines = vec!["config system snmp community", "end"];
        assert!(switch.parse_snmp_community(&lines).is_none());
    }

    #[test]
    fn test_parse_snmp_community_indices_single() {
        // Regression: real hardware shipped a factory-default "public"
        // community at index 1 that our desired config never declared.
        // Removing it requires `delete 1`, not `delete public` — FortiOS's
        // `config system snmp community` is index-based, and a bare `delete
        // <name>` is silently a no-op (confirmed: the community was still
        // present on the very next read).
        let lines = vec![
            "config system snmp community",
            "    edit 1",
            "        set name \"public\"",
            "    next",
            "end",
        ];
        let indices = FortiswitchSwitch::parse_snmp_community_indices(&lines);
        assert_eq!(indices.get("public"), Some(&1));
    }

    #[test]
    fn test_parse_snmp_community_indices_multiple() {
        let lines = vec![
            "config system snmp community",
            "    edit 1",
            "        set name \"public\"",
            "    next",
            "    edit 2",
            "        set name \"private\"",
            "    next",
            "end",
        ];
        let indices = FortiswitchSwitch::parse_snmp_community_indices(&lines);
        assert_eq!(indices.get("public"), Some(&1));
        assert_eq!(indices.get("private"), Some(&2));
    }

    #[test]
    fn test_parse_snmp_community_indices_ignores_host_entries() {
        // The nested "config hosts" block also has "edit <N>" entries (trap
        // receiver hosts) that must not be mistaken for community indices.
        let lines = vec![
            "config system snmp community",
            "    edit 1",
            "        set name \"public\"",
            "        config hosts",
            "            edit 1",
            "                set ip 192.168.1.1",
            "            next",
            "        end",
            "    next",
            "end",
        ];
        let indices = FortiswitchSwitch::parse_snmp_community_indices(&lines);
        assert_eq!(indices.len(), 1);
        assert_eq!(indices.get("public"), Some(&1));
    }

    #[test]
    fn test_parse_snmp_community_indices_empty_when_not_configured() {
        let lines = vec!["config system snmp community", "end"];
        assert!(FortiswitchSwitch::parse_snmp_community_indices(&lines).is_empty());
    }

    #[test]
    fn test_denormalize_port_id() {
        assert_eq!(FortiswitchSwitch::denormalize_port_id("port1"), "1");
        assert_eq!(FortiswitchSwitch::denormalize_port_id("port24"), "24");
        assert_eq!(FortiswitchSwitch::denormalize_port_id("1"), "1");
    }

    #[test]
    fn test_parse_speed_from_fortiswitch_round_trips() {
        let switch = create_test_switch();
        for speed in [
            SpeedDuplex::Auto, SpeedDuplex::TenHalf, SpeedDuplex::TenFull,
            SpeedDuplex::HundredHalf, SpeedDuplex::HundredFull,
            SpeedDuplex::ThousandFull, SpeedDuplex::TenGFull,
        ] {
            let written = switch.convert_speed_to_fortiswitch(&speed);
            assert_eq!(FortiswitchSwitch::parse_speed_from_fortiswitch(&written), speed);
        }
    }

    #[test]
    fn test_parse_trap_type_from_fortiswitch_round_trips() {
        let switch = create_test_switch();
        for trap in [TrapType::MacNotify, TrapType::LinkChange, TrapType::All] {
            let written = switch.convert_trap_type_to_fortiswitch(&trap);
            assert_eq!(FortiswitchSwitch::parse_trap_type_from_fortiswitch(&written), Some(trap));
        }
    }
}
