use super::traits::{SwitchVendor, VendorError};
use crate::models::{
    ConfigResult, ConnectionType, Credentials, MirrorDirection, Port, PortMirror, PortMode,
    StateDiff, SwitchConfig, SwitchModel,
    SwitchState, Vlan, VlanIpConfig,
};
use crate::validation::{ValidationConfig, ValidationResult, RollbackMethod};
use async_trait::async_trait;
use mockall::mock;

// Mock implementation of SwitchVendor trait for testing
mock! {
    pub Vendor {}

    #[async_trait]
    impl SwitchVendor for Vendor {
        async fn connect(&mut self) -> Result<(), VendorError>;
        async fn disconnect(&mut self) -> Result<(), VendorError>;
        async fn parse_current_state(&mut self) -> Result<SwitchState, VendorError>;
        async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>, VendorError>;
        async fn configure_vlans(&mut self, vlans: &[Vlan]) -> Result<ConfigResult, VendorError>;
        async fn configure_ports(&mut self, ports: &[Port]) -> Result<ConfigResult, VendorError>;
        async fn configure_port_mirrors(&mut self, mirrors: &[PortMirror]) -> Result<ConfigResult, VendorError>;
        async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError>;
        async fn save_configuration(&mut self) -> Result<(), VendorError>;
        async fn get_running_config(&mut self) -> Result<String, VendorError>;
        fn validate_configuration(&self) -> Result<(), VendorError>;
        async fn run_validation_tests(&mut self, validation_config: &ValidationConfig) -> Result<ValidationResult, VendorError>;
        async fn rollback_configuration(&mut self, method: RollbackMethod) -> Result<(), VendorError>;
        fn generate_commands_for_diff(&self, diff: &StateDiff) -> crate::models::CommandPreview;
        async fn execute_raw_commands(&mut self, commands: &[String]) -> Result<Vec<String>, VendorError>;
        fn get_warnings(&self) -> Vec<String>;
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_vendor_connect_success() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_connect()
            .times(1)
            .returning(|| Ok(()));

        let result = mock_vendor.connect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_connect_failure() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_connect()
            .times(1)
            .returning(|| Err(VendorError::SshError("Connection refused".to_string())));

        let result = mock_vendor.connect().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VendorError::SshError(_)));
    }

    #[tokio::test]
    async fn test_mock_vendor_parse_current_state() {
        let mut mock_vendor = MockVendor::new();

        let expected_state = SwitchState {
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
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 1,
                    tagged_vlans: vec![],
                    description: None,
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

        let expected_state_clone = expected_state.clone();

        mock_vendor
            .expect_parse_current_state()
            .times(1)
            .returning(move || Ok(expected_state_clone.clone()));

        let result = mock_vendor.parse_current_state().await;
        assert!(result.is_ok());

        let state = result.unwrap();
        assert_eq!(state.vlans.len(), 1);
        assert_eq!(state.vlans[0].id, 1);
        assert_eq!(state.ports.len(), 1);
        assert_eq!(state.ports[0].port_id, "1");
    }

    #[tokio::test]
    async fn test_mock_vendor_apply_diff() {
        let mut mock_vendor = MockVendor::new();

        let diff = StateDiff {
            vlans_to_add: vec![
                Vlan {
                    id: 10,
                    name: "vlan10".to_string(),
                    description: Some("Test VLAN".to_string()),
                    ip_config: VlanIpConfig::Dhcp,
                },
            ],
            vlans_to_remove: vec![],
            vlans_to_update: vec![],
            ports_to_configure: vec![],
            ports_to_reset: vec![],
            mirrors_to_add: vec![],
            mirrors_to_remove: vec![],
            mirrors_to_update: vec![],
            mirror_dest_ports_to_configure: vec![],
            snmp_config_changed: false,
            snmp_config: None,
            snmp_diff: None,
            management_vlan_changed: false,
            management_vlan: None,
        };

        mock_vendor
            .expect_apply_diff()
            .times(1)
            .returning(|_| {
                Ok(vec![ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: "VLANs configured successfully".to_string(),
                    commands_executed: vec!["vlan 10".to_string(), "name vlan10".to_string()],
                    timestamp: chrono::Utc::now(),
                }])
            });

        let result = mock_vendor.apply_diff(&diff).await;
        assert!(result.is_ok());

        let results = result.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_mock_vendor_configure_vlans() {
        let mut mock_vendor = MockVendor::new();

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
                description: Some("Management".to_string()),
                ip_config: VlanIpConfig::Dhcp,
            },
        ];

        mock_vendor
            .expect_configure_vlans()
            .times(1)
            .returning(|vlans| {
                Ok(ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: format!("Configured {} VLANs", vlans.len()),
                    commands_executed: vec![],
                    timestamp: chrono::Utc::now(),
                })
            });

        let result = mock_vendor.configure_vlans(&vlans).await;
        assert!(result.is_ok());

        let config_result = result.unwrap();
        assert!(config_result.success);
        assert!(config_result.message.contains("2 VLANs"));
    }

    #[tokio::test]
    async fn test_mock_vendor_configure_ports() {
        let mut mock_vendor = MockVendor::new();

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
            Port {
                port_id: "24".to_string(),
                mode: PortMode::Trunk,
                vlan: 1,
                tagged_vlans: vec![1, 10, 20],
                description: Some("Uplink".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        mock_vendor
            .expect_configure_ports()
            .times(1)
            .returning(|ports| {
                Ok(ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: format!("Configured {} ports", ports.len()),
                    commands_executed: vec![],
                    timestamp: chrono::Utc::now(),
                })
            });

        let result = mock_vendor.configure_ports(&ports).await;
        assert!(result.is_ok());

        let config_result = result.unwrap();
        assert!(config_result.success);
    }

    #[tokio::test]
    async fn test_mock_vendor_configure_port_mirrors() {
        let mut mock_vendor = MockVendor::new();

        let mirrors = vec![
            PortMirror {
                session_id: "1".to_string(),
                source_ports: vec!["1".to_string(), "2".to_string()],
                destination_port: "10".to_string(),
                direction: MirrorDirection::Both,
            },
        ];

        mock_vendor
            .expect_configure_port_mirrors()
            .times(1)
            .returning(|_| {
                Ok(ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: "Port mirroring configured".to_string(),
                    commands_executed: vec![],
                    timestamp: chrono::Utc::now(),
                })
            });

        let result = mock_vendor.configure_port_mirrors(&mirrors).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_apply_configuration() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_apply_configuration()
            .times(1)
            .returning(|| {
                Ok(vec![
                    ConfigResult {
                        switch: "test-switch".to_string(),
                        success: true,
                        message: "VLANs configured".to_string(),
                        commands_executed: vec![],
                        timestamp: chrono::Utc::now(),
                    },
                    ConfigResult {
                        switch: "test-switch".to_string(),
                        success: true,
                        message: "Ports configured".to_string(),
                        commands_executed: vec![],
                        timestamp: chrono::Utc::now(),
                    },
                ])
            });

        let result = mock_vendor.apply_configuration().await;
        assert!(result.is_ok());

        let results = result.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn test_mock_vendor_save_configuration() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_save_configuration()
            .times(1)
            .returning(|| Ok(()));

        let result = mock_vendor.save_configuration().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_save_configuration_failure() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_save_configuration()
            .times(1)
            .returning(|| Err(VendorError::CommandError("Failed to save config".to_string())));

        let result = mock_vendor.save_configuration().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_vendor_get_running_config() {
        let mut mock_vendor = MockVendor::new();

        let expected_config = r#"
vlan 1
  name DEFAULT_VLAN
vlan 10
  name management
interface 1
  untagged vlan 10
  enable
"#.to_string();

        let expected_config_clone = expected_config.clone();

        mock_vendor
            .expect_get_running_config()
            .times(1)
            .returning(move || Ok(expected_config_clone.clone()));

        let result = mock_vendor.get_running_config().await;
        assert!(result.is_ok());

        let config = result.unwrap();
        assert!(config.contains("vlan 1"));
        assert!(config.contains("vlan 10"));
    }

    #[test]
    fn test_mock_vendor_validate_configuration() {
        let mut mock_vendor = MockVendor::new();

        // Set up expectation for validate_configuration
        mock_vendor
            .expect_validate_configuration()
            .times(1)
            .returning(|| Ok(()));

        let result = mock_vendor.validate_configuration();
        assert!(result.is_ok(), "Validation should succeed");
    }

    #[test]
    fn test_mock_vendor_validate_configuration_failure() {
        let mut mock_vendor = MockVendor::new();

        // Set up expectation for validation failure
        mock_vendor
            .expect_validate_configuration()
            .times(1)
            .returning(|| Err(VendorError::ValidationError("Invalid VLAN configuration".to_string())));

        let result = mock_vendor.validate_configuration();
        assert!(result.is_err(), "Validation should fail");
        assert!(matches!(result.unwrap_err(), VendorError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_mock_vendor_full_workflow() {
        let mut mock_vendor = MockVendor::new();

        // Set up expectations for a full configuration workflow
        mock_vendor
            .expect_connect()
            .times(1)
            .returning(|| Ok(()));

        mock_vendor
            .expect_parse_current_state()
            .times(1)
            .returning(|| Ok(SwitchState {
                vlans: vec![],
                ports: vec![],
                port_mirrors: vec![],
                snmp: None,
            management_vlan: None,
            warnings: vec![],
            }));

        mock_vendor
            .expect_apply_configuration()
            .times(1)
            .returning(|| Ok(vec![
                ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: "Configuration applied".to_string(),
                    commands_executed: vec![],
                    timestamp: chrono::Utc::now(),
                },
            ]));

        mock_vendor
            .expect_save_configuration()
            .times(1)
            .returning(|| Ok(()));

        mock_vendor
            .expect_disconnect()
            .times(1)
            .returning(|| Ok(()));

        // Execute workflow
        assert!(mock_vendor.connect().await.is_ok());
        assert!(mock_vendor.parse_current_state().await.is_ok());
        assert!(mock_vendor.apply_configuration().await.is_ok());
        assert!(mock_vendor.save_configuration().await.is_ok());
        assert!(mock_vendor.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_connection_failure_handling() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_connect()
            .times(3)
            .returning(|| Err(VendorError::SshError("Connection timeout".to_string())));

        // Simulate retry logic
        let mut attempts = 0;
        let max_retries = 3;

        while attempts < max_retries {
            if mock_vendor.connect().await.is_ok() {
                break;
            }
            attempts += 1;
        }

        assert_eq!(attempts, max_retries);
    }

    // ========== Speed_Duplex Port Configuration Tests ==========

    #[tokio::test]
    async fn test_mock_vendor_configure_ports_with_speed_duplex() {
        let mut mock_vendor = MockVendor::new();

        let ports = vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("Auto negotiation".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::Auto,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "2".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("100Mbps full duplex".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::HundredFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
            Port {
                port_id: "3".to_string(),
                mode: PortMode::Access,
                vlan: 10,
                tagged_vlans: vec![],
                description: Some("1Gbps".to_string()),
                enabled: true,
                poe_enabled: false,
                mac_notify: false,
                speed_duplex: crate::models::SpeedDuplex::ThousandFull,
                vlan_name: None,
                tagged_vlan_refs: vec![],
            },
        ];

        mock_vendor
            .expect_configure_ports()
            .times(1)
            .returning(|ports| {
                // Verify all speed_duplex settings are present
                assert_eq!(ports[0].speed_duplex, crate::models::SpeedDuplex::Auto);
                assert_eq!(ports[1].speed_duplex, crate::models::SpeedDuplex::HundredFull);
                assert_eq!(ports[2].speed_duplex, crate::models::SpeedDuplex::ThousandFull);
                
                Ok(ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: format!("Configured {} ports with speed_duplex", ports.len()),
                    commands_executed: vec![],
                    timestamp: chrono::Utc::now(),
                })
            });

        let result = mock_vendor.configure_ports(&ports).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_parse_state_with_speed_duplex() {
        let mut mock_vendor = MockVendor::new();

        let expected_state = SwitchState {
            vlans: vec![],
            ports: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 1,
                    tagged_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::Auto,
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
                    speed_duplex: crate::models::SpeedDuplex::HundredFull,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let expected_state_clone = expected_state.clone();

        mock_vendor
            .expect_parse_current_state()
            .times(1)
            .returning(move || Ok(expected_state_clone.clone()));

        let result = mock_vendor.parse_current_state().await;
        assert!(result.is_ok());

        let state = result.unwrap();
        assert_eq!(state.ports.len(), 2);
        assert_eq!(state.ports[0].speed_duplex, crate::models::SpeedDuplex::Auto);
        assert_eq!(state.ports[1].speed_duplex, crate::models::SpeedDuplex::HundredFull);
    }

    #[tokio::test]
    async fn test_mock_vendor_apply_diff_speed_duplex_change() {
        let mut mock_vendor = MockVendor::new();

        let diff = StateDiff {
            vlans_to_add: vec![],
            vlans_to_remove: vec![],
            vlans_to_update: vec![],
            ports_to_configure: vec![
                Port {
                    port_id: "1".to_string(),
                    mode: PortMode::Access,
                    vlan: 10,
                    tagged_vlans: vec![],
                    description: None,
                    enabled: true,
                    poe_enabled: false,
                    mac_notify: false,
                    speed_duplex: crate::models::SpeedDuplex::HundredFull,
                    vlan_name: None,
                    tagged_vlan_refs: vec![],
                },
            ],
            ports_to_reset: vec![],
            mirrors_to_add: vec![],
            mirrors_to_remove: vec![],
            mirrors_to_update: vec![],
            mirror_dest_ports_to_configure: vec![],
            snmp_config_changed: false,
            snmp_config: None,
            snmp_diff: None,
            management_vlan_changed: false,
            management_vlan: None,
        };

        mock_vendor
            .expect_apply_diff()
            .times(1)
            .returning(|diff| {
                // Verify speed_duplex change is present
                assert_eq!(diff.ports_to_configure.len(), 1);
                assert_eq!(diff.ports_to_configure[0].speed_duplex, crate::models::SpeedDuplex::HundredFull);
                
                Ok(vec![ConfigResult {
                    switch: "test-switch".to_string(),
                    success: true,
                    message: "Speed_duplex configured".to_string(),
                    commands_executed: vec!["speed-duplex 100-full".to_string()],
                    timestamp: chrono::Utc::now(),
                }])
            });

        let result = mock_vendor.apply_diff(&diff).await;
        assert!(result.is_ok());

        let results = result.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert!(results[0].commands_executed.contains(&"speed-duplex 100-full".to_string()));
    }

    #[tokio::test]
    async fn test_mock_vendor_get_warnings_empty() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_get_warnings()
            .times(1)
            .returning(|| Vec::new());

        let warnings = mock_vendor.get_warnings();
        assert!(warnings.is_empty());
    }

    #[tokio::test]
    async fn test_mock_vendor_get_warnings_with_model_mismatch() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_get_warnings()
            .times(1)
            .returning(|| vec![
                "Hardware product number mismatch: switch reports J9779A but configured model Aruba2530_24G_POE expects one of [\"J9773A\"]".to_string()
            ]);

        let warnings = mock_vendor.get_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("mismatch"));
        assert!(warnings[0].contains("J9779A"));
    }

    #[tokio::test]
    async fn test_mock_vendor_full_workflow_with_warnings() {
        // Test the full workflow: connect -> apply -> get_warnings -> save -> disconnect
        let mut mock_vendor = MockVendor::new();

        mock_vendor.expect_connect().times(1).returning(|| Ok(()));

        mock_vendor
            .expect_apply_configuration()
            .times(1)
            .returning(|| Ok(vec![ConfigResult {
                switch: "test-switch".to_string(),
                success: true,
                message: "Configured 2 ports".to_string(),
                commands_executed: vec!["configure terminal".to_string()],
                timestamp: chrono::Utc::now(),
            }]));

        mock_vendor
            .expect_get_warnings()
            .times(1)
            .returning(|| vec![
                "Hardware product number mismatch: switch reports JXXXXXA but configured model expects [\"JYYYYYA\"]".to_string()
            ]);

        mock_vendor.expect_save_configuration().times(1).returning(|| Ok(()));
        mock_vendor.expect_disconnect().times(1).returning(|| Ok(()));

        // Simulate the workflow
        assert!(mock_vendor.connect().await.is_ok());

        let results = mock_vendor.apply_configuration().await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);

        let warnings = mock_vendor.get_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("mismatch"));

        assert!(mock_vendor.save_configuration().await.is_ok());
        assert!(mock_vendor.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn test_status_tracker_record_warnings() {
        // Test that warnings are recorded in the StatusTracker and retrievable
        let tracker = crate::status::StatusTracker::new();

        // Initialize with a test switch
        let switches = vec![crate::models::SwitchConfig {
            id: "test-sw".to_string(),
            hostname: Some("test-switch".to_string()),
            model: Some(SwitchModel::Aruba2930F),
            management_ip: Some("192.168.1.1".to_string()),
            credentials: Some(crate::models::Credentials {
                username: "admin".to_string(),
                password: Some("pass".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: crate::models::ConnectionType::Ssh,
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
        }];

        tracker.initialize_switches(&switches).await;

        // Record a warning (keyed by switch id, matching the watcher/API callers)
        let warnings = vec!["Hardware product number mismatch: J9779A vs J9773A".to_string()];
        tracker.record_warnings("test-sw", warnings.clone()).await;

        // Verify via get_status
        let status = tracker.get_status(4000).await;
        let sw = status.switches.iter().find(|s| s.hostname == "test-switch").unwrap();
        assert_eq!(sw.warnings.len(), 1);
        assert!(sw.warnings[0].contains("J9779A"));

        // Record new warnings (should replace, not append)
        tracker.record_warnings("test-sw", vec!["New warning".to_string()]).await;
        let status2 = tracker.get_status(4000).await;
        let sw2 = status2.switches.iter().find(|s| s.hostname == "test-switch").unwrap();
        assert_eq!(sw2.warnings.len(), 1);
        assert!(sw2.warnings[0].contains("New warning"));

        // Clear warnings
        tracker.record_warnings("test-sw", vec![]).await;
        let status3 = tracker.get_status(4000).await;
        let sw3 = status3.switches.iter().find(|s| s.hostname == "test-switch").unwrap();
        assert!(sw3.warnings.is_empty());
    }

    #[test]
    fn test_switch_state_warnings_serialization() {
        // Test that warnings are serialized correctly (and skipped when empty)
        let state_no_warnings = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec![],
        };

        let json = serde_json::to_string(&state_no_warnings).unwrap();
        assert!(!json.contains("warnings"), "Empty warnings should be skipped in serialization");

        let state_with_warnings = SwitchState {
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: None,
            management_vlan: None,
            warnings: vec!["Model mismatch: J9779A".to_string()],
        };

        let json = serde_json::to_string(&state_with_warnings).unwrap();
        assert!(json.contains("warnings"), "Non-empty warnings should be serialized");
        assert!(json.contains("J9779A"), "Warning content should be in JSON");
    }

    #[tokio::test]
    async fn test_mock_vendor_cisco_model_mismatch_warning() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor.expect_connect().times(1).returning(|| Ok(()));
        mock_vendor.expect_apply_configuration().times(1).returning(|| Ok(vec![]));
        mock_vendor.expect_get_warnings().times(1).returning(|| vec![
            "Hardware model mismatch: switch reports c9200-48p but configured model CiscoCatalyst9300_24P_UPOE expects one of [\"c9300-24u\", \"C9300-24U\", \"C9300-24P\"]".to_string()
        ]);
        mock_vendor.expect_disconnect().times(1).returning(|| Ok(()));

        assert!(mock_vendor.connect().await.is_ok());
        let _ = mock_vendor.apply_configuration().await;
        let warnings = mock_vendor.get_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("c9200-48p"));
        assert!(warnings[0].contains("CiscoCatalyst9300_24P_UPOE"));
        assert!(mock_vendor.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_fortiswitch_model_mismatch_warning() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor.expect_connect().times(1).returning(|| Ok(()));
        mock_vendor.expect_apply_configuration().times(1).returning(|| Ok(vec![]));
        mock_vendor.expect_get_warnings().times(1).returning(|| vec![
            "Hardware model mismatch: switch reports FortiSwitch-108F-POE but configured model Fortiswitch124F_FPOE expects one of [\"FortiSwitch-124F-FPOE\", \"S124F\"]".to_string()
        ]);
        mock_vendor.expect_disconnect().times(1).returning(|| Ok(()));

        assert!(mock_vendor.connect().await.is_ok());
        let _ = mock_vendor.apply_configuration().await;
        let warnings = mock_vendor.get_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("FortiSwitch-108F-POE"));
        assert!(warnings[0].contains("Fortiswitch124F_FPOE"));
        assert!(mock_vendor.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_no_model_mismatch() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor.expect_connect().times(1).returning(|| Ok(()));
        mock_vendor.expect_apply_configuration().times(1).returning(|| Ok(vec![]));
        mock_vendor.expect_get_warnings().times(1).returning(Vec::new);
        mock_vendor.expect_disconnect().times(1).returning(|| Ok(()));

        assert!(mock_vendor.connect().await.is_ok());
        let _ = mock_vendor.apply_configuration().await;
        let warnings = mock_vendor.get_warnings();
        assert!(warnings.is_empty(), "No mismatch should produce no warnings");
        assert!(mock_vendor.disconnect().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_vendor_rollback_restore_backup() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_rollback_configuration()
            .times(1)
            .withf(|method| matches!(method, RollbackMethod::RestoreBackup))
            .returning(|_| Ok(()));

        let result = mock_vendor.rollback_configuration(RollbackMethod::RestoreBackup).await;
        assert!(result.is_ok(), "Rollback with RestoreBackup should succeed");
    }

    #[tokio::test]
    async fn test_mock_vendor_rollback_reload() {
        let mut mock_vendor = MockVendor::new();

        mock_vendor
            .expect_rollback_configuration()
            .times(1)
            .withf(|method| matches!(method, RollbackMethod::Reload))
            .returning(|_| Ok(()));

        let result = mock_vendor.rollback_configuration(RollbackMethod::Reload).await;
        assert!(result.is_ok(), "Rollback with Reload should succeed");
    }
}
