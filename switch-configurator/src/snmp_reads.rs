//! The community the service reads with over SNMP, kept next to the
//! communities a person lists.
//!
//! With `snmp.reads.mode: auto` the switch must end up with the listed
//! communities AND the service's own read community. They are merged into ONE
//! desired list before the diff, so a single apply writes both: an apply of the
//! listed communities can't delete the service's community, and the service
//! never has to touch the listed ones. Rotating the secret falls out of the
//! normal diff (the old string is no longer wanted, the new one is added).

use crate::models::{SnmpAccess, SnmpConfig, SnmpReadsMode, SwitchConfig};

/// The SNMP config the switch should end up with: the listed communities plus,
/// when `snmp.reads` is on, the service's own read community.
pub fn effective_snmp(switch: &SwitchConfig) -> Option<SnmpConfig> {
    let mut effective = switch.snmp.clone()?;
    if reads_enabled(switch) {
        if let Some(mut managed) = effective.reads.as_ref().and_then(|r| r.community.clone()) {
            // The service only ever reads, whatever the entry says.
            managed.access = SnmpAccess::Operator;
            if !effective.communities.iter().any(|c| c.name == managed.name) {
                effective.communities.push(managed);
            }
        }
    }
    Some(effective)
}

/// Is the service's own read community switched on for this switch?
pub fn reads_enabled(switch: &SwitchConfig) -> bool {
    switch
        .snmp
        .as_ref()
        .and_then(|snmp| snmp.reads.as_ref())
        .is_some_and(|reads| reads.mode == SnmpReadsMode::Auto && reads.community.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AllowedHost, ConnectionType, Credentials, SnmpCommunity, SnmpReads, SwitchModel, TrapType,
    };

    fn switch(snmp: Option<SnmpConfig>) -> SwitchConfig {
        SwitchConfig {
            id: "sw1".to_string(),
            hostname: Some("sw1".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.0.2.10".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("pw".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 115200,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp,
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        }
    }

    fn reads(mode: SnmpReadsMode, community: Option<SnmpCommunity>) -> Option<SnmpReads> {
        Some(SnmpReads { mode, community })
    }

    fn config(communities: Vec<SnmpCommunity>, reads: Option<SnmpReads>) -> SnmpConfig {
        SnmpConfig { communities, trap_receivers: vec![], enabled_traps: vec![TrapType::LinkChange], reads }
    }

    fn managed() -> SnmpCommunity {
        let mut c = SnmpCommunity::new("service-read-secret", SnmpAccess::Unrestricted);
        c.name_credential = Some("snmp-ro".to_string());
        c
    }

    fn names(snmp: &SnmpConfig) -> Vec<&str> {
        snmp.communities.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn without_reads_the_listed_communities_are_all_there_is() {
        let listed = vec![SnmpCommunity::new("mine", SnmpAccess::Operator)];
        let sw = switch(Some(config(listed.clone(), None)));
        assert_eq!(effective_snmp(&sw).unwrap().communities, listed);
        assert!(!reads_enabled(&sw));
    }

    #[test]
    fn reads_off_adds_nothing_even_when_a_community_is_configured() {
        let sw = switch(Some(config(vec![], reads(SnmpReadsMode::Off, Some(managed())))));
        let effective = effective_snmp(&sw);
        assert!(effective.is_none_or(|c| c.communities.is_empty()));
        assert!(!reads_enabled(&sw));
    }

    #[test]
    fn reads_auto_appends_the_service_community_after_the_listed_ones() {
        let sw = switch(Some(config(
            vec![SnmpCommunity::new("mine", SnmpAccess::Operator)],
            reads(SnmpReadsMode::Auto, Some(managed())),
        )));

        let effective = effective_snmp(&sw).unwrap();

        assert_eq!(names(&effective), vec!["mine", "service-read-secret"]);
        assert_eq!(effective.communities[1].access, SnmpAccess::Operator, "the service only ever reads");
        assert_eq!(effective.enabled_traps, vec![TrapType::LinkChange], "everything else is carried over");
        assert!(reads_enabled(&sw));
    }

    #[test]
    fn reads_auto_works_when_nothing_else_is_listed_and_when_snmp_is_otherwise_absent() {
        let only_reads = switch(Some(config(vec![], reads(SnmpReadsMode::Auto, Some(managed())))));
        assert_eq!(names(&effective_snmp(&only_reads).unwrap()), vec!["service-read-secret"]);
    }

    #[test]
    fn a_listed_community_with_the_same_string_is_not_duplicated() {
        let sw = switch(Some(config(
            vec![SnmpCommunity::new("service-read-secret", SnmpAccess::Operator)],
            reads(SnmpReadsMode::Auto, Some(managed())),
        )));
        assert_eq!(names(&effective_snmp(&sw).unwrap()), vec!["service-read-secret"]);
    }

    #[test]
    fn the_managed_community_keeps_its_own_allowed_hosts() {
        let mut community = managed();
        community.allowed_hosts = vec![AllowedHost::Network("192.0.2.1".to_string())];
        let sw = switch(Some(config(vec![], reads(SnmpReadsMode::Auto, Some(community)))));

        let effective = effective_snmp(&sw).unwrap();

        assert_eq!(effective.communities[0].allowed_hosts, vec![AllowedHost::Network("192.0.2.1".to_string())]);
    }

    #[test]
    fn auto_without_a_community_adds_nothing() {
        let sw = switch(Some(config(vec![], reads(SnmpReadsMode::Auto, None))));
        assert!(effective_snmp(&sw).is_none_or(|c| c.communities.is_empty()));
        assert!(!reads_enabled(&sw), "auto with nothing to configure is not 'enabled'");
    }

    #[test]
    fn effective_snmp_does_not_change_the_switch_config_itself() {
        let sw = switch(Some(config(vec![], reads(SnmpReadsMode::Auto, Some(managed())))));
        let _ = effective_snmp(&sw);
        assert!(sw.snmp.as_ref().unwrap().communities.is_empty(), "the listed communities stay as written");
    }

    #[test]
    fn yaml_parses_the_reads_block_and_mode_defaults_to_off() {
        let yaml = "communities: []\nreads:\n  mode: auto\n  community:\n    name_credential: snmp-ro\n    allowed_hosts: [auto]\n";
        let snmp: SnmpConfig = serde_yaml::from_str(yaml).unwrap();
        let reads = snmp.reads.unwrap();
        assert_eq!(reads.mode, SnmpReadsMode::Auto);
        assert_eq!(reads.community.unwrap().name_credential.as_deref(), Some("snmp-ro"));

        let bare: SnmpConfig = serde_yaml::from_str("reads:\n  community:\n    name: x\n").unwrap();
        assert_eq!(bare.reads.unwrap().mode, SnmpReadsMode::Off);
    }

    #[test]
    fn a_resolved_reads_community_serialises_as_its_reference_only() {
        let sw = switch(Some(config(vec![], reads(SnmpReadsMode::Auto, Some(managed())))));
        let json = serde_json::to_string(&sw.snmp).unwrap();
        assert!(json.contains("name_credential") && !json.contains("service-read-secret"), "leaked: {json}");
    }
}
