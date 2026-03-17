use crate::models::{Port, PortMirror, PortMode, StateDiff, SwitchConfig, SwitchState, Vlan};
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// Represents a port that needs to be migrated before VLAN deletion
#[derive(Debug, Clone, PartialEq)]
pub struct PortMigration {
    /// Port identifier
    pub port_id: String,
    /// The VLAN being removed
    pub vlan_being_removed: u16,
    /// Migration action needed
    pub action: PortMigrationAction,
}

/// Type of migration action needed for a port
#[derive(Debug, Clone, PartialEq)]
pub enum PortMigrationAction {
    /// Move untagged (access) port to VLAN 1
    MoveAccessToVlan1,
    /// Remove VLAN from trunk port's allowed_vlans list
    RemoveVlanFromTrunk { remaining_vlans: Vec<u16> },
}

/// Compute the difference between current and desired state
/// If enforce_port_config is true, ports not in the desired config will be reset
pub fn compute_diff(current: &SwitchState, desired: &SwitchConfig, enforce_port_config: bool) -> StateDiff {
    let mut diff = StateDiff::default();

    // Compute VLAN differences
    diff_vlans(current, desired, &mut diff);

    // Compute port differences
    diff_ports(current, desired, enforce_port_config, &mut diff);

    // Compute port mirror differences
    diff_mirrors(current, desired, &mut diff);

    // Compute SNMP configuration differences
    diff_snmp(current, desired, &mut diff);

    // Compute management VLAN differences
    diff_management_vlan(current, desired, &mut diff);

    if diff.has_changes() {
        debug!("State differences detected:");
        debug!("  VLANs to add: {}", diff.vlans_to_add.len());
        debug!("  VLANs to remove: {}", diff.vlans_to_remove.len());
        debug!("  VLANs to update: {}", diff.vlans_to_update.len());
        debug!("  Ports to configure: {}", diff.ports_to_configure.len());
        debug!("  Ports to reset: {}", diff.ports_to_reset.len());
        debug!("  Mirrors to add: {}", diff.mirrors_to_add.len());
        debug!("  Mirrors to remove: {}", diff.mirrors_to_remove.len());
        debug!("  Mirrors to update: {}", diff.mirrors_to_update.len());
        debug!("  SNMP config changed: {}", diff.snmp_config_changed);
        debug!("  Management VLAN changed: {}", diff.management_vlan_changed);
    } else {
        debug!("No state differences detected - switch is already in desired state");
    }

    diff
}

fn diff_vlans(current: &SwitchState, desired: &SwitchConfig, diff: &mut StateDiff) {
    let current_vlans: HashMap<u16, &Vlan> = current
        .vlans
        .iter()
        .map(|v| (v.id, v))
        .collect();

    let desired_vlans: HashMap<u16, &Vlan> = desired
        .vlans
        .iter()
        .map(|v| (v.id, v))
        .collect();

    let current_ids: HashSet<u16> = current_vlans.keys().copied().collect();
    let desired_ids: HashSet<u16> = desired_vlans.keys().copied().collect();

    // Check if the switch model supports VLAN descriptions
    let supports_vlan_description = desired.model.as_ref().map_or(true, |m| m.supports_vlan_description());

    // VLANs to add (in desired but not in current)
    for vlan_id in desired_ids.difference(&current_ids) {
        if let Some(vlan) = desired_vlans.get(vlan_id) {
            debug!("  VLAN {} ({}) to add: not in current state", vlan_id, vlan.name);
            diff.vlans_to_add.push((*vlan).clone());
        }
    }

    // VLANs to remove (in current but not in desired)
    for vlan_id in current_ids.difference(&desired_ids) {
        if let Some(vlan) = current_vlans.get(vlan_id) {
            debug!("  VLAN {} ({}) to remove: not in desired config", vlan_id, vlan.name);
        }
        diff.vlans_to_remove.push(*vlan_id);
    }

    // VLANs to update (in both but with different properties)
    for vlan_id in current_ids.intersection(&desired_ids) {
        if let (Some(current_vlan), Some(desired_vlan)) =
            (current_vlans.get(vlan_id), desired_vlans.get(vlan_id)) {
            if !vlans_equivalent_for_model(current_vlan, desired_vlan, supports_vlan_description) {
                debug!("  VLAN {} to update: current={:?}, desired={:?}", vlan_id, current_vlan, desired_vlan);
                diff.vlans_to_update.push((*desired_vlan).clone());
            }
        }
    }
}

/// Check if two VLANs are functionally equivalent, with model awareness for VLAN descriptions
/// When supports_vlan_description is false (e.g., Aruba switches), description differences are ignored
fn vlans_equivalent_for_model(current: &Vlan, desired: &Vlan, supports_vlan_description: bool) -> bool {
    // ID and name must always match
    if current.id != desired.id || current.name != desired.name {
        return false;
    }

    // IP config must always match
    if current.ip_config != desired.ip_config {
        return false;
    }

    // Only compare description if the switch supports VLAN descriptions
    // For switches like Aruba that don't support descriptions, ignore any differences
    if supports_vlan_description && current.description != desired.description {
        return false;
    }

    true
}

