use super::traits::{SwitchVendor, VendorError};
use crate::config::RuntimeConfig;
use crate::models::{ConfigResult, MirrorDirection, Port, PortMirror, PortMode, StateDiff, SwitchConfig, SwitchState, Vlan, VlanIpConfig, ConnectionType};
use crate::ssh::{ConnectionClient, SerialClient, SshClient};
use async_trait::async_trait;
use tracing::{debug, info, warn};

pub struct CiscoSwitch {
    config: SwitchConfig,
    runtime_config: RuntimeConfig,
    client: Option<ConnectionClient>,
    current_state: Option<SwitchState>,
    enforce_port_config: bool,
}

impl CiscoSwitch {
    pub fn new(config: SwitchConfig, runtime_config: RuntimeConfig, enforce_port_config: bool) -> Self {
        Self {
            config,
            runtime_config,
            client: None,
            current_state: None,
            enforce_port_config,
        }
    }

    fn generate_vlan_commands(&self, vlans: &[Vlan]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for vlan in vlans {
            commands.push(format!("vlan {}", vlan.id));
            commands.push(format!("name {}", vlan.name));
            if let Some(desc) = &vlan.description {
                commands.push(format!("description {}", desc));
            }
            commands.push("exit".to_string());
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_port_commands(&self, ports: &[Port]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for port in ports {
            let interface = self.normalize_port_id(&port.port_id);
            commands.push(format!("interface {}", interface));

            if let Some(desc) = &port.description {
                commands.push(format!("description {}", desc));
            }

            match port.mode {
                PortMode::Access => {
                    commands.push("switchport mode access".to_string());
                    commands.push(format!("switchport access vlan {}", port.vlan));
                }
                PortMode::Trunk => {
                    commands.push("switchport mode trunk".to_string());
                    commands.push(format!("switchport trunk native vlan {}", port.vlan));
                    if !port.allowed_vlans.is_empty() {
                        let vlans: Vec<String> =
                            port.allowed_vlans.iter().map(|v| v.to_string()).collect();
                        commands.push(format!("switchport trunk allowed vlan {}", vlans.join(",")));
                    } else {
                        commands.push("switchport trunk allowed vlan all".to_string());
                    }
                }
            }

            if port.enabled {
                commands.push("no shutdown".to_string());
            } else {
                commands.push("shutdown".to_string());
            }

            if port.poe_enabled {
                commands.push("power inline auto".to_string());
            } else {
                commands.push("power inline never".to_string());
            }

            if port.mac_notify {
                commands.push("snmp trap mac-notification change added".to_string());
                commands.push("snmp trap mac-notification change removed".to_string());
            } else {
                commands.push("no snmp trap mac-notification change added".to_string());
                commands.push("no snmp trap mac-notification change removed".to_string());
            }

            // Configure speed and duplex (Cisco uses separate commands)
            commands.push(format!("speed {}", port.speed_duplex.to_cisco_speed()));
            commands.push(format!("duplex {}", port.speed_duplex.to_cisco_duplex()));

            commands.push("exit".to_string());
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_mirror_commands(&self, mirrors: &[PortMirror]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for mirror in mirrors {
            // Remove existing session first
            commands.push(format!("no monitor session {}", mirror.session_id));
            commands.push(format!("monitor session {}", mirror.session_id));

            // Source ports
            for source in &mirror.source_ports {
                let port = self.normalize_port_id(source);
                let direction = match mirror.direction {
                    MirrorDirection::Rx => "rx",
                    MirrorDirection::Tx => "tx",
                    MirrorDirection::Both => "both",
                };
                commands.push(format!(
                    "monitor session {} source interface {} {}",
                    mirror.session_id, port, direction
                ));
            }

            // Destination port
            let dest = self.normalize_port_id(&mirror.destination_port);
            commands.push(format!(
                "monitor session {} destination interface {}",
                mirror.session_id, dest
            ));
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_snmp_commands(&self, snmp_config: &crate::models::SnmpConfig) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        // Configure SNMP communities
        for community in &snmp_config.communities {
            let access_level = match community.access {
                crate::models::SnmpAccess::Unrestricted => "RW",
                crate::models::SnmpAccess::Manager => "RW",
                crate::models::SnmpAccess::Operator => "RO",
            };
            commands.push(format!("snmp-server community {} {}", community.name, access_level));
        }

        // Configure SNMP trap receivers
        for receiver in &snmp_config.trap_receivers {
            let version = receiver.version.as_deref().unwrap_or("2c");
            commands.push(format!(
                "snmp-server host {} version {} {}",
                receiver.host, version, receiver.community
            ));
        }

        // Enable SNMP traps
        for trap_type in &snmp_config.enabled_traps {
            match trap_type {
                crate::models::TrapType::MacNotify => {
                    commands.push("snmp-server enable traps mac-notification".to_string());
                }
                crate::models::TrapType::LinkChange => {
                    commands.push("snmp-server enable traps link".to_string());
                }
                crate::models::TrapType::All => {
                    commands.push("snmp-server enable traps".to_string());
                }
            }
        }

        commands.push("end".to_string());
        commands
    }

    /// Parse management VLAN from Cisco running config
    /// Detects SVIs (Switched Virtual Interfaces) with IP addresses
    /// Returns the VLAN ID if an SVI with IP is found
    fn parse_management_vlan(&self, lines: &[&str]) -> Option<u16> {
        let mut in_interface_vlan = false;
        let mut current_vlan_id: Option<u16> = None;
        let mut has_ip_address = false;

        for line in lines {
            let trimmed = line.trim();

            // Detect interface vlan X
            if let Some(rest) = trimmed.strip_prefix("interface Vlan") {
                // Parse VLAN ID
                if let Some(vlan_str) = rest.split_whitespace().next() {
                    if let Ok(vlan_id) = vlan_str.parse::<u16>() {
                        in_interface_vlan = true;
                        current_vlan_id = Some(vlan_id);
                        has_ip_address = false;
                        debug!("  Found interface Vlan{}", vlan_id);
                    }
                }
            } else if in_interface_vlan {
                // Check for IP address configuration
                if trimmed.starts_with("ip address") && !trimmed.contains("dhcp") {
                    // Static IP: "ip address 192.168.88.1 255.255.255.0"
                    has_ip_address = true;
                    debug!("    Found static IP address on Vlan{:?}", current_vlan_id);
                } else if trimmed == "ip address dhcp" {
                    // DHCP: "ip address dhcp"
                    has_ip_address = true;
                    debug!("    Found DHCP IP address on Vlan{:?}", current_vlan_id);
                } else if trimmed.starts_with("interface ") || trimmed.starts_with("!") {
                    // Exiting this interface section
                    // If we found a VLAN with an IP, this is likely the management VLAN
                    if has_ip_address && current_vlan_id.is_some() {
                        debug!("  Detected management VLAN: {:?}", current_vlan_id);
                        return current_vlan_id;
                    }
                    in_interface_vlan = false;
                    current_vlan_id = None;
                    has_ip_address = false;
                }
            }
        }

        // Check if the last interface had an IP (in case it's at the end of the config)
        if in_interface_vlan && has_ip_address && current_vlan_id.is_some() {
            debug!("  Detected management VLAN (at end): {:?}", current_vlan_id);
            return current_vlan_id;
        }

        None
    }

    fn normalize_port_id(&self, port_id: &str) -> String {
        // Cisco Catalyst 9300 uses formats like "GigabitEthernet1/0/1" or "Gi1/0/1"
        // Expand short forms to full interface names

        // Already in full format
        if port_id.starts_with("GigabitEthernet") {
            return port_id.to_string();
        }

        if port_id.starts_with("TenGigabitEthernet") {
            return port_id.to_string();
        }

        // Expand Gi short form to GigabitEthernet
        if port_id.starts_with("Gi") {
            return port_id.replacen("Gi", "GigabitEthernet", 1);
        }

        // Expand Te short form to TenGigabitEthernet
        if port_id.starts_with("Te") {
            return port_id.replacen("Te", "TenGigabitEthernet", 1);
        }

        // Convert simple format "1" or "1/0/1" to GigabitEthernet format
        if port_id.contains('/') {
            format!("GigabitEthernet{}", port_id)
        } else {
            format!("GigabitEthernet1/0/{}", port_id)
        }
    }

    fn generate_remove_vlan_commands(&self, vlan_ids: &[u16]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for vlan_id in vlan_ids {
            commands.push(format!("no vlan {}", vlan_id));
        }

        commands.push("end".to_string());
        commands
    }

    fn generate_remove_mirror_commands(&self, session_ids: &[String]) -> Vec<String> {
        let mut commands = vec!["configure terminal".to_string()];

        for session_id in session_ids {
            commands.push(format!("no monitor session {}", session_id));
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

    /// Migrate ports away from VLANs before deletion to avoid interactive confirmation prompts
    async fn migrate_ports_before_vlan_deletion(
        &mut self,
        migrations: &[crate::diff::PortMigration],
    ) -> Result<ConfigResult, VendorError> {
        use crate::diff::PortMigrationAction;

        let mut commands = Vec::new();
        commands.push("configure terminal".to_string());

        for migration in migrations {
            match &migration.action {
                PortMigrationAction::MoveAccessToVlan1 => {
                    debug!(
                        "Migrating access port {} from VLAN {} to VLAN 1",
                        migration.port_id, migration.vlan_being_removed
                    );
                    commands.push(format!("interface {}", migration.port_id));
                    commands.push("switchport access vlan 1".to_string());
                }
                PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans } => {
                    debug!(
                        "Removing VLAN {} from trunk port {} (remaining VLANs: {:?})",
                        migration.vlan_being_removed, migration.port_id, remaining_vlans
                    );
                    commands.push(format!("interface {}", migration.port_id));

                    if remaining_vlans.is_empty() {
                        // If no VLANs remain, switch to access mode on VLAN 1
                        commands.push("switchport mode access".to_string());
                        commands.push("switchport access vlan 1".to_string());
                    } else {
                        // Update allowed VLANs list
                        let vlan_list = remaining_vlans
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        commands.push(format!("switchport trunk allowed vlan {}", vlan_list));
                    }
                }
            }
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
    /// This is much more efficient than configure_snmp which does full replacement
    async fn apply_snmp_diff(
        &mut self,
        snmp_diff: &crate::models::SnmpStateDiff,
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
            commands.push(format!("no snmp-server community {}", community_name));
            actions.push(format!("removed community '{}'", community_name));
        }

        // Update communities (remove old, add new with different access)
        for community in &snmp_diff.communities_to_update {
            info!("Updating SNMP community: {} -> {:?}", community.name, community.access);
            commands.push(format!("no snmp-server community {}", community.name));
            let access_level = match community.access {
                crate::models::SnmpAccess::Unrestricted => "RW",
                crate::models::SnmpAccess::Manager => "RW",
                crate::models::SnmpAccess::Operator => "RO",
            };
            commands.push(format!("snmp-server community {} {}", community.name, access_level));
            actions.push(format!("updated community '{}' to {:?}", community.name, community.access));
        }

        // Add new communities
        for community in &snmp_diff.communities_to_add {
            info!("Adding SNMP community: {} ({:?})", community.name, community.access);
            let access_level = match community.access {
                crate::models::SnmpAccess::Unrestricted => "RW",
                crate::models::SnmpAccess::Manager => "RW",
                crate::models::SnmpAccess::Operator => "RO",
            };
            commands.push(format!("snmp-server community {} {}", community.name, access_level));
            actions.push(format!("added community '{}'", community.name));
        }

        // Remove trap receivers that are no longer wanted
        for host in &snmp_diff.trap_receivers_to_remove {
            info!("Removing SNMP trap receiver: {}", host);
            commands.push(format!("no snmp-server host {}", host));
            actions.push(format!("removed trap receiver '{}'", host));
        }

        // Add new trap receivers
        for receiver in &snmp_diff.trap_receivers_to_add {
            let version = receiver.version.as_deref().unwrap_or("2c");
            info!("Adding SNMP trap receiver: {} (community: {})", receiver.host, receiver.community);
            commands.push(format!(
                "snmp-server host {} version {} {}",
                receiver.host, version, receiver.community
            ));
            actions.push(format!("added trap receiver '{}'", receiver.host));
        }

        // Enable traps that aren't currently enabled
        for trap_type in &snmp_diff.traps_to_enable {
            match trap_type {
                TrapType::MacNotify => {
                    info!("Enabling mac-notification traps");
                    commands.push("snmp-server enable traps mac-notification".to_string());
                    actions.push("enabled mac-notification traps".to_string());
                }
                TrapType::LinkChange => {
                    info!("Enabling link traps");
                    commands.push("snmp-server enable traps link".to_string());
                    actions.push("enabled link traps".to_string());
                }
                TrapType::All => {
                    info!("Enabling all traps");
                    commands.push("snmp-server enable traps".to_string());
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
                    info!("Disabling mac-notification traps");
                    commands.push("no snmp-server enable traps mac-notification".to_string());
                    actions.push("disabled mac-notification traps".to_string());
                }
                TrapType::LinkChange => {
                    info!("Disabling link traps");
                    commands.push("no snmp-server enable traps link".to_string());
                    actions.push("disabled link traps".to_string());
                }
                TrapType::All => {
                    info!("Disabling all traps");
                    commands.push("no snmp-server enable traps".to_string());
                    actions.push("disabled all traps".to_string());
                }
                _ => {
                    debug!("Trap type {:?} not specifically handled for disable", trap_type);
                }
            }
        }

        commands.push("end".to_string());

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
impl SwitchVendor for CiscoSwitch {
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

                // Disable pagination to prevent "--More--" prompts
                ssh_client
                    .execute_command("terminal length 0")
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

                // Disable pagination to prevent "--More--" prompts
                serial_client
                    .execute_command("terminal length 0")
                    .await
                    .map_err(|e| VendorError::SshError(e.to_string()))?;

                ConnectionClient::Serial(serial_client)
            }
        };

        self.client = Some(client);
        info!("Connected to Cisco switch: {}", self.config.hostname());
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

        // Parse management VLAN (detect SVIs with IP addresses)
        let management_vlan = self.parse_management_vlan(&lines);

        debug!("Parsed Cisco state: Management VLAN: {:?}", management_vlan);

        // Verify hardware model against running config
        // Cisco IOS running config may contain "! model WS-C9300-24P" or similar
        let hardware_id_pattern = regex::Regex::new(
            r"^!\s*model\s+(\S+)"
        ).unwrap();
        let warnings = super::traits::verify_hardware_model(
            &config,
            &self.config.model(),
            &hardware_id_pattern,
        );

        // TODO: Implement full state parsing (VLANs, ports, mirrors, SNMP)
        // For now, we only parse management_vlan for idempotency
        warn!(
            "Cisco state parsing partially implemented for {}. Only management_vlan is parsed.",
            self.config.hostname()
        );

        Ok(SwitchState {
            vlans: vec![],
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
            // Migrate ports away from VLANs before deletion to avoid interactive prompts
            if let Some(current_state) = &self.current_state {
                let migrations = crate::diff::find_ports_to_migrate(current_state, &diff.vlans_to_remove);

                if !migrations.is_empty() {
                    debug!("Migrating {} ports before VLAN deletion", migrations.len());
                    results.push(self.migrate_ports_before_vlan_deletion(&migrations).await?);
                }
            }

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
        self.apply_diff(&diff).await
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

        // Note: Pagination is disabled at connection time with 'terminal length 0'
        debug!("Reading running configuration");
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

        // Build set of defined VLAN IDs for existence checks
        let defined_vlans: std::collections::HashSet<u16> =
            self.config.vlans.iter().map(|v| v.id).collect();

        // Validate port configurations
        for port in &self.config.ports {
            // Check VLAN ID range
            if port.vlan < 1 || port.vlan > 4094 {
                return Err(VendorError::ValidationError(format!(
                    "Invalid VLAN ID on port {}: {}",
                    port.port_id, port.vlan
                )));
            }

            // Check that the VLAN exists in configuration
            // For access mode: check the access VLAN
            // For trunk mode: check the native VLAN
            if !defined_vlans.contains(&port.vlan) {
                let vlan_type = match port.mode {
                    crate::models::PortMode::Access => "access",
                    crate::models::PortMode::Trunk => "native/untagged",
                };
                return Err(VendorError::ValidationError(format!(
                    "Port {} references non-existent VLAN {} as {} VLAN",
                    port.port_id, port.vlan, vlan_type
                )));
            }

            // For trunk ports, also check that allowed VLANs exist
            if port.mode == crate::models::PortMode::Trunk {
                for allowed_vlan in &port.allowed_vlans {
                    if !defined_vlans.contains(allowed_vlan) {
                        return Err(VendorError::ValidationError(format!(
                            "Port {} references non-existent VLAN {} in allowed VLANs list",
                            port.port_id, allowed_vlan
                        )));
                    }
                }
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

                // For Cisco IOS, use 'reload'
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
                    .execute_command("configure replace nvram:startup-config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Restore failed: {}", e)))?;

                info!("Configuration restored on {}", self.config.hostname());
            }
            RollbackMethod::RevertCommands => {
                warn!("Revert commands method not fully implemented for Cisco, using restore backup instead");

                // Fallback to restore backup
                client
                    .execute_command("configure replace nvram:startup-config")
                    .await
                    .map_err(|e| VendorError::CommandError(format!("Revert failed: {}", e)))?;

                info!("Configuration reverted on {}", self.config.hostname());
            }
        }

        Ok(())
    }
}

// Additional helper methods for CiscoSwitch
impl CiscoSwitch {
    /// Reset ports to default state (shutdown, VLAN 1, access mode, no description)
    async fn reset_ports(&mut self, port_ids: &[String]) -> Result<ConfigResult, VendorError> {
        let mut commands = vec!["configure terminal".to_string()];

        for port_id in port_ids {
            let interface = self.normalize_port_id(port_id);
            debug!("  Resetting port {} to default state", port_id);

            commands.push(format!("interface {}", interface));
            commands.push("shutdown".to_string());  // Disable the port
            commands.push("no description".to_string());  // Remove description
            commands.push("switchport mode access".to_string());  // Set to access mode
            commands.push("switchport access vlan 1".to_string());  // Set to default VLAN
            commands.push("no switchport trunk allowed vlan".to_string());  // Remove trunk config
            commands.push("power inline auto".to_string());  // Set PoE to default
            commands.push("exit".to_string());
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

    /// Configure management VLAN on Cisco switch
    /// This creates an SVI (Switched Virtual Interface) for the management VLAN
    /// Note: Cisco management is typically configured with a static IP and default gateway
    async fn configure_management_vlan(&mut self, vlan_id: u16) -> Result<ConfigResult, VendorError> {
        info!("Configuring Cisco management VLAN: {}", vlan_id);

        // Find the VLAN configuration to get IP settings
        let vlan_config = self.config.vlans.iter()
            .find(|v| v.id == vlan_id)
            .ok_or_else(|| VendorError::ValidationError(
                format!("Management VLAN {} not found in VLAN configuration", vlan_id)
            ))?;

        let mut commands = vec![
            "configure terminal".to_string(),
            format!("interface vlan {}", vlan_id),
        ];

        // Configure IP address based on VLAN IP configuration
        match &vlan_config.ip_config {
            VlanIpConfig::Static { address, netmask } => {
                commands.push(format!("ip address {} {}", address, netmask));
                info!("  Configured static IP: {} {}", address, netmask);
            }
            VlanIpConfig::Dhcp => {
                commands.push("ip address dhcp".to_string());
                info!("  Configured DHCP for management VLAN");
            }
            VlanIpConfig::None => {
                warn!("  Management VLAN {} has no IP configuration - switch may not be reachable", vlan_id);
            }
        }

        commands.push("no shutdown".to_string());
        commands.push("exit".to_string());

        // Note: Default gateway is typically configured separately with "ip default-gateway <ip>"
        // or "ip route 0.0.0.0 0.0.0.0 <gateway>" depending on routing mode
        // For now, we only configure the SVI interface

        commands.push("end".to_string());

        let client = self.client.as_mut()
            .ok_or_else(|| VendorError::SshError("Not connected".to_string()))?;

        client.execute_commands(&commands).await
            .map_err(|e| VendorError::CommandError(e.to_string()))?;

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: format!("Configured management VLAN {} with SVI", vlan_id),
            commands_executed: commands,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Remove management VLAN configuration on Cisco switch
    /// This shuts down and removes the SVI interface
    async fn remove_management_vlan(&mut self) -> Result<ConfigResult, VendorError> {
        info!("Removing Cisco management VLAN configuration");

        // Since we don't know which VLAN was the management VLAN without parsing state,
        // we can't remove a specific SVI. For now, return a warning.
        // A proper implementation would parse the current state first.

        warn!("Cisco management VLAN removal requires knowing which VLAN to remove");
        warn!("This operation should be implemented after state parsing is complete");

        Ok(ConfigResult {
            switch: self.config.hostname().to_string(),
            success: true,
            message: "Management VLAN removal not fully implemented for Cisco".to_string(),
            commands_executed: vec![],
            timestamp: chrono::Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConnectionType, Credentials, SnmpAccess, SnmpCommunity, SnmpConfig, TrapType, SnmpTrapReceiver, SpeedDuplex, SwitchModel, VlanIpConfig};
    use std::collections::HashMap;

    fn create_test_cisco_config() -> SwitchConfig {
        SwitchConfig {
            id: "test-cisco-01".to_string(),
            hostname: Some("cisco-c9300-24u-a".to_string()),
            model: Some(SwitchModel::CiscoCatalyst9300_24P_UPOE),
            management_ip: Some("192.168.1.100".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("admin".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Serial,
                serial_device: Some("/dev/serial_cisco_c9300-24u-a".to_string()),
                baud_rate: 9600,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            validation: None,
            vendor_specific: HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        }
    }

    fn create_test_cisco() -> CiscoSwitch {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        CiscoSwitch::new(config, runtime_config, false)
    }

    // ========== VLAN Command Generation Tests ==========
    // Based on hardware Test 2: VLAN Creation

    #[test]
    fn test_generate_vlan_commands_single() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let vlans = vec![
            Vlan {
                id: 100,
                name: "test-vlan-100".to_string(),
                description: Some("Test VLAN".to_string()),
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = cisco.generate_vlan_commands(&vlans);

        assert_eq!(commands[0], "configure terminal");
        assert_eq!(commands[1], "vlan 100");
        assert_eq!(commands[2], "name test-vlan-100");
        assert_eq!(commands[3], "description Test VLAN");
        assert_eq!(commands[4], "exit");
        assert_eq!(commands[5], "end");
    }

    #[test]
    fn test_generate_vlan_commands_multiple() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let vlans = vec![
            Vlan {
                id: 100,
                name: "servers".to_string(),
                description: Some("VLAN for servers".to_string()),
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 200,
                name: "workstations".to_string(),
                description: Some("VLAN for workstations".to_string()),
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 300,
                name: "guests".to_string(),
                description: Some("VLAN for guests".to_string()),
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = cisco.generate_vlan_commands(&vlans);

        // Verify structure
        assert_eq!(commands[0], "configure terminal");
        assert!(commands.contains(&"vlan 100".to_string()));
        assert!(commands.contains(&"name servers".to_string()));
        assert!(commands.contains(&"vlan 200".to_string()));
        assert!(commands.contains(&"name workstations".to_string()));
        assert!(commands.contains(&"vlan 300".to_string()));
        assert!(commands.contains(&"name guests".to_string()));
        assert_eq!(commands.last().unwrap(), "end");
    }

    #[test]
    fn test_generate_vlan_commands_without_description() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let vlans = vec![
            Vlan {
                id: 100,
                name: "test-vlan-100".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        let commands = cisco.generate_vlan_commands(&vlans);

        // Should not contain description command
        assert!(!commands.iter().any(|c| c.starts_with("description")));
        assert_eq!(commands[0], "configure terminal");
        assert_eq!(commands[1], "vlan 100");
        assert_eq!(commands[2], "name test-vlan-100");
        assert_eq!(commands[3], "exit");
        assert_eq!(commands[4], "end");
    }

    // ========== Port Command Generation Tests ==========
    // Based on hardware Test 3: Port Configuration

    #[test]
    fn test_generate_port_commands_access_mode() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: Some("Test port".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert_eq!(commands[0], "configure terminal");
        assert_eq!(commands[1], "interface GigabitEthernet1/0/1");
        assert_eq!(commands[2], "description Test port");
        assert_eq!(commands[3], "switchport mode access");
        assert_eq!(commands[4], "switchport access vlan 100");
        assert_eq!(commands[5], "no shutdown");
        assert_eq!(commands[6], "power inline auto");
        assert!(commands.contains(&"no snmp trap mac-notification change added".to_string()));
        assert!(commands.contains(&"no snmp trap mac-notification change removed".to_string()));
        assert!(commands.contains(&"speed auto".to_string()));
        assert!(commands.contains(&"duplex auto".to_string()));
        assert_eq!(commands.last().unwrap(), "end");
    }

    #[test]
    fn test_generate_port_commands_trunk_mode() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                allowed_vlans: vec![10, 20, 30],
                description: Some("Trunk port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert!(commands.contains(&"interface GigabitEthernet1/0/24".to_string()));
        assert!(commands.contains(&"switchport mode trunk".to_string()));
        assert!(commands.contains(&"switchport trunk native vlan 1".to_string()));
        assert!(commands.contains(&"switchport trunk allowed vlan 10,20,30".to_string()));
        assert!(commands.contains(&"power inline never".to_string()));
    }

    #[test]
    fn test_generate_port_commands_poe_disabled() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/3".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: Some("PoE disabled port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert!(commands.contains(&"power inline never".to_string()));
        assert!(!commands.contains(&"power inline auto".to_string()));
    }

    #[test]
    fn test_generate_port_commands_mac_notify_enabled() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: true,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert!(commands.contains(&"snmp trap mac-notification change added".to_string()));
        assert!(commands.contains(&"snmp trap mac-notification change removed".to_string()));
        assert!(!commands.iter().any(|c| c.starts_with("no snmp trap")));
    }

    #[test]
    fn test_generate_port_commands_speed_duplex() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert!(commands.contains(&"speed 100".to_string()));
        assert!(commands.contains(&"duplex full".to_string()));
    }

    #[test]
    fn test_generate_port_commands_disabled_port() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: None,
                enabled: false,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        assert!(commands.contains(&"shutdown".to_string()));
        assert!(!commands.contains(&"no shutdown".to_string()));
    }

    // ========== Port ID Normalization Tests ==========

    #[test]
    fn test_normalize_port_id_gi_short_form() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        assert_eq!(cisco.normalize_port_id("Gi1/0/1"), "GigabitEthernet1/0/1");
        assert_eq!(cisco.normalize_port_id("Gi1/0/24"), "GigabitEthernet1/0/24");
    }

    #[test]
    fn test_normalize_port_id_already_full() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        assert_eq!(cisco.normalize_port_id("GigabitEthernet1/0/1"), "GigabitEthernet1/0/1");
    }

    #[test]
    fn test_normalize_port_id_te_short_form() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        assert_eq!(cisco.normalize_port_id("Te1/0/1"), "TenGigabitEthernet1/0/1");
    }

    // ========== Port Mirror Command Generation Tests ==========

    #[test]
    fn test_generate_mirror_commands() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["GigabitEthernet1/0/1".to_string(), "GigabitEthernet1/0/2".to_string()],
                destination_port: "GigabitEthernet1/0/10".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let commands = cisco.generate_mirror_commands(&mirrors);

        assert!(commands.contains(&"monitor session 1 source interface GigabitEthernet1/0/1 both".to_string()));
        assert!(commands.contains(&"monitor session 1 source interface GigabitEthernet1/0/2 both".to_string()));
        assert!(commands.contains(&"monitor session 1 destination interface GigabitEthernet1/0/10".to_string()));
    }

    #[test]
    fn test_generate_mirror_commands_rx_direction() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["GigabitEthernet1/0/1".to_string()],
                destination_port: "GigabitEthernet1/0/10".to_string(),
                direction: MirrorDirection::Rx,
            },
        ];

        let commands = cisco.generate_mirror_commands(&mirrors);

        assert!(commands.contains(&"monitor session 1 source interface GigabitEthernet1/0/1 rx".to_string()));
    }

    #[test]
    fn test_generate_mirror_commands_tx_direction() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["GigabitEthernet1/0/1".to_string()],
                destination_port: "GigabitEthernet1/0/10".to_string(),
                direction: MirrorDirection::Tx,
            },
        ];

        let commands = cisco.generate_mirror_commands(&mirrors);

        assert!(commands.contains(&"monitor session 1 source interface GigabitEthernet1/0/1 tx".to_string()));
    }

    // ========== SNMP Command Generation Tests ==========

    #[test]
    fn test_generate_snmp_commands_basic() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let snmp_config = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![],
            enabled_traps: vec![],
        };

        let commands = cisco.generate_snmp_commands(&snmp_config);

        assert!(commands.contains(&"snmp-server community public RO".to_string()));
    }

