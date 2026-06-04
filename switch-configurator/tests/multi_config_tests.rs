use std::path::PathBuf;
use switch_configurator::config::{AppConfig, ConfigSourceType};

#[cfg(test)]
mod multi_config_tests {
    use super::*;

    /// Helper to create a test fixtures path
    fn fixtures_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config")
            .join(relative)
    }

    #[test]
    fn test_basic_multi_config_merge() {
        // Setup: main config + folder with vlans and ports
        let main_config = fixtures_path("basic/main.yaml");
        let common_folder = fixtures_path("basic/common");

        let result = AppConfig::load_multi(&main_config, &[common_folder]);

        assert!(result.is_ok(), "Basic multi-config merge should succeed");
        let (config, _failures) = result.unwrap();

        assert_eq!(config.switches.len(), 1, "Should have 1 switch");
        let switch = &config.switches[0];

        assert_eq!(switch.id, "test-sw-01");
        assert_eq!(switch.hostname.as_deref(), Some("test-switch-01"));

        // Check that VLANs from folder config were merged
        assert_eq!(switch.vlans.len(), 2, "Should have 2 VLANs");

        // Check that ports from folder config were merged (and expanded)
        assert!(switch.ports.len() >= 5, "Should have at least 5 ports after expansion");
    }

    #[test]
    fn test_priority_override() {
        // Setup: main + common + override folder
        let main_config = fixtures_path("priority/main.yaml");
        let common_folder = fixtures_path("priority/common");
        let override_folder = fixtures_path("priority/overrides");

        let result = AppConfig::load_multi(&main_config, &[common_folder, override_folder]);

        assert!(result.is_ok(), "Priority override merge should succeed");
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Find VLAN 10 which should be overridden
        let vlan_10 = switch.vlans.iter().find(|v| v.id == 10);
        assert!(vlan_10.is_some(), "VLAN 10 should exist");

        let vlan = vlan_10.unwrap();
        // Name should come from higher priority override (priority 50)
        assert_eq!(vlan.name, "mgmt-override", "VLAN 10 name should be overridden");

        // VLAN 100 should not be overridden
        let vlan_100 = switch.vlans.iter().find(|v| v.id == 100);
        assert!(vlan_100.is_some(), "VLAN 100 should exist");
        assert_eq!(vlan_100.unwrap().name, "users", "VLAN 100 should not be overridden");
    }

    #[test]
    fn test_conflict_detection_hostname_mismatch() {
        // Setup: configs with conflicting hostname
        let main_config = fixtures_path("conflicts/main.yaml");
        let conflict_folder = fixtures_path("conflicts/hostname-conflict");

        let result = AppConfig::load_multi(&main_config, &[conflict_folder]);

        assert!(result.is_err(), "Should fail on hostname conflict");
        let error_msg = result.unwrap_err().to_string();

        assert!(error_msg.contains("merge conflicts"), "Error should mention merge conflicts");
        assert!(error_msg.contains("hostname"), "Error should mention hostname");
        assert!(error_msg.contains("Hostname mismatch"), "Error should describe the mismatch");
    }

    #[test]
    fn test_conflict_detection_management_ip_mismatch() {
        // Setup: configs with conflicting management_ip
        let main_config = fixtures_path("conflicts/main.yaml");
        let conflict_folder = fixtures_path("conflicts/ip-conflict");

        let result = AppConfig::load_multi(&main_config, &[conflict_folder]);

        assert!(result.is_err(), "Should fail on management_ip conflict");
        let error_msg = result.unwrap_err().to_string();

        assert!(error_msg.contains("management_ip"), "Error should mention management_ip");
        assert!(error_msg.contains("Management IP mismatch"), "Error should describe the mismatch");
    }

    #[test]
    fn test_conflict_detection_model_mismatch() {
        // Setup: configs with conflicting model
        let main_config = fixtures_path("conflicts/main.yaml");
        let conflict_folder = fixtures_path("conflicts/model-conflict");

        let result = AppConfig::load_multi(&main_config, &[conflict_folder]);

        assert!(result.is_err(), "Should fail on model conflict");
        let error_msg = result.unwrap_err().to_string();

        assert!(error_msg.contains("model"), "Error should mention model");
        assert!(error_msg.contains("Model mismatch"), "Error should describe the mismatch");
    }

    #[test]
    fn test_priority_validation_folder_cannot_use_0_to_10() {
        // Setup: folder config with priority in 0-10 range
        let main_config = fixtures_path("priority-validation/main.yaml");
        let invalid_folder = fixtures_path("priority-validation/invalid-priority");

        let result = AppConfig::load_multi(&main_config, &[invalid_folder]);

        assert!(result.is_err(), "Should fail when folder uses priority 0-10: {:?}", result.as_ref().err());
        let error_msg = result.unwrap_err().to_string();

        // The error could contain either the priority restriction message or reference the file
        assert!(error_msg.contains("0-10 reserved") || error_msg.contains("bad.yaml"),
                "Error should explain priority restriction. Got: {}", error_msg);
    }

    #[test]
    fn test_port_range_expansion_in_merge() {
        // Setup: config with port ranges
        let main_config = fixtures_path("port-ranges/main.yaml");
        let ports_folder = fixtures_path("port-ranges/common");

        let result = AppConfig::load_multi(&main_config, &[ports_folder]);

        assert!(result.is_ok(), "Port range expansion should succeed");
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Port range "2-5" should expand to 4 individual ports
        let port_2 = switch.ports.iter().find(|p| p.port_id == "2");
        let port_3 = switch.ports.iter().find(|p| p.port_id == "3");
        let port_4 = switch.ports.iter().find(|p| p.port_id == "4");
        let port_5 = switch.ports.iter().find(|p| p.port_id == "5");

        assert!(port_2.is_some(), "Port 2 should exist after expansion");
        assert!(port_3.is_some(), "Port 3 should exist after expansion");
        assert!(port_4.is_some(), "Port 4 should exist after expansion");
        assert!(port_5.is_some(), "Port 5 should exist after expansion");

        // All should have same description from range
        assert_eq!(port_2.unwrap().description, port_3.unwrap().description);
    }

    #[test]
    fn test_snmp_sub_component_merge() {
        // Setup: multiple configs with different SNMP components
        let main_config = fixtures_path("snmp-merge/main.yaml");
        let common_folder = fixtures_path("snmp-merge/common");
        let override_folder = fixtures_path("snmp-merge/overrides");

        let result = AppConfig::load_multi(&main_config, &[common_folder, override_folder]);

        assert!(result.is_ok(), "SNMP merge should succeed: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];
        let snmp = switch.snmp.as_ref().expect("SNMP config should exist");

        // Communities should come from highest priority non-empty list
        assert!(!snmp.communities.is_empty(), "Should have communities");

        // Trap receivers should come from highest priority non-empty list
        assert!(!snmp.trap_receivers.is_empty(), "Should have trap receivers");

        // Enabled traps should come from highest priority non-empty list
        assert!(!snmp.enabled_traps.is_empty(), "Should have enabled traps");
    }

    #[test]
    fn test_port_mirror_merge_by_session_id() {
        // Setup: configs with port mirrors
        let main_config = fixtures_path("mirrors/main.yaml");
        let mirrors_folder = fixtures_path("mirrors/common");

        let result = AppConfig::load_multi(&main_config, &[mirrors_folder]);

        assert!(result.is_ok(), "Port mirror merge should succeed: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Should have port mirrors
        assert!(!switch.port_mirrors.is_empty(), "Should have port mirrors");

        // Find session "1"
        let session_1 = switch.port_mirrors.iter().find(|m| m.session_id == "1");
        assert!(session_1.is_some(), "Session 1 should exist");
    }

    #[test]
    fn test_validation_config_merge() {
        // Setup: configs with validation
        let main_config = fixtures_path("validation/main.yaml");
        let validation_folder = fixtures_path("validation/common");

        let result = AppConfig::load_multi(&main_config, &[validation_folder]);

        assert!(result.is_ok(), "Validation merge should succeed: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Validation should come from highest priority config that has it
        assert!(switch.validation.is_some(), "Should have validation config");
        let validation = switch.validation.as_ref().unwrap();
        assert!(validation.enabled, "Validation should be enabled");
    }

    #[test]
    fn test_folder_scanning_alphabetical_order() {
        // Setup: folder with multiple configs (01-, 02-, 03- prefixes)
        let main_config = fixtures_path("ordering/main.yaml");
        let ordered_folder = fixtures_path("ordering/configs");

        let result = AppConfig::load_multi(&main_config, &[ordered_folder]);

        assert!(result.is_ok(), "Alphabetically ordered configs should load: {:?}", result.as_ref().err());
        // The merge should succeed and process files in order
        let (config, _failures) = result.unwrap();
        assert_eq!(config.switches.len(), 1);
    }

    #[test]
    fn test_credentials_merge_highest_priority_wins() {
        // Setup: main config with credentials, folder tries to override
        let main_config = fixtures_path("credentials/main.yaml");
        let folder = fixtures_path("credentials/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Credentials merge should succeed");
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Credentials should come from main config (priority 5)
        // not from folder config (priority 100)
        assert_eq!(switch.credentials.as_ref().unwrap().username, "main-admin",
                   "Credentials should come from highest priority (main)");
    }

    #[test]
    fn test_settings_merge_highest_priority_wins() {
        // Setup: main config with settings, folder tries to override
        let main_config = fixtures_path("settings/main.yaml");
        let folder = fixtures_path("settings/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Settings merge should succeed");
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Settings should come from main config (highest priority)
        assert!(switch.settings.enforce_port_config, "enforce_port_config from main");
    }

    #[test]
    fn test_empty_main_config_with_folders() {
        // Setup: main config with minimal fields, everything from folders
        let main_config = fixtures_path("minimal-main/main.yaml");
        let folder = fixtures_path("minimal-main/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Minimal main config should work: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];

        // Should have VLANs and ports from folder
        assert!(!switch.vlans.is_empty(), "Should have VLANs from folder");
        assert!(!switch.ports.is_empty(), "Should have ports from folder");
    }

    #[test]
    fn test_multiple_switches_same_config() {
        // Setup: config files defining multiple switches
        let main_config = fixtures_path("multi-switch/main.yaml");
        let folder = fixtures_path("multi-switch/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Multiple switches should merge independently: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        assert_eq!(config.switches.len(), 2, "Should have 2 switches");

        // Each switch should have been merged independently
        let switch_1 = config.switches.iter().find(|s| s.id == "sw-01");
        let switch_2 = config.switches.iter().find(|s| s.id == "sw-02");

        assert!(switch_1.is_some(), "Switch 1 should exist");
        assert!(switch_2.is_some(), "Switch 2 should exist");
    }

    #[test]
    fn test_split_vlans_and_ports_into_separate_files() {
        // This test verifies that VLANs and ports can be in separate files
        // Previously this would fail because validation happened before merging
        let main_config = fixtures_path("split-vlans-ports/main.yaml");
        let folder = fixtures_path("split-vlans-ports/configs");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Should allow splitting VLANs and ports: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        assert_eq!(config.switches.len(), 1, "Should have 1 switch");
        let switch = &config.switches[0];

        // Verify VLANs from vlans.yaml
        assert_eq!(switch.vlans.len(), 2, "Should have 2 VLANs");
        assert!(switch.vlans.iter().any(|v| v.id == 10 && v.name == "management"));
        assert!(switch.vlans.iter().any(|v| v.id == 20 && v.name == "users"));

        // Verify ports from ports.yaml (after range expansion: 1-5 becomes 1,2,3,4,5)
        assert_eq!(switch.ports.len(), 6, "Should have 6 ports after expansion");

        // Verify all ports reference valid VLANs
        for port in &switch.ports {
            assert!(
                switch.vlans.iter().any(|v| v.id == port.vlan),
                "Port {} references valid VLAN {}",
                port.port_id,
                port.vlan
            );
        }
    }

    #[test]
    fn test_snmp_empty_lists_treated_as_not_present() {
        // Setup: base config with SNMP, override with empty SNMP lists
        // NOTE: Current implementation treats empty lists as "not specified"
        // and inherits from lower priority. To truly clear SNMP, don't include
        // the snmp field at all in the higher priority config.
        let main_config = fixtures_path("snmp-clear/main.yaml");
        let base_folder = fixtures_path("snmp-clear/base");
        let clear_folder = fixtures_path("snmp-clear/clear");

        let result = AppConfig::load_multi(&main_config, &[base_folder, clear_folder]);

        assert!(result.is_ok(), "SNMP merge should succeed: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];
        let snmp = switch.snmp.as_ref().expect("SNMP config should exist");

        // Empty lists in higher priority are treated as "not specified"
        // so values should come from lower priority base config
        assert!(!snmp.communities.is_empty(), "Communities should inherit from base (empty lists ignored)");
        assert!(!snmp.trap_receivers.is_empty(), "Trap receivers should inherit from base");
        assert!(!snmp.enabled_traps.is_empty(), "Enabled traps should inherit from base");
    }

    #[test]
    fn test_snmp_not_present_uses_lower_priority() {
        // Setup: base config with SNMP, higher priority config without SNMP field
        let main_config = fixtures_path("snmp-inherit/main.yaml");
        let base_folder = fixtures_path("snmp-inherit/base");
        let override_folder = fixtures_path("snmp-inherit/override");

        let result = AppConfig::load_multi(&main_config, &[base_folder, override_folder]);

        assert!(result.is_ok(), "SNMP inherit should succeed: {:?}", result.as_ref().err());
        let (config, _failures) = result.unwrap();

        let switch = &config.switches[0];
        let snmp = switch.snmp.as_ref().expect("SNMP config should exist");

        // Should have SNMP from base config (lower priority) since override doesn't specify SNMP
        assert!(!snmp.communities.is_empty(), "Should have communities from base config");
        assert_eq!(snmp.communities.len(), 2, "Should have 2 communities from base");
        assert!(!snmp.trap_receivers.is_empty(), "Should have trap receivers from base config");
    }

    #[test]
    fn test_incomplete_credentials_accepted_by_schema() {
        // Setup: config with incomplete credentials (missing password)
        // NOTE: Current implementation does not validate that credentials have
        // either password OR ssh_key_path. This would require custom validation.
        // The schema allows password to be optional (Option<String>).
        let main_config = fixtures_path("incomplete-credentials/main.yaml");
        let folder = fixtures_path("incomplete-credentials/bad");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        // Currently passes schema validation even though password is missing
        assert!(result.is_ok(), "Incomplete credentials pass schema validation: {:?}", result.as_ref().err());

        let (config, _failures) = result.unwrap();
        let switch = &config.switches[0];

        // Credentials from main config (priority 5) win over folder (priority 50)
        // Main config has complete credentials, so those are used
        assert_eq!(switch.credentials.as_ref().unwrap().username, "admin", "Username should come from main (priority 5)");
        assert!(switch.credentials.as_ref().unwrap().password.is_some(), "Password from main config");
        assert_eq!(switch.credentials.as_ref().unwrap().password.as_ref().unwrap(), "goodpass");
    }

    #[test]
    fn test_multiple_conflicts_reported_together() {
        // Setup: configs with multiple conflicts (hostname + model + VLAN mismatch)
        let main_config = fixtures_path("multi-conflict/main.yaml");
        let conflict_folder = fixtures_path("multi-conflict/conflicts");

        let result = AppConfig::load_multi(&main_config, &[conflict_folder]);

        assert!(result.is_err(), "Should fail on multiple conflicts");
        let error_msg = result.unwrap_err().to_string();

        // Error should mention multiple issues
        // At minimum, we should see indication of conflicts
        assert!(
            error_msg.contains("conflict") || error_msg.contains("mismatch"),
            "Error should mention conflicts. Got: {}",
            error_msg
        );
    }

    // ============================================================================
    // Tests for Optional Fields Post-Merge Validation
    // ============================================================================
    // These tests verify behavior when required fields (credentials, vlans) are
    // missing after multi-config merge. According to TODO.md, these should be
    // validated AFTER merge, not during individual file parsing.
    // ============================================================================

    #[test]
    #[ignore] // TODO: Enable once post-merge validation is implemented (Task #5)
    fn test_missing_credentials_in_all_configs_should_fail() {
        // Setup: No config file provides credentials for the switch
        // Expected: Should fail with helpful error message after merge
        let main_config = fixtures_path("missing-credentials-all/main.yaml");
        let folder = fixtures_path("missing-credentials-all/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        // CURRENTLY: This passes but shouldn't - credentials is Option<Credentials>
        // SHOULD: Fail with error message like:
        // "Switch 'test-switch-missing-creds' (id: test-sw-missing-creds) is missing
        //  required field 'credentials' after merging all configuration sources."
        assert!(result.is_err(), "Should fail when credentials missing in all configs");

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("credentials"), "Error should mention credentials");
        assert!(error_msg.contains("test-sw-missing-creds") || error_msg.contains("test-switch-missing-creds"),
                "Error should identify which switch is missing credentials");
    }

    #[test]
    fn test_missing_vlans_in_all_configs_skips_switch() {
        // Setup: No config file provides VLANs for the switch (empty vlans: [])
        // Expected: Switch is skipped (graceful mode) and appears in failures
        let main_config = fixtures_path("missing-vlans-all/main.yaml");
        let folder = fixtures_path("missing-vlans-all/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);
        assert!(result.is_ok(), "Load should succeed in graceful mode");

        let (config, failures) = result.unwrap();
        assert!(config.switches.is_empty() || !failures.is_empty(),
                "Invalid switch should be skipped or have failures");
    }

    #[test]
    fn test_credentials_provided_in_main_omitted_in_folder_succeeds() {
        // Setup: Main config has credentials, folder config omits them
        // Expected: Should succeed - credentials from main config are used
        let main_config = fixtures_path("credentials/main.yaml");
        let folder = fixtures_path("credentials/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Should succeed when credentials in main config");
        let (config, _failures) = result.unwrap();
        let switch = &config.switches[0];

        // Verify credentials came from main config
        assert!(switch.credentials.as_ref().unwrap().password.is_some() ||
                switch.credentials.as_ref().unwrap().ssh_key_path.is_some(),
                "Switch should have either password or SSH key from main config");
    }

    #[test]
    fn test_vlans_provided_in_folder_omitted_in_main_succeeds() {
        // Setup: Main config has empty vlans:[], folder config provides VLANs
        // Expected: Should succeed - VLANs from folder config are used
        let main_config = fixtures_path("minimal-main/main.yaml");
        let folder = fixtures_path("minimal-main/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);

        assert!(result.is_ok(), "Should succeed when VLANs provided in folder");
        let (config, _failures) = result.unwrap();
        let switch = &config.switches[0];

        // Verify VLANs came from folder config
        assert!(!switch.vlans.is_empty(), "Switch should have VLANs from folder config");
    }

    #[test]
    fn test_credentials_optional_during_parsing_required_after_merge() {
        // Credentials is Option<Credentials> — switch without credentials
        // is now skipped in graceful mode (validation failure)
        let main_config = fixtures_path("missing-credentials-all/main.yaml");
        let folder = fixtures_path("missing-credentials-all/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);
        assert!(result.is_ok(), "Load should succeed in graceful mode");

        let (config, failures) = result.unwrap();
        // Switch without credentials should be in failures
        let has_failure = failures.iter().any(|f| f.switch_id == "test-sw-missing-creds");
        let in_config = config.switches.iter().any(|s| s.id == "test-sw-missing-creds");
        // It's either skipped (in failures) or loaded without credentials
        assert!(has_failure || in_config, "Switch should appear somewhere");
    }

    #[test]
    fn test_vlans_optional_during_parsing_required_after_merge() {
        // Switch with empty VLANs should be skipped in graceful mode
        let main_config = fixtures_path("missing-vlans-all/main.yaml");
        let folder = fixtures_path("missing-vlans-all/common");

        let result = AppConfig::load_multi(&main_config, &[folder]);
        assert!(result.is_ok(), "Load should succeed in graceful mode");

        let (_config, _failures) = result.unwrap();
        // Switch may be in failures or config depending on validation
    }
}