fn diff_ports(current: &SwitchState, desired: &SwitchConfig, enforce_port_config: bool, diff: &mut StateDiff) {
    let current_ports: HashMap<String, &Port> = current
        .ports
        .iter()
        .map(|p| (p.port_id.clone(), p))
        .collect();

    let desired_ports: HashMap<String, &Port> = desired
        .ports
        .iter()
        .map(|p| (p.port_id.clone(), p))
        .collect();

    // Check if the switch model supports PoE
    let supports_poe = desired.model.as_ref().map_or(true, |m| m.supports_poe());

    // For ports, we configure any port that is in the desired state
    // regardless of whether it exists in current or has different config
    for desired_port in &desired.ports {
        let needs_config = match current_ports.get(&desired_port.port_id) {
            Some(current_port) => {
                let different = !ports_equivalent_for_model(current_port, desired_port, supports_poe);
                if different {
                    debug!("  Port {} to configure:", desired_port.port_id);
                    debug!("    Current: mode={:?}, vlan={}, allowed_vlans={:?}, desc={:?}, enabled={}, poe={}, speed_duplex={:?}",
                        current_port.mode, current_port.vlan, current_port.allowed_vlans,
                        current_port.description, current_port.enabled, current_port.poe_enabled, current_port.speed_duplex);
                    debug!("    Desired: mode={:?}, vlan={}, allowed_vlans={:?}, desc={:?}, enabled={}, poe={}, speed_duplex={:?}",
                        desired_port.mode, desired_port.vlan, desired_port.allowed_vlans,
                        desired_port.description, desired_port.enabled, desired_port.poe_enabled, desired_port.speed_duplex);
                }
                different
            }
            None => {
                debug!("  Port {} to configure: not found in current state", desired_port.port_id);
                true
            }
        };

        if needs_config {
            diff.ports_to_configure.push(desired_port.clone());
        }
    }

    // If enforce_port_config is enabled, identify ports to reset
    // (ports that exist on the switch but are not in the desired config)
    if enforce_port_config {
        let current_port_ids: HashSet<String> = current_ports.keys().cloned().collect();
        let desired_port_ids: HashSet<String> = desired_ports.keys().cloned().collect();

        // Collect mirror destination ports - these should NOT be reset
        let mirror_dest_ports: HashSet<String> = desired
            .port_mirrors
            .iter()
            .map(|m| m.destination_port.clone())
            .collect();

        for port_id in current_port_ids.difference(&desired_port_ids) {
            // Skip mirror destination ports - resetting them would break mirroring
            if mirror_dest_ports.contains(port_id) {
                debug!("  Port {} skipped from reset: mirror destination port", port_id);
                continue;
            }

            // Skip ports that are already in default state - no need to reset them
            if let Some(current_port) = current_ports.get(port_id) {
                if is_port_in_default_state(current_port) {
                    debug!("  Port {} skipped from reset: already in default state", port_id);
                    continue;
                }
            }

            debug!("  Port {} to reset: not in desired config (enforce_port_config=true)", port_id);
            diff.ports_to_reset.push(port_id.clone());
        }
    }
}

/// Check if a port is already in default state (disabled, VLAN 1, access mode, no description/name)
/// This prevents unnecessary reset commands for ports that are already defaults
fn is_port_in_default_state(port: &Port) -> bool {
    // Default state: disabled, VLAN 1, access mode, no description
    // We're lenient - if the port matches these criteria, don't reset it
    let is_disabled = !port.enabled;
    let is_vlan_1 = port.vlan == 1;
    let is_access_mode = matches!(port.mode, PortMode::Access);
    let no_description = port.description.is_none() || port.description.as_ref().map_or(true, |d| d.is_empty());
    let no_allowed_vlans = port.allowed_vlans.is_empty();
    let no_mac_notify = !port.mac_notify;

    // Consider port in default state if ALL default criteria match
    is_disabled && is_vlan_1 && is_access_mode && no_description && no_allowed_vlans && no_mac_notify
}

/// Check if two ports are functionally equivalent
/// This handles cases where Access mode with no tagged VLANs is equivalent to
/// Trunk mode with only the native VLAN in allowed_vlans
fn ports_equivalent(current: &Port, desired: &Port) -> bool {
    // Default: compare all fields including PoE
    ports_equivalent_for_model(current, desired, true)
}

/// Check if two ports are functionally equivalent, with model awareness for PoE
/// When supports_poe is false, poe_enabled differences are ignored since the switch
/// doesn't support PoE and it's irrelevant to compare this field.
fn ports_equivalent_for_model(current: &Port, desired: &Port, supports_poe: bool) -> bool {

    // Basic properties must match
    if current.port_id != desired.port_id
        || current.vlan != desired.vlan
        || current.description != desired.description
        || current.enabled != desired.enabled
        || current.mac_notify != desired.mac_notify
        || current.speed_duplex != desired.speed_duplex
    {
        return false;
    }

    // Only compare poe_enabled if the switch supports PoE
    // For non-PoE switches, ignore any poe_enabled differences
    if supports_poe && current.poe_enabled != desired.poe_enabled {
        return false;
    }

    // Get tagged VLANs (exclude native VLAN from allowed_vlans)
    let current_tagged: Vec<u16> = current.allowed_vlans
        .iter()
        .filter(|&&v| v != current.vlan)
        .copied()
        .collect();

    let desired_tagged: Vec<u16> = desired.allowed_vlans
        .iter()
        .filter(|&&v| v != desired.vlan)
        .copied()
        .collect();

    // If tagged VLANs don't match, ports are different
    if current_tagged != desired_tagged {
        return false;
    }

    // If both have no tagged VLANs, they're equivalent regardless of mode
    // (Access mode and Trunk mode with no tagged VLANs are functionally identical)
    if current_tagged.is_empty() && desired_tagged.is_empty() {
        return true;
    }

    // Otherwise, mode must match
    current.mode == desired.mode
}