    #[test]
    fn test_generate_snmp_commands_with_traps() {
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let snmp_config = SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.10".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
            ],
            enabled_traps: vec![
                TrapType::LinkChange,
                TrapType::MacNotify,
            ],
        };

        let commands = cisco.generate_snmp_commands(&snmp_config);

        assert!(commands.contains(&"snmp-server host 192.168.1.10 version 2c public".to_string()));
        assert!(commands.contains(&"snmp-server enable traps link".to_string()));
        assert!(commands.contains(&"snmp-server enable traps mac-notification".to_string()));
    }

    // ========== Validation Tests ==========
    // Based on hardware discovery: VLAN 1 must be explicitly defined for trunk ports

    #[test]
    fn test_validate_configuration_success() {
        let mut config = create_test_cisco_config();

        // Add VLAN 1 and VLAN 100
        config.vlans = vec![
            Vlan {
                id: 1,
                name: "default".to_string(),
                description: Some("Default VLAN".to_string()),
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 100,
                name: "test".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        // Add port in VLAN 100
        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let result = cisco.validate_configuration();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_configuration_missing_vlan() {
        let mut config = create_test_cisco_config();

        // Port references VLAN 100, but VLAN 100 not defined
        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 100,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let result = cisco.validate_configuration();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_configuration_trunk_without_vlan_1() {
        let mut config = create_test_cisco_config();

        // Trunk port with native VLAN 1, but VLAN 1 not defined
        // This was the issue discovered in tests 6-10
        config.vlans = vec![
            Vlan {
                id: 100,
                name: "test".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,  // Native VLAN 1
                allowed_vlans: vec![100],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let result = cisco.validate_configuration();
        assert!(result.is_err());

        let error_message = format!("{:?}", result.unwrap_err());
        assert!(error_message.contains("VLAN 1"));
    }

    #[test]
    fn test_validate_configuration_trunk_with_vlan_1() {
        let mut config = create_test_cisco_config();

        // Properly configured: VLAN 1 explicitly defined
        config.vlans = vec![
            Vlan {
                id: 1,
                name: "default".to_string(),
                description: Some("Default VLAN".to_string()),
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 100,
                name: "test".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                allowed_vlans: vec![100],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let result = cisco.validate_configuration();
        assert!(result.is_ok());
    }

    // ========== Complex Scenario Tests ==========
    // Based on hardware Test 10: Complex Production Scenario

    #[test]
    fn test_complex_production_scenario_commands() {
        let mut config = create_test_cisco_config();

        // Complex scenario: multiple VLANs, mixed ports, PoE variations
        config.vlans = vec![
            Vlan { id: 1, name: "default".to_string(), description: Some("Default VLAN".to_string()), ip_config: VlanIpConfig::None },
            Vlan { id: 10, name: "management".to_string(), description: Some("Management VLAN".to_string()), ip_config: VlanIpConfig::None },
            Vlan { id: 20, name: "production".to_string(), description: Some("Production VLAN".to_string()), ip_config: VlanIpConfig::None },
            Vlan { id: 30, name: "guest".to_string(), description: Some("Guest VLAN".to_string()), ip_config: VlanIpConfig::None },
        ];

        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("Management port with PoE".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "GigabitEthernet1/0/2".to_string(),
                mode: PortMode::Access,
                vlan: 20,
                allowed_vlans: vec![],
                description: Some("Production port with PoE".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "GigabitEthernet1/0/3".to_string(),
                mode: PortMode::Access,
                vlan: 30,
                allowed_vlans: vec![],
                description: Some("Guest port without PoE".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "GigabitEthernet1/0/24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                allowed_vlans: vec![10, 20, 30],
                description: Some("Uplink trunk port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        // Test VLAN commands
        let vlan_commands = cisco.generate_vlan_commands(&cisco.config.vlans);
        assert!(vlan_commands.contains(&"vlan 1".to_string()));
        assert!(vlan_commands.contains(&"vlan 10".to_string()));
        assert!(vlan_commands.contains(&"vlan 20".to_string()));
        assert!(vlan_commands.contains(&"vlan 30".to_string()));

        // Test port commands
        let port_commands = cisco.generate_port_commands(&cisco.config.ports);
        assert!(port_commands.contains(&"interface GigabitEthernet1/0/1".to_string()));
        assert!(port_commands.contains(&"switchport access vlan 10".to_string()));
        assert!(port_commands.contains(&"interface GigabitEthernet1/0/24".to_string()));
        assert!(port_commands.contains(&"switchport mode trunk".to_string()));
        assert!(port_commands.contains(&"switchport trunk allowed vlan 10,20,30".to_string()));

        // Validate configuration
        assert!(cisco.validate_configuration().is_ok());
    }

    // ========== Regression Tests ==========
    // Ensure fixes from hardware testing remain in place

    #[test]
    fn test_regression_vlan_1_trunk_validation() {
        // Regression test for Tests 6-10 VLAN 1 issue
        let mut config = create_test_cisco_config();

        config.vlans = vec![
            Vlan { id: 300, name: "test-vlan-300".to_string(), description: None, ip_config: VlanIpConfig::None },
            Vlan { id: 400, name: "test-vlan-400".to_string(), description: None, ip_config: VlanIpConfig::None },
            Vlan { id: 500, name: "test-vlan-500".to_string(), description: None, ip_config: VlanIpConfig::None },
        ];

        config.ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,  // References VLAN 1 but VLAN 1 not defined
                allowed_vlans: vec![300, 400, 500],
                description: Some("Trunk port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        // Should fail validation - this was the bug discovered in hardware testing
        let result = cisco.validate_configuration();
        assert!(result.is_err(), "Should fail validation when VLAN 1 is referenced but not defined");
    }

    #[test]
    fn test_poe_command_generation() {
        // Regression test for Test 9: PoE Configuration
        let config = create_test_cisco_config();
        let runtime_config = RuntimeConfig::default();
        let cisco = CiscoSwitch::new(config, runtime_config, false);

        let ports = vec![
            Port {
                port_id: "GigabitEthernet1/0/1".to_string(),
                mode: PortMode::Access,
                vlan: 500,
                allowed_vlans: vec![],
                description: Some("PoE enabled port 1".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
            Port {
                port_id: "GigabitEthernet1/0/3".to_string(),
                mode: PortMode::Access,
                vlan: 500,
                allowed_vlans: vec![],
                description: Some("PoE disabled port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let commands = cisco.generate_port_commands(&ports);

        // Verify PoE commands are correct
        let gi1_idx = commands.iter().position(|c| c == "interface GigabitEthernet1/0/1").unwrap();
        let gi3_idx = commands.iter().position(|c| c == "interface GigabitEthernet1/0/3").unwrap();

        // Check that power inline auto appears after Gi1/0/1
        let auto_idx = commands[gi1_idx..].iter().position(|c| c == "power inline auto").unwrap();
        assert!(auto_idx > 0);

        // Check that power inline never appears after Gi1/0/3
        let never_idx = commands[gi3_idx..].iter().position(|c| c == "power inline never").unwrap();
        assert!(never_idx > 0);
    }

    #[test]
    fn test_management_vlan_diff_add() {
        use crate::diff::compute_diff;

        let cisco = create_test_cisco_config();

        // Current state: no management VLAN
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        // Desired config: add management VLAN 88
        let mut desired_config = cisco.clone();
        desired_config.management_vlan = Some(88);
        desired_config.vlans.push(Vlan {
            id: 88,
            name: "management".to_string(),
            description: Some("Management SVI".to_string()),
            ip_config: VlanIpConfig::Static {
                address: "192.168.88.1".to_string(),
                netmask: "255.255.255.0".to_string(),
            },
        });

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being added");
        assert_eq!(diff.management_vlan, Some(88), "Should show new management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_change() {
        use crate::diff::compute_diff;

        let cisco = create_test_cisco_config();

        // Current state: management VLAN 10
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(10),
            warnings: vec![],
        };

        // Desired config: change to management VLAN 99
        let mut desired_config = cisco.clone();
        desired_config.management_vlan = Some(99);
        desired_config.vlans.push(Vlan {
            id: 99,
            name: "mgmt".to_string(),
            description: None,
            ip_config: VlanIpConfig::Dhcp,
        });

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN change");
        assert_eq!(diff.management_vlan, Some(99), "Should show new management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_remove() {
        use crate::diff::compute_diff;

        let cisco = create_test_cisco_config();

        // Current state: management VLAN 50
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(50),
            warnings: vec![],
        };

        // Desired config: remove management VLAN
        let desired_config = cisco.clone();
        // management_vlan defaults to None

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(diff.management_vlan_changed, "Should detect management VLAN being removed");
        assert_eq!(diff.management_vlan, None, "Should show no management VLAN");
    }

    #[test]
    fn test_management_vlan_diff_no_change() {
        use crate::diff::compute_diff;

        let cisco = create_test_cisco_config();

        // Current state: management VLAN 20
        let current_state = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: Some(20),
            warnings: vec![],
        };

        // Desired config: same management VLAN 20
        let mut desired_config = cisco.clone();
        desired_config.management_vlan = Some(20);

        let diff = compute_diff(&current_state, &desired_config, false);

        assert!(!diff.management_vlan_changed, "Should not detect change when management VLAN is same");
    }

    #[test]
    fn test_parse_management_vlan_static_ip() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface Vlan88",
            " ip address 192.168.88.1 255.255.255.0",
            " no shutdown",
            "!",
            "interface GigabitEthernet1/0/1",
            " switchport mode access",
            "!",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, Some(88), "Should parse SVI with static IP as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_dhcp() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface Vlan77",
            " ip address dhcp",
            " no shutdown",
            "!",
            "interface GigabitEthernet1/0/1",
            " switchport mode access",
            "!",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, Some(77), "Should parse SVI with DHCP as management VLAN");
    }

    #[test]
    fn test_parse_management_vlan_multiple_svis() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface Vlan10",
            " description Data VLAN",
            "!",
            "interface Vlan88",
            " ip address 192.168.88.1 255.255.255.0",
            " no shutdown",
            "!",
            "interface Vlan99",
            " ip address 192.168.99.1 255.255.255.0",
            "!",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, Some(88), "Should return first SVI with IP address");
    }

    #[test]
    fn test_parse_management_vlan_no_ip() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface Vlan10",
            " description Management VLAN",
            " no shutdown",
            "!",
            "interface GigabitEthernet1/0/1",
            " switchport mode access",
            "!",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, None, "Should return None when no SVI has IP address");
    }

    #[test]
    fn test_parse_management_vlan_none() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface GigabitEthernet1/0/1",
            " switchport mode access",
            "!",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, None, "Should return None when no SVIs exist");
    }

    #[test]
    fn test_parse_management_vlan_at_end() {
        let cisco = create_test_cisco();

        let running_config = vec![
            "!",
            "interface GigabitEthernet1/0/1",
            " switchport mode access",
            "!",
            "interface Vlan88",
            " ip address 192.168.88.1 255.255.255.0",
            " no shutdown",
        ];

        let result = cisco.parse_management_vlan(&running_config);
        assert_eq!(result, Some(88), "Should parse SVI at end of config");
    }
}
