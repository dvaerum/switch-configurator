use super::traits::{SwitchVendor, VendorError};
use crate::config::RuntimeConfig;
use crate::models::{ConfigResult, MirrorDirection, Port, PortMirror, PortMode, StateDiff, SwitchConfig, SwitchState, Vlan, VlanIpConfig, ConnectionType};
use crate::ssh::{ConnectionClient, SerialClient, SshClient};
use async_trait::async_trait;
use tracing::{debug, info, warn};

pub struct FortiswitchSwitch {
    config: SwitchConfig,
    runtime_config: RuntimeConfig,
    client: Option<ConnectionClient>,
    enforce_port_config: bool,
    current_state: Option<SwitchState>,
}

impl FortiswitchSwitch {
    pub fn new(config: SwitchConfig, runtime_config: RuntimeConfig, enforce_port_config: bool) -> Self {
        Self {
            config,
            runtime_config,
            client: None,
            enforce_port_config,
            current_state: None,
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

            match port.mode {
                PortMode::Access => {
                    // Access port: native VLAN is untagged, only that VLAN allowed
                    commands.push(format!("set native-vlan {}", port.vlan));
                    commands.push(format!("set allowed-vlans {}", port.vlan));
                    commands.push(format!("set untagged-vlans {}", port.vlan));
                }
                PortMode::Trunk => {
                    // Trunk port: native VLAN + multiple allowed VLANs
                    commands.push(format!("set native-vlan {}", port.vlan));
                    if !port.allowed_vlans.is_empty() {
                        let vlans: Vec<String> =
                            port.allowed_vlans.iter().map(|v| v.to_string()).collect();
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

        // Remove communities (by deleting the entry)
        for community_name in &snmp_diff.communities_to_remove {
            info!("Removing SNMP community: {}", community_name);
            // FortiSwitch uses index-based editing, but we can try to delete by name
            // Note: This may need to be done by finding the index first in real implementation
            commands.push(format!("delete {}", community_name));
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

                // FortiSwitch pagination control
                // Note: Most FortiSwitch models handle output without pagination issues.
                // If "--More--" prompts appear, the correct command sequence is:
                // config system console -> set output standard -> end
                // However, this may not be supported on all models.

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

                // FortiSwitch pagination control
                // Note: FortiSwitch serial connections typically don't require pagination disabling
                // as they output full responses without "--More--" prompts. If pagination becomes
                // an issue, the correct FortiSwitch command would be:
                // config system console -> set output standard -> end
                // However, this command may not be supported on all models and can cause timeouts.

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
        let config = self.get_running_config().await?;
        let lines: Vec<&str> = config.lines().collect();

        // Parse management VLAN (detect VLAN interfaces with IP and allowaccess)
        let management_vlan = self.parse_management_vlan(&lines);

        debug!("Parsed FortiSwitch state: Management VLAN: {:?}", management_vlan);

        // Add management VLAN to vlans vec if it exists, to prevent diff from re-adding it
        let mut vlans = vec![];
        if let Some(vlan_id) = management_vlan {
            // Find the VLAN config to get the full details
            if let Some(vlan_config) = self.config.vlans.iter().find(|v| v.id == vlan_id) {
                vlans.push(vlan_config.clone());
                debug!("Added management VLAN {} to parsed state to prevent re-configuration", vlan_id);
            }
        }

        // Verify hardware model by running "get system status"
        // This returns lines like "Version: FortiSwitch-108F-POE v7.2.8,build0660,..."
        let warnings = self.detect_hardware_model().await;

        // TODO: Implement full state parsing (VLANs, ports, mirrors, SNMP)
        // For now, we only parse management_vlan for idempotency
        warn!(
            "FortiSwitch state parsing partially implemented for {}. Only management_vlan is parsed.",
            self.config.hostname()
        );

        Ok(SwitchState {
            vlans,
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
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
        self.apply_diff(&diff).await
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
        use crate::models::{ConnectionType, Credentials, SwitchModel, Vendor};

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
            allowed_vlans: vec![],
            description: Some("Test Port".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
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
            allowed_vlans: vec![1, 10, 20, 30],
            description: Some("Uplink".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::ThousandFull,
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
            allowed_vlans: vec![],
            description: Some("Disabled Port".to_string()),
            enabled: false,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
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
            allowed_vlans: vec![],
            description: Some("PoE Port".to_string()),
            enabled: true,
            poe_enabled: true,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
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
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,
            },
            Port {
                port_id: "3".to_string(),
                mode: PortMode::Access,
                vlan: 1,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::ThousandFull,
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
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
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
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
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
                allowed_vlans: vec![],
                description: Some("Management Port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                allowed_vlans: vec![1, 10],
                description: Some("Uplink".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::ThousandFull,
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
        use crate::config::RuntimeConfig;

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
        use crate::config::RuntimeConfig;

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
        use crate::config::RuntimeConfig;

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
        use crate::config::RuntimeConfig;

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
}