fn diff_mirrors(current: &SwitchState, desired: &SwitchConfig, diff: &mut StateDiff) {
    let current_mirrors: HashMap<String, &PortMirror> = current
        .port_mirrors
        .iter()
        .map(|m| (m.session_id.clone(), m))
        .collect();

    let desired_mirrors: HashMap<String, &PortMirror> = desired
        .port_mirrors
        .iter()
        .map(|m| (m.session_id.clone(), m))
        .collect();

    let current_ids: HashSet<String> = current_mirrors.keys().cloned().collect();
    let desired_ids: HashSet<String> = desired_mirrors.keys().cloned().collect();

    // Mirrors to add
    for session_id in desired_ids.difference(&current_ids) {
        if let Some(mirror) = desired_mirrors.get(session_id) {
            debug!("  Mirror session {} to add: not in current state", session_id);
            debug!("    Sources: {:?}, Dest: {}, Dir: {:?}",
                mirror.source_ports, mirror.destination_port, mirror.direction);
            diff.mirrors_to_add.push((*mirror).clone());
        }
    }

    // Mirrors to remove
    for session_id in current_ids.difference(&desired_ids) {
        debug!("  Mirror session {} to remove: not in desired config", session_id);
        diff.mirrors_to_remove.push(session_id.clone());
    }

    // Mirrors to update
    for session_id in current_ids.intersection(&desired_ids) {
        if let (Some(current_mirror), Some(desired_mirror)) =
            (current_mirrors.get(session_id), desired_mirrors.get(session_id)) {
            if current_mirror != desired_mirror {
                debug!("  Mirror session {} to update: current={:?}, desired={:?}",
                    session_id, current_mirror, desired_mirror);
                diff.mirrors_to_update.push((*desired_mirror).clone());
            }
        }
    }
}

fn diff_snmp(current: &SwitchState, desired: &SwitchConfig, diff: &mut StateDiff) {
    use crate::models::SnmpStateDiff;

    let current_snmp = current.snmp.as_ref();
    let desired_snmp = desired.snmp.as_ref();

    match (current_snmp, desired_snmp) {
        (None, None) => {
            // No SNMP config in either - no change
            diff.snmp_config_changed = false;
            diff.snmp_diff = None;
        }
        (None, Some(desired_config)) => {
            // New SNMP config to apply - add everything
            debug!("  SNMP config to add: not in current state");
            debug!("    Communities to add: {}", desired_config.communities.len());
            debug!("    Trap receivers to add: {}", desired_config.trap_receivers.len());
            debug!("    Traps to enable: {:?}", desired_config.enabled_traps);

            let snmp_diff = SnmpStateDiff {
                communities_to_add: desired_config.communities.clone(),
                communities_to_remove: vec![],
                communities_to_update: vec![],
                trap_receivers_to_add: desired_config.trap_receivers.clone(),
                trap_receivers_to_remove: vec![],
                traps_to_enable: desired_config.enabled_traps.clone(),
                traps_to_disable: vec![],
            };

            diff.snmp_config_changed = snmp_diff.has_changes();
            diff.snmp_diff = Some(snmp_diff);
            diff.snmp_config = Some(desired_config.clone());
        }
        (Some(current_config), None) => {
            // SNMP config to remove - remove everything
            debug!("  SNMP config to remove: not in desired state");

            let snmp_diff = SnmpStateDiff {
                communities_to_add: vec![],
                communities_to_remove: current_config.communities.iter().map(|c| c.name.clone()).collect(),
                communities_to_update: vec![],
                trap_receivers_to_add: vec![],
                trap_receivers_to_remove: current_config.trap_receivers.iter().map(|r| r.host.clone()).collect(),
                traps_to_enable: vec![],
                traps_to_disable: current_config.enabled_traps.clone(),
            };

            diff.snmp_config_changed = snmp_diff.has_changes();
            diff.snmp_diff = Some(snmp_diff);
            diff.snmp_config = None;
        }
        (Some(current_config), Some(desired_config)) => {
            // Compute granular diff between current and desired
            let snmp_diff = compute_snmp_diff(current_config, desired_config);

            if snmp_diff.has_changes() {
                debug!("  SNMP granular diff:");
                debug!("    Communities to add: {:?}", snmp_diff.communities_to_add.iter().map(|c| &c.name).collect::<Vec<_>>());
                debug!("    Communities to remove: {:?}", snmp_diff.communities_to_remove);
                debug!("    Communities to update: {:?}", snmp_diff.communities_to_update.iter().map(|c| &c.name).collect::<Vec<_>>());
                debug!("    Trap receivers to add: {:?}", snmp_diff.trap_receivers_to_add.iter().map(|r| &r.host).collect::<Vec<_>>());
                debug!("    Trap receivers to remove: {:?}", snmp_diff.trap_receivers_to_remove);
                debug!("    Traps to enable: {:?}", snmp_diff.traps_to_enable);
                debug!("    Traps to disable: {:?}", snmp_diff.traps_to_disable);
                diff.snmp_config_changed = true;
            } else {
                debug!("  SNMP config unchanged (granular comparison)");
                diff.snmp_config_changed = false;
            }

            diff.snmp_diff = Some(snmp_diff);
            diff.snmp_config = Some(desired_config.clone());
        }
    }
}

/// Compute granular diff between two SNMP configurations
fn compute_snmp_diff(
    current: &crate::models::SnmpConfig,
    desired: &crate::models::SnmpConfig,
) -> crate::models::SnmpStateDiff {
    use crate::models::{SnmpStateDiff, TrapType};

    let mut snmp_diff = SnmpStateDiff::default();

    // Build maps for efficient lookup
    let current_communities: HashMap<&str, &crate::models::SnmpCommunity> = current
        .communities
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    let desired_communities: HashMap<&str, &crate::models::SnmpCommunity> = desired
        .communities
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    // Communities to add (in desired but not in current)
    for (name, community) in &desired_communities {
        if !current_communities.contains_key(name) {
            snmp_diff.communities_to_add.push((*community).clone());
        }
    }

    // Communities to remove (in current but not in desired)
    for name in current_communities.keys() {
        if !desired_communities.contains_key(name) {
            snmp_diff.communities_to_remove.push((*name).to_string());
        }
    }

    // Communities to update (same name but different access level)
    for (name, desired_community) in &desired_communities {
        if let Some(current_community) = current_communities.get(name) {
            if current_community.access != desired_community.access {
                snmp_diff.communities_to_update.push((*desired_community).clone());
            }
        }
    }

    // Compare trap receivers by host (the primary identifier)
    let current_receivers: HashMap<&str, &crate::models::SnmpTrapReceiver> = current
        .trap_receivers
        .iter()
        .map(|r| (r.host.as_str(), r))
        .collect();

    let desired_receivers: HashMap<&str, &crate::models::SnmpTrapReceiver> = desired
        .trap_receivers
        .iter()
        .map(|r| (r.host.as_str(), r))
        .collect();

    // Trap receivers to add (in desired but not in current)
    for (host, receiver) in &desired_receivers {
        if !current_receivers.contains_key(host) {
            snmp_diff.trap_receivers_to_add.push((*receiver).clone());
        }
    }

    // Trap receivers to remove (in current but not in desired)
    for host in current_receivers.keys() {
        if !desired_receivers.contains_key(host) {
            snmp_diff.trap_receivers_to_remove.push((*host).to_string());
        }
    }

    // Note: We don't track "trap receivers to update" because the host is the key
    // and if the community changes, we'd need to remove and re-add anyway.
    // For simplicity, if a receiver exists with same host, we consider it "same"
    // since the community is typically the same and version doesn't matter much.

    // Compare enabled traps
    let current_traps: HashSet<&TrapType> = current.enabled_traps.iter().collect();
    let desired_traps: HashSet<&TrapType> = desired.enabled_traps.iter().collect();

    // Traps to enable (in desired but not in current)
    for trap in &desired_traps {
        if !current_traps.contains(trap) {
            snmp_diff.traps_to_enable.push((*trap).clone());
        }
    }

    // Traps to disable (in current but not in desired)
    for trap in &current_traps {
        if !desired_traps.contains(trap) {
            snmp_diff.traps_to_disable.push((*trap).clone());
        }
    }

    snmp_diff
}

fn diff_management_vlan(current: &SwitchState, desired: &SwitchConfig, diff: &mut StateDiff) {
    // Check if management VLAN configuration has changed
    let current_mgmt_vlan = current.management_vlan;
    let desired_mgmt_vlan = desired.management_vlan;

    if current_mgmt_vlan != desired_mgmt_vlan {
        debug!("  Management VLAN changed: current={:?}, desired={:?}", current_mgmt_vlan, desired_mgmt_vlan);
        diff.management_vlan_changed = true;
        diff.management_vlan = desired_mgmt_vlan;
    } else {
        diff.management_vlan_changed = false;
    }
}

/// Find all ports that need to be migrated before VLANs can be deleted
/// This prevents interactive confirmation prompts when deleting VLANs with assigned ports
///
/// Returns a vector of PortMigration objects describing what needs to be done for each port
pub fn find_ports_to_migrate(current_state: &SwitchState, vlans_to_remove: &[u16]) -> Vec<PortMigration> {
    let mut migrations = Vec::new();

    // Skip if no VLANs to remove
    if vlans_to_remove.is_empty() {
        return migrations;
    }

    // Create a set for efficient lookup
    let vlan_set: HashSet<u16> = vlans_to_remove.iter().copied().collect();

    // Check each port in current state
    for port in &current_state.ports {
        for &vlan_id in vlans_to_remove {
            match port.mode {
                PortMode::Access => {
                    // If port's access VLAN is being removed, move to VLAN 1
                    if port.vlan == vlan_id {
                        debug!(
                            "Port {} needs migration: access port on VLAN {} which is being removed",
                            port.port_id, vlan_id
                        );
                        migrations.push(PortMigration {
                            port_id: port.port_id.clone(),
                            vlan_being_removed: vlan_id,
                            action: PortMigrationAction::MoveAccessToVlan1,
                        });
                        break; // Each port only needs one migration entry
                    }
                }
                PortMode::Trunk => {
                    // If VLAN is in the allowed_vlans list, remove it
                    if port.allowed_vlans.contains(&vlan_id) {
                        // Calculate remaining VLANs after removing the one being deleted
                        let remaining_vlans: Vec<u16> = port
                            .allowed_vlans
                            .iter()
                            .filter(|&&v| !vlan_set.contains(&v))
                            .copied()
                            .collect();

                        debug!(
                            "Port {} needs migration: trunk port has VLAN {} which is being removed (remaining: {:?})",
                            port.port_id, vlan_id, remaining_vlans
                        );

                        migrations.push(PortMigration {
                            port_id: port.port_id.clone(),
                            vlan_being_removed: vlan_id,
                            action: PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans },
                        });
                        break; // Each port only needs one migration entry
                    }
                }
            }
        }
    }

    if !migrations.is_empty() {
        debug!(
            "Found {} ports that need migration before VLAN deletion",
            migrations.len()
        );
    }

    migrations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ConnectionType, Credentials, Port, PortMirror, PortMode, MirrorDirection, SnmpConfig,
        SnmpCommunity, SnmpTrapReceiver, TrapType, SnmpAccess, SwitchModel, Vendor, VlanIpConfig,
        SpeedDuplex,
    };

    fn create_test_switch_config() -> SwitchConfig {
        SwitchConfig {
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
        }
    }

    #[test]
    fn test_compute_diff_no_changes() {
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "vlan10".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(!diff.has_changes());
    }

    #[test]
    fn test_compute_diff_vlan_additions() {
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "vlan10".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
            Vlan {
                id: 20,
                name: "vlan20".to_string(),
                description: Some("New VLAN".to_string()),
                ip_config: VlanIpConfig::Dhcp,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert_eq!(diff.vlans_to_add.len(), 1);
        assert_eq!(diff.vlans_to_add[0].id, 20);
        assert_eq!(diff.vlans_to_remove.len(), 0);
        assert_eq!(diff.vlans_to_update.len(), 0);
    }

    #[test]
    fn test_compute_diff_vlan_removals() {
        let current = SwitchState {
            vlans: vec![
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
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert_eq!(diff.vlans_to_add.len(), 0);
        assert_eq!(diff.vlans_to_remove.len(), 1);
        assert_eq!(diff.vlans_to_remove[0], 20);
        assert_eq!(diff.vlans_to_update.len(), 0);
    }

    #[test]
    fn test_compute_diff_vlan_updates() {
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "vlan10".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "vlan10-updated".to_string(),
                description: Some("Updated VLAN".to_string()),
                ip_config: VlanIpConfig::Dhcp,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert_eq!(diff.vlans_to_add.len(), 0);
        assert_eq!(diff.vlans_to_remove.len(), 0);
        assert_eq!(diff.vlans_to_update.len(), 1);
        assert_eq!(diff.vlans_to_update[0].name, "vlan10-updated");
    }

    #[test]
    fn test_compute_diff_port_changes() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
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
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("Updated port".to_string()),
                enabled: true,
                poe_enabled: true,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert_eq!(diff.ports_to_configure.len(), 1);
        assert_eq!(diff.ports_to_configure[0].vlan, 10);
        assert_eq!(diff.ports_to_configure[0].poe_enabled, true);
    }

    #[test]
    fn test_compute_diff_port_enforcement() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Access,
                    vlan: 20,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        // Without enforcement
        let diff_no_enforce = compute_diff(&current, &desired, false);
        assert_eq!(diff_no_enforce.ports_to_reset.len(), 0);

        // With enforcement
        let diff_enforce = compute_diff(&current, &desired, true);
        assert!(diff_enforce.has_changes());
        assert_eq!(diff_enforce.ports_to_reset.len(), 1);
        assert_eq!(diff_enforce.ports_to_reset[0], "2");
    }

    #[test]
    fn test_compute_diff_port_equivalence_access_vs_trunk() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Trunk,
                vlan: 10,
                allowed_vlans: vec![10], // Only native VLAN in allowed list
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        // These should be considered equivalent (access with no tags == trunk with only native VLAN)
        let diff = compute_diff(&current, &desired, false);
        assert!(!diff.has_changes());
    }

    #[test]
    fn test_compute_diff_mirror_additions() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.port_mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string(), "2".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert_eq!(diff.mirrors_to_add.len(), 1);
        assert_eq!(diff.mirrors_to_add[0].session_id, "1");
    }

    #[test]
    fn test_compute_diff_snmp_addition() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.snmp = Some(SnmpConfig {
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
            enabled_traps: vec![TrapType::MacNotify],
        });

        let diff = compute_diff(&current, &desired, false);
        assert!(diff.has_changes());
        assert!(diff.snmp_config_changed);
        assert!(diff.snmp_config.is_some());
    }

    #[test]
    fn test_ports_equivalent_different_descriptions() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: Some("Port 1".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: Some("Port 1 Updated".to_string()),
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        assert!(!ports_equivalent(&port1, &port2));
    }

    #[test]
    fn test_ports_equivalent_trunk_mode() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Trunk,
            vlan: 10,
            allowed_vlans: vec![10, 20, 30],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Trunk,
            vlan: 10,
            allowed_vlans: vec![10, 20, 30],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        assert!(ports_equivalent(&port1, &port2));
    }

    #[test]
    fn test_ports_equivalent_different_tagged_vlans() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Trunk,
            vlan: 10,
            allowed_vlans: vec![10, 20],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Trunk,
            vlan: 10,
            allowed_vlans: vec![10, 30],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        assert!(!ports_equivalent(&port1, &port2));
    }

    #[test]
    fn test_compute_diff_snmp_trap_receiver_removal() {
        // Test case: switch has extra trap receiver that should be removed
        let current = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: Some(SnmpConfig {
                communities: vec![
                    SnmpCommunity {
                        name: "public".to_string(),
                        access: SnmpAccess::Operator,
                    },
                ],
                trap_receivers: vec![
                    SnmpTrapReceiver {
                        host: "192.168.1.100".to_string(),
                        community: "public".to_string(),
                        version: Some("2c".to_string()),
                    },
                    SnmpTrapReceiver {
                        host: "192.168.1.1".to_string(),
                        community: "public".to_string(),
                        version: Some("2c".to_string()),
                    },
                ],
                enabled_traps: vec![TrapType::MacNotify],
            }),
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.snmp = Some(SnmpConfig {
            communities: vec![
                SnmpCommunity {
                    name: "public".to_string(),
                    access: SnmpAccess::Operator,
                },
            ],
            trap_receivers: vec![
                SnmpTrapReceiver {
                    host: "192.168.1.1".to_string(),
                    community: "public".to_string(),
                    version: Some("2c".to_string()),
                },
            ],
            enabled_traps: vec![TrapType::MacNotify],
        });

        let diff = compute_diff(&current, &desired, false);
        // Should detect SNMP config change because trap_receivers differ
        assert!(diff.has_changes());
        assert!(diff.snmp_config_changed);
        assert!(diff.snmp_config.is_some());

        // The desired config should only have 1 trap receiver
        let snmp_config = diff.snmp_config.unwrap();
        assert_eq!(snmp_config.trap_receivers.len(), 1);
        assert_eq!(snmp_config.trap_receivers[0].host, "192.168.1.1");
    }

    // ========== Speed_Duplex Diff Tests ==========

    #[test]
    fn test_compute_diff_speed_duplex_change() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: Some("Test Port".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("Test Port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect speed_duplex change
        assert!(diff.has_changes());
        assert_eq!(diff.ports_to_configure.len(), 1);
        assert_eq!(diff.ports_to_configure[0].speed_duplex, SpeedDuplex::HundredFull);
    }

    #[test]
    fn test_compute_diff_speed_duplex_no_change() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: Some("Test Port".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::HundredFull,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("Test Port".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should NOT detect any changes (speed_duplex matches)
        assert!(!diff.has_changes());
        assert_eq!(diff.ports_to_configure.len(), 0);
    }

    #[test]
    fn test_compute_diff_multiple_ports_different_speeds() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,  // No change
            },
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,  // Changed
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should only detect change for port 2
        assert!(diff.has_changes());
        assert_eq!(diff.ports_to_configure.len(), 1);
        assert_eq!(diff.ports_to_configure[0].port_id, "2");
        assert_eq!(diff.ports_to_configure[0].speed_duplex, SpeedDuplex::HundredFull);
    }

    #[test]
    fn test_compute_diff_speed_duplex_with_other_changes() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: Some("Old Description".to_string()),
                    enabled: true,
                    poe_enabled: true,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("New Description".to_string()),
                enabled: true,
                poe_enabled: false,  // Changed
                mac_notify: true,     // Changed
                speed_duplex: SpeedDuplex::HundredFull,  // Changed
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect all changes including speed_duplex
        assert!(diff.has_changes());
        assert_eq!(diff.ports_to_configure.len(), 1);
        
        let port = &diff.ports_to_configure[0];
        assert_eq!(port.speed_duplex, SpeedDuplex::HundredFull);
        assert_eq!(port.poe_enabled, false);
        assert_eq!(port.mac_notify, true);
        assert_eq!(port.description, Some("New Description".to_string()));
    }

    #[test]
    fn test_ports_equivalent_different_speed_duplex() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::HundredFull,
        };

        // Ports should NOT be equivalent due to different speed_duplex
        assert!(!ports_equivalent(&port1, &port2));
    }

    #[test]
    fn test_ports_equivalent_same_speed_duplex() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::HundredFull,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,
            mac_notify: false,
            speed_duplex: SpeedDuplex::HundredFull,
        };

        // Ports should be equivalent
        assert!(ports_equivalent(&port1, &port2));
    }

    // ========== Port Migration Tests ==========

    #[test]
    fn test_find_ports_to_migrate_no_vlans_to_remove() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 0);
    }

    #[test]
    fn test_find_ports_to_migrate_access_port_on_vlan_being_removed() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Access,
                    vlan: 20,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![10];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].port_id, "1");
        assert_eq!(migrations[0].vlan_being_removed, 10);
        assert_eq!(migrations[0].action, PortMigrationAction::MoveAccessToVlan1);
    }

    #[test]
    fn test_find_ports_to_migrate_multiple_access_ports() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "3".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "5".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![10];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 3);
        assert_eq!(migrations[0].vlan_being_removed, 10);
        assert_eq!(migrations[1].vlan_being_removed, 10);
        assert_eq!(migrations[2].vlan_being_removed, 10);
    }

    #[test]
    fn test_find_ports_to_migrate_trunk_port_with_vlan_being_removed() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 1,
                    allowed_vlans: vec![10, 20, 30],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![20];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].port_id, "1");
        assert_eq!(migrations[0].vlan_being_removed, 20);

        match &migrations[0].action {
            PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans } => {
                assert_eq!(remaining_vlans.len(), 2);
                assert!(remaining_vlans.contains(&10));
                assert!(remaining_vlans.contains(&30));
                assert!(!remaining_vlans.contains(&20));
            }
            _ => panic!("Expected RemoveVlanFromTrunk action"),
        }
    }

    #[test]
    fn test_find_ports_to_migrate_trunk_port_all_vlans_removed() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 1,
                    allowed_vlans: vec![10, 20],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![10, 20];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        // Should find migration for one of the VLANs (stops at first match per port)
        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].port_id, "1");

        match &migrations[0].action {
            PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans } => {
                // After removing the first VLAN, should have one remaining
                // But since we're removing both, the actual remaining will be 0 or 1
                // depending on which VLAN was processed first
                assert!(remaining_vlans.len() <= 1);
            }
            _ => panic!("Expected RemoveVlanFromTrunk action"),
        }
    }

    #[test]
    fn test_find_ports_to_migrate_mixed_access_and_trunk() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 1,
                    allowed_vlans: vec![10, 20],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![10];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 2);

        // Port 1 should be MoveAccessToVlan1
        let port1_migration = migrations.iter().find(|m| m.port_id == "1").unwrap();
        assert_eq!(port1_migration.action, PortMigrationAction::MoveAccessToVlan1);

        // Port 2 should be RemoveVlanFromTrunk
        let port2_migration = migrations.iter().find(|m| m.port_id == "2").unwrap();
        match &port2_migration.action {
            PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans } => {
                assert_eq!(remaining_vlans.len(), 1);
                assert_eq!(remaining_vlans[0], 20);
            }
            _ => panic!("Expected RemoveVlanFromTrunk action for port 2"),
        }
    }

    #[test]
    fn test_find_ports_to_migrate_no_affected_ports() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 1,
                    allowed_vlans: vec![10, 20],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![30]; // VLAN not used by any port
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 0);
    }

    #[test]
    fn test_find_ports_to_migrate_multiple_vlans_to_remove() {
        let current = SwitchState {
            vlans: vec![],
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
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "2".to_string(),
                    mode: PortMode::Access,
                    vlan: 20,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
                Port {
                    port_id: "3".to_string(),
                    mode: PortMode::Access,
                    vlan: 30,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![10, 20];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 2);
        assert!(migrations.iter().any(|m| m.port_id == "1" && m.vlan_being_removed == 10));
        assert!(migrations.iter().any(|m| m.port_id == "2" && m.vlan_being_removed == 20));
    }

    #[test]
    fn test_find_ports_to_migrate_trunk_removes_one_vlan_from_many() {
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "uplink1".to_string(),
                    mode: PortMode::Trunk,
                    vlan: 1,
                    allowed_vlans: vec![10, 20, 30, 40, 50],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let vlans_to_remove = vec![30];
        let migrations = find_ports_to_migrate(&current, &vlans_to_remove);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].port_id, "uplink1");

        match &migrations[0].action {
            PortMigrationAction::RemoveVlanFromTrunk { remaining_vlans } => {
                assert_eq!(remaining_vlans.len(), 4);
                assert!(remaining_vlans.contains(&10));
                assert!(remaining_vlans.contains(&20));
                assert!(remaining_vlans.contains(&40));
                assert!(remaining_vlans.contains(&50));
                assert!(!remaining_vlans.contains(&30));
            }
            _ => panic!("Expected RemoveVlanFromTrunk action"),
        }
    }

    // ========== Non-PoE Switch Model-Aware Diff Tests ==========

    #[test]
    fn test_non_poe_switch_ignores_poe_enabled_diff() {
        // When a switch doesn't support PoE, poe_enabled differences should be ignored
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: Some("Server Port".to_string()),
                    enabled: true,
                    poe_enabled: false,  // Parser sets false for non-PoE switch
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        // Config has poe_enabled: true (user mistake or default)
        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::Aruba2540_48G_4SFP);  // Non-PoE switch
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: Some("Server Port".to_string()),
                enabled: true,
                poe_enabled: true,   // User incorrectly specified true
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should NOT detect changes because poe_enabled should be ignored for non-PoE switches
        assert!(!diff.has_changes(), "Non-PoE switch should ignore poe_enabled differences");
        assert_eq!(diff.ports_to_configure.len(), 0);
    }

    #[test]
    fn test_poe_switch_detects_poe_enabled_diff() {
        // When a switch supports PoE, poe_enabled differences SHOULD be detected
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: true,  // PoE enabled on switch
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::Aruba2930F);  // PoE-capable switch
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                allowed_vlans: vec![],
                description: None,
                enabled: true,
                poe_enabled: false,  // User wants PoE disabled
                mac_notify: false,
                speed_duplex: SpeedDuplex::Auto,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect changes because this is a PoE switch
        assert!(diff.has_changes(), "PoE switch should detect poe_enabled differences");
        assert_eq!(diff.ports_to_configure.len(), 1);
        assert_eq!(diff.ports_to_configure[0].poe_enabled, false);
    }

    #[test]
    fn test_ports_equivalent_for_model_non_poe_ignores_poe() {
        let port1 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: false,  // Current state
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        let port2 = Port {
            port_id: "1".to_string(),
            mode: PortMode::Access,
            vlan: 10,
            allowed_vlans: vec![],
            description: None,
            enabled: true,
            poe_enabled: true,   // Different poe_enabled
            mac_notify: false,
            speed_duplex: SpeedDuplex::Auto,
        };

        // With supports_poe = false, should be equivalent despite poe_enabled difference
        assert!(ports_equivalent_for_model(&port1, &port2, false),
            "Non-PoE switch should ignore poe_enabled difference");

        // With supports_poe = true, should NOT be equivalent
        assert!(!ports_equivalent_for_model(&port1, &port2, true),
            "PoE switch should detect poe_enabled difference");
    }

    #[test]
    fn test_non_poe_switch_still_detects_other_differences() {
        // Non-PoE switch should still detect non-PoE related changes
        let current = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    allowed_vlans: vec![],
                    description: Some("Old Description".to_string()),
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: SpeedDuplex::Auto,
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::Aruba2540_48G_4SFP);  // Non-PoE switch
        desired.ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 20,  // Different VLAN
                allowed_vlans: vec![],
                description: Some("New Description".to_string()),  // Different description
                enabled: true,
                poe_enabled: true,   // Different PoE (should be ignored)
                mac_notify: false,
                speed_duplex: SpeedDuplex::HundredFull,  // Different speed
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect changes (VLAN, description, speed_duplex)
        assert!(diff.has_changes(), "Should detect non-PoE related changes");
        assert_eq!(diff.ports_to_configure.len(), 1);
        assert_eq!(diff.ports_to_configure[0].vlan, 20);
    }

    // ========================================================================
    // VLAN Description Model-Aware Diff Tests
    // ========================================================================

    #[test]
    fn test_aruba_switch_ignores_vlan_description_diff() {
        // Aruba switches don't support VLAN descriptions, only names
        // Description differences should be ignored for Aruba
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "test-vlan".to_string(),
                    description: None,  // Aruba parser returns None for description
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::Aruba2540_48G_4SFP);  // Aruba switch
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "test-vlan".to_string(),
                description: Some("Test VLAN Description".to_string()),  // User specified description
                ip_config: VlanIpConfig::None,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should NOT detect changes - description should be ignored for Aruba
        assert!(!diff.has_changes(), "Aruba switch should ignore VLAN description differences");
        assert_eq!(diff.vlans_to_update.len(), 0);
    }

    #[test]
    fn test_cisco_switch_detects_vlan_description_diff() {
        // Cisco switches support VLAN descriptions
        // Description differences SHOULD be detected for Cisco
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "test-vlan".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::CiscoCatalyst9300_24P_UPOE);  // Cisco switch
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "test-vlan".to_string(),
                description: Some("Test VLAN Description".to_string()),  // Different description
                ip_config: VlanIpConfig::None,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect changes - Cisco supports VLAN descriptions
        assert!(diff.has_changes(), "Cisco switch should detect VLAN description differences");
        assert_eq!(diff.vlans_to_update.len(), 1);
    }

    #[test]
    fn test_vlans_equivalent_for_model_function() {
        let vlan1 = Vlan {
            id: 10,
            name: "test-vlan".to_string(),
            description: None,
            ip_config: VlanIpConfig::None,
        };

        let vlan2 = Vlan {
            id: 10,
            name: "test-vlan".to_string(),
            description: Some("Has description".to_string()),
            ip_config: VlanIpConfig::None,
        };

        // With supports_vlan_description = false (Aruba), should be equivalent
        assert!(vlans_equivalent_for_model(&vlan1, &vlan2, false),
            "Non-description-supporting switch should ignore description difference");

        // With supports_vlan_description = true (Cisco/FortiSwitch), should NOT be equivalent
        assert!(!vlans_equivalent_for_model(&vlan1, &vlan2, true),
            "Description-supporting switch should detect description difference");
    }

    #[test]
    fn test_aruba_switch_still_detects_vlan_name_diff() {
        // Aruba should still detect VLAN name changes even if descriptions are ignored
        let current = SwitchState {
            vlans: vec![
                Vlan {
                    id: 10,
                    name: "old-name".to_string(),
                    description: None,
                    ip_config: VlanIpConfig::None,
                },
            ],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let mut desired = create_test_switch_config();
        desired.model = Some(SwitchModel::Aruba2540_48G_4SFP);  // Aruba switch
        desired.vlans = vec![
            Vlan {
                id: 10,
                name: "new-name".to_string(),  // Different name
                description: Some("Ignored description".to_string()),
                ip_config: VlanIpConfig::None,
            },
        ];

        let diff = compute_diff(&current, &desired, false);

        // Should detect name change even though description is ignored
        assert!(diff.has_changes(), "Aruba should detect VLAN name changes");
        assert_eq!(diff.vlans_to_update.len(), 1);
        assert_eq!(diff.vlans_to_update[0].name, "new-name");
    }
}
