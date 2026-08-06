use super::handlers::*;
use crate::config::{AppConfig, ConfigStore, Settings, SseEvent};
use crate::models::{
    ConnectionType, Credentials, Port, PortMode, SpeedDuplex, SwitchConfig, SwitchModel, Vlan,
    VlanIpConfig,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;
use tower::ServiceExt; // for `oneshot` method

async fn create_test_config_store_async() -> ConfigStore {
    let switch1 = SwitchConfig {
        id: "test-sw-01".to_string(),
        hostname: Some("test-switch-1".to_string()),
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
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ],
        ports: vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
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
        validation: None,
        settings: Settings::default(),
        vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
    };

    let switch2 = SwitchConfig {
        id: "test-sw-02".to_string(),
        hostname: Some("test-switch-2".to_string()),
        model: Some(SwitchModel::CiscoCatalyst9300_24P_UPOE),
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
        vlans: vec![
            Vlan {
                id: 20,
                name: "vlan20".to_string(),
                description: None,
                ip_config: VlanIpConfig::Dhcp,
            },
        ],
        ports: vec![],
        port_mirrors: vec![],
        snmp: None,
        validation: None,
        settings: Settings::default(),
        vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,

    };

    let app_config = AppConfig {
        switches: vec![switch1.clone(), switch2.clone()],
    };

    let store = ConfigStore::new(app_config, 4002);

    // Initialize the status tracker with the switches
    store.status.initialize_switches(&vec![switch1, switch2]).await;

    store
}

fn create_test_config_store() -> ConfigStore {
    // Synchronous version for tests that don't need status tracking
    let switch1 = SwitchConfig {
        id: "test-sw-01".to_string(),
        hostname: Some("test-switch-1".to_string()),
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
                name: "vlan10".to_string(),
                description: None,
                ip_config: VlanIpConfig::None,
            },
        ],
        ports: vec![
            Port {
                port_id: "1".to_string(),
                mode: PortMode::Access,
                vlan: 10,
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
        validation: None,
        settings: Settings::default(),
        vendor_specific: std::collections::HashMap::new(),
        management_vlan: None,

    };

    let switch2 = SwitchConfig {
        id: "test-sw-02".to_string(),
        hostname: Some("test-switch-2".to_string()),
        model: Some(SwitchModel::CiscoCatalyst9300_24P_UPOE),
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
        vlans: vec![
            Vlan {
                id: 20,
                name: "vlan20".to_string(),
                description: None,
                ip_config: VlanIpConfig::Dhcp,
            },
        ],
        ports: vec![],
        port_mirrors: vec![],
        snmp: None,
        validation: None,
        settings: Settings::default(),
        vendor_specific: std::collections::HashMap::new(),
        management_vlan: None,

    };

    let switch3 = SwitchConfig {
        id: "test-sw-03".to_string(),
        hostname: Some("test-switch-3".to_string()),
        model: Some(SwitchModel::Aruba2540_24G),
        management_ip: Some("192.168.1.3".to_string()),
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
        ports: vec![
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
        ],
        port_mirrors: vec![],
        snmp: None,
        validation: None,
        settings: Settings::default(),
        vendor_specific: std::collections::HashMap::new(),
        management_vlan: None,
    };

    let app_config = AppConfig {
        switches: vec![switch1, switch2, switch3],
    };

    ConfigStore::new(app_config, 4002)
}

#[tokio::test]
async fn test_health_endpoint() {
    let response = health().await.into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "switch-configurator");
}

#[tokio::test]
async fn test_list_switches() {
    let store = create_test_config_store();
    let response = list_switches(axum::extract::State(store))
        .await
        .into_response();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["count"], 3);
    assert!(json["switches"].is_array());

    let switches = json["switches"].as_array().unwrap();
    assert_eq!(switches.len(), 3);

    // Check first switch - verify both id and hostname are present
    assert_eq!(switches[0]["id"], "test-sw-01");
    assert_eq!(switches[0]["hostname"], "test-switch-1");
    assert_eq!(switches[0]["management_ip"], "192.168.1.1");
    assert_eq!(switches[0]["vlans"], 1);
    assert_eq!(switches[0]["ports"], 1);

    // Check second switch - verify both id and hostname are present
    assert_eq!(switches[1]["id"], "test-sw-02");
    assert_eq!(switches[1]["hostname"], "test-switch-2");
    assert_eq!(switches[1]["management_ip"], "192.168.1.2");
    assert_eq!(switches[1]["vlans"], 1);
    assert_eq!(switches[1]["ports"], 0);
}

#[tokio::test]
async fn test_get_switch_config_not_found() {
    let store = create_test_config_store();
    let response = get_config(
        axum::extract::State(store),
        axum::extract::Path("nonexistent-switch".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Verify error message specifically references "id" (not "hostname")
    let error_msg = json["error"].as_str().unwrap();
    assert!(error_msg.contains("not found"));
    assert!(error_msg.contains("id"));
    assert!(error_msg.contains("nonexistent-switch"));
}

#[tokio::test]
async fn test_get_config_by_valid_id_not_found() {
    // Test that we can lookup by valid ID (will fail to connect but that's expected)
    let store = create_test_config_store();
    let response = get_config(
        axum::extract::State(store),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    // Will return 500 because we can't actually connect to the switch
    // This is expected - we're just verifying the ID lookup works
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_apply_config_not_found() {
    let store = create_test_config_store();
    let response = apply_config(
        axum::extract::State(store),
        axum::extract::Path("nonexistent-id".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Verify error message specifically references "id" (not "hostname")
    let error_msg = json["error"].as_str().unwrap();
    assert!(error_msg.contains("not found"));
    assert!(error_msg.contains("id"));
    assert!(error_msg.contains("nonexistent-id"));
}

#[tokio::test]
async fn test_apply_config_by_valid_id() {
    // Test that apply_config can lookup by valid ID
    let store = create_test_config_store();
    let response = apply_config(
        axum::extract::State(store),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    // Returns 202 Accepted because apply is always async
    // The actual connection happens in background task
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn test_apply_config_by_hostname_fails() {
    // BREAKING CHANGE TEST: Verify that using hostname (instead of ID) fails
    let store = create_test_config_store();
    let response = apply_config(
        axum::extract::State(store),
        axum::extract::Path("test-switch-1".to_string()), // This is a hostname, not an ID
    )
    .await
    .into_response();

    // Should return 404 because hostname is not the same as ID
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Verify error message references the hostname we passed
    let error_msg = json["error"].as_str().unwrap();
    assert!(error_msg.contains("test-switch-1"));
    assert!(error_msg.contains("not found"));
}

#[tokio::test]
async fn test_apply_config_returns_202_with_correct_response() {
    // Test that apply returns 202 Accepted with correct JSON structure
    let store = create_test_config_store();
    let response = apply_config(
        axum::extract::State(store),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Verify response structure
    assert_eq!(json["status"], "accepted");
    assert_eq!(json["switch_id"], "test-sw-01");
    assert!(json["message"].as_str().unwrap().contains("test-sw-01"));
    assert_eq!(json["poll_url"], "/api/status");
    assert!(json["hint"].as_str().is_some());
}

#[tokio::test]
async fn test_apply_config_conflict_same_switch() {
    // Test that applying to the same switch twice returns 409 Conflict
    let store = create_test_config_store();

    // Simulate that the switch is already being configured
    store.status.set_currently_configuring("test-sw-01".to_string()).await;

    let response = apply_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"].as_str().unwrap().contains("already being configured"));
    assert_eq!(json["switch_id"], "test-sw-01");
}

#[tokio::test]
async fn test_apply_config_no_conflict_different_switch() {
    // Test that applying to a different switch doesn't conflict
    let store = create_test_config_store();

    // Simulate that switch-01 is being configured
    store.status.set_currently_configuring("test-sw-01".to_string()).await;

    // Apply to switch-02 should succeed (return 202, not 409)
    let response = apply_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-02".to_string()),
    )
    .await
    .into_response();

    // Should be 202 Accepted, not 409 Conflict
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn test_get_config_conflict_when_apply_in_progress() {
    // Test that get_config returns 409 when apply is in progress for the same switch
    let store = create_test_config_store();

    // Simulate that the switch is being configured
    store.status.set_currently_configuring("test-sw-01".to_string()).await;

    let response = get_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"].as_str().unwrap().contains("currently being configured"));
    assert_eq!(json["switch_id"], "test-sw-01");
}

#[tokio::test]
async fn test_get_config_no_conflict_different_switch() {
    // Test that get_config for a different switch doesn't conflict
    let store = create_test_config_store();

    // Simulate that switch-01 is being configured
    store.status.set_currently_configuring("test-sw-01".to_string()).await;

    // Get config for switch-02 should not return 409
    // (will return 500 because we can't connect, but that's expected)
    let response = get_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-02".to_string()),
    )
    .await
    .into_response();

    // Should NOT be 409 Conflict (will be 500 due to connection failure, which is fine)
    assert_ne!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_apply_config_conflict_pending_reload() {
    // Test that apply returns 409 when switch has a pending config reload queued
    let store = create_test_config_store();

    // Queue a pending reload for this switch
    store.status.queue_pending_reload("test-sw-01".to_string()).await;

    let response = apply_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"].as_str().unwrap().contains("pending config reload"));
    assert_eq!(json["switch_id"], "test-sw-01");
}

#[tokio::test]
async fn test_get_config_conflict_pending_reload() {
    // Test that get_config returns 409 when switch has a pending config reload queued
    let store = create_test_config_store();

    // Queue a pending reload for this switch
    store.status.queue_pending_reload("test-sw-01".to_string()).await;

    let response = get_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"].as_str().unwrap().contains("pending config reload"));
    assert_eq!(json["switch_id"], "test-sw-01");
}

#[tokio::test]
async fn test_reload_config_no_config_paths() {
    // Test that reload_config returns 500 when config paths are not set
    let store = create_test_config_store();
    // Note: store doesn't have config_metadata set, so get_config_paths() returns None

    let response = reload_config(axum::extract::State(store))
        .await
        .into_response();

    // Should return 500 because config paths are not available
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"].as_str().unwrap().contains("Configuration metadata not available"));
}

#[tokio::test]
async fn test_reload_switch_config_no_config_paths() {
    // Test that reload_switch_config returns 500 when config paths are not set
    let store = create_test_config_store();
    // Note: store doesn't have config_metadata set, so get_config_paths() returns None

    let response = reload_switch_config(
        axum::extract::State(store),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("Configuration metadata not available"));
}

#[tokio::test]
async fn test_reload_switch_config_conflict_when_busy() {
    // Test that reload_switch_config returns 409 when switch is currently being configured
    use std::path::PathBuf;
    use crate::status::ConfigMetadata;
    use chrono::Utc;

    let store = create_test_config_store();

    // Set config metadata so the endpoint doesn't fail early
    store.status.set_config_metadata(ConfigMetadata {
        config_file: PathBuf::from("/nonexistent/config.yaml"),
        config_folders: vec![],
        last_loaded: Utc::now(),
        switches_count: 2,
    }).await;

    // Mark switch as being configured
    store.status.set_currently_configuring("test-sw-01".to_string()).await;

    let response = reload_switch_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    // Should return 409 CONFLICT before trying to reload YAML
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("is already being configured"));
    assert_eq!(json["switch_id"], "test-sw-01");

    // Clean up
    store.status.clear_currently_configuring("test-sw-01").await;
}

#[tokio::test]
async fn test_reload_switch_config_conflict_pending_reload() {
    // Test that reload_switch_config returns 409 when switch has pending reload
    use std::path::PathBuf;
    use crate::status::ConfigMetadata;
    use chrono::Utc;

    let store = create_test_config_store();

    // Set config metadata
    store.status.set_config_metadata(ConfigMetadata {
        config_file: PathBuf::from("/nonexistent/config.yaml"),
        config_folders: vec![],
        last_loaded: Utc::now(),
        switches_count: 2,
    }).await;

    // Queue pending reload for the switch
    store.status.queue_pending_reload("test-sw-01".to_string()).await;

    let response = reload_switch_config(
        axum::extract::State(store.clone()),
        axum::extract::Path("test-sw-01".to_string()),
    )
    .await
    .into_response();

    // Should return 409 CONFLICT before trying to reload YAML
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("has a pending config reload queued"));
    assert_eq!(json["switch_id"], "test-sw-01");
}

#[tokio::test]
async fn test_status_endpoint() {
    let store = create_test_config_store_async().await;
    let response = status(axum::extract::State(store))
        .await
        .into_response();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Verify top-level fields
    assert_eq!(json["service"], "switch-configurator");
    assert!(json["version"].is_string());
    assert!(json["status"].is_string());
    assert!(json["uptime_seconds"].is_number());

    // Verify currently_configuring field exists and is empty array (no config in progress)
    assert!(json.get("currently_configuring").is_some());
    assert!(json["currently_configuring"].is_array());
    assert_eq!(json["currently_configuring"].as_array().unwrap().len(), 0);

    // Verify pending_config_reload field exists and is empty array (no pending reloads)
    assert!(json.get("pending_config_reload").is_some());
    assert!(json["pending_config_reload"].is_array());
    assert_eq!(json["pending_config_reload"].as_array().unwrap().len(), 0);

    // Verify configuration section
    assert!(json["configuration"].is_object());
    assert!(json["configuration"]["loaded"].is_boolean());
    assert!(json["configuration"]["switches_count"].is_number());

    // Verify API section
    assert!(json["api"].is_object());
    assert_eq!(json["api"]["port"], 4002);
    assert!(json["api"]["endpoints"].is_array());

    // Verify endpoints list uses :id (not :hostname)
    let endpoints = json["api"]["endpoints"].as_array().unwrap();
    let apply_endpoint = endpoints.iter()
        .find(|e| e.as_str().unwrap().contains("/switches/"))
        .unwrap()
        .as_str()
        .unwrap();
    assert!(apply_endpoint.contains(":id"), "Endpoint should use :id not :hostname");
    assert!(!apply_endpoint.contains(":hostname"), "Endpoint should not use :hostname");

    // Verify switches array exists and contains all switches
    assert!(json["switches"].is_array());
    let switches = json["switches"].as_array().unwrap();
    assert_eq!(switches.len(), 2, "Should have 2 switches");

    // Verify each switch has required status fields
    for switch in switches {
        assert!(switch["id"].is_string());
        assert!(switch["hostname"].is_string());
        assert!(switch["model"].is_string());
        assert!(switch["management_ip"].is_string());
        assert!(switch["connection_type"].is_string());
        assert!(switch["apply_count"].is_number());
        assert!(switch["success_count"].is_number());
        assert!(switch["failure_count"].is_number());
        // last_applied and last_result can be null initially
    }

    // Verify specific switch IDs are present
    let switch_ids: Vec<&str> = switches.iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(switch_ids.contains(&"test-sw-01"));
    assert!(switch_ids.contains(&"test-sw-02"));

    // Verify recent_errors array exists
    assert!(json["recent_errors"].is_array());

    // Verify runtime section
    assert!(json["runtime"].is_object());
    assert_eq!(json["runtime"]["mode"], "service");
}

#[tokio::test]
async fn test_status_endpoint_switch_details() {
    // Test that status includes detailed information for each switch
    let store = create_test_config_store_async().await;
    let response = status(axum::extract::State(store))
        .await
        .into_response();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    let switches = json["switches"].as_array().unwrap();

    // Find test-sw-01 and verify its details
    let sw1 = switches.iter()
        .find(|s| s["id"] == "test-sw-01")
        .expect("Should find test-sw-01");

    assert_eq!(sw1["hostname"], "test-switch-1");
    assert_eq!(sw1["management_ip"], "192.168.1.1");
    assert_eq!(sw1["model"], "Aruba2930F");
    assert_eq!(sw1["connection_type"], "Ssh");

    // Find test-sw-02 and verify its details
    let sw2 = switches.iter()
        .find(|s| s["id"] == "test-sw-02")
        .expect("Should find test-sw-02");

    assert_eq!(sw2["hostname"], "test-switch-2");
    assert_eq!(sw2["management_ip"], "192.168.1.2");
    assert_eq!(sw2["model"], "CiscoCatalyst9300_24P_UPOE");
    assert_eq!(sw2["connection_type"], "Ssh");
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::api;

    #[tokio::test]
    async fn test_api_routes_health() {
        let store = create_test_config_store();

        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_routes_list_switches() {
        let store = create_test_config_store();

        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/switches")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["count"], 3);
    }

    #[tokio::test]
    async fn test_api_routes_get_config_by_id() {
        // Test that the route /switches/:id/config works with ID parameter
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/switches/test-sw-01/config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Will return 500 because we can't connect, but route resolved correctly
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_api_routes_get_config_by_id_not_found() {
        // Test that the route /switches/:id/config returns 404 for invalid ID
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/switches/nonexistent-id/config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify error message references "id"
        let error_msg = json["error"].as_str().unwrap();
        assert!(error_msg.contains("id"));
        assert!(error_msg.contains("nonexistent-id"));
    }

    #[tokio::test]
    async fn test_api_routes_apply_config_by_id() {
        // Test that the route /switches/{id}/apply works with ID parameter
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-02/apply")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Returns 202 Accepted because apply is always async
        // Actual connection happens in background task
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_api_routes_apply_config_by_id_not_found() {
        // Test that the route /switches/:id/apply returns 404 for invalid ID
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .method("POST")
            .uri("/switches/invalid-id/apply")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify error message references "id"
        let error_msg = json["error"].as_str().unwrap();
        assert!(error_msg.contains("id"));
        assert!(error_msg.contains("invalid-id"));
    }

    #[tokio::test]
    async fn test_api_routes_apply_config_by_hostname_fails() {
        // BREAKING CHANGE TEST: Verify that using hostname fails with full routing
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-switch-1/apply") // hostname, not ID
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 404 because hostname != ID
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_routes_status() {
        // Test that the route /api/status works correctly
        let store = create_test_config_store_async().await;
        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/api/status")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify the response structure
        assert_eq!(json["service"], "switch-configurator");
        assert!(json["currently_configuring"].is_array());
        assert_eq!(json["currently_configuring"].as_array().unwrap().len(), 0);
        assert!(json["switches"].is_array());

        // Verify all switches are included
        let switches = json["switches"].as_array().unwrap();
        assert_eq!(switches.len(), 2);

        // Verify endpoints use :id
        let endpoints = json["api"]["endpoints"].as_array().unwrap();
        let switch_endpoints: Vec<&str> = endpoints.iter()
            .filter_map(|e| e.as_str())
            .filter(|e| e.contains("/switches/"))
            .collect();

        for endpoint in switch_endpoints {
            assert!(endpoint.contains(":id"), "Endpoint '{}' should use :id", endpoint);
            assert!(!endpoint.contains(":hostname"), "Endpoint '{}' should not use :hostname", endpoint);
        }
    }

    // ========== Tests for new config API endpoints ==========

    #[tokio::test]
    async fn test_get_desired_config_success() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/switches/test-sw-01/desired-config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["id"], "test-sw-01");
        assert_eq!(json["hostname"], "test-switch-1");
        assert!(json["vlans"].is_array());
        assert_eq!(json["vlans"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_get_desired_config_not_found() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .uri("/switches/nonexistent/desired-config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_set_switch_config_create_new() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let new_switch = serde_json::json!({
            "id": "new-switch-01",
            "hostname": "new-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.100",
            "credentials": {
                "username": "admin",
                "password": "secret"
            },
            "vlans": [
                {"id": 100, "name": "test-vlan"}
            ],
            "ports": []
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/new-switch-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify switch was created
        let config = store.config.read().await;
        assert_eq!(config.switches.len(), 4);
        let new_sw = config.switches.iter().find(|s| s.id == "new-switch-01");
        assert!(new_sw.is_some());
        assert_eq!(new_sw.unwrap().hostname, Some("new-switch".to_string()));
    }

    #[tokio::test]
    async fn test_set_switch_config_replace_existing() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let updated_switch = serde_json::json!({
            "id": "test-sw-01",
            "hostname": "updated-hostname",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.1",
            "credentials": {
                "username": "admin",
                "password": "newpassword"
            },
            "vlans": [
                {"id": 200, "name": "new-vlan"}
            ],
            "ports": []
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(updated_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify switch was replaced
        let config = store.config.read().await;
        assert_eq!(config.switches.len(), 3); // Still 3 switches
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.hostname, Some("updated-hostname".to_string()));
        assert_eq!(sw.vlans.len(), 1);
        assert_eq!(sw.vlans[0].id, 200);
    }

    #[tokio::test]
    async fn test_set_switch_config_id_mismatch() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let mismatched = serde_json::json!({
            "id": "different-id",
            "hostname": "test",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.1",
            "credentials": {"username": "admin", "password": "pass"}
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(mismatched.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("mismatch"));
    }

    #[tokio::test]
    async fn test_set_switch_config_new_missing_required() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        // Missing hostname, model, management_ip, credentials
        let incomplete = serde_json::json!({
            "id": "new-incomplete",
            "vlans": []
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/new-incomplete/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(incomplete.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let error = json["error"].as_str().unwrap();
        assert!(error.contains("hostname"));
        assert!(error.contains("model"));
        assert!(error.contains("management_ip"));
        assert!(error.contains("credentials"));
    }

    #[tokio::test]
    async fn test_patch_switch_config_update_hostname() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let patch = serde_json::json!({
            "id": "test-sw-01",
            "hostname": "patched-hostname"
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify hostname was updated but other fields preserved
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.hostname, Some("patched-hostname".to_string()));
        assert_eq!(sw.vlans.len(), 1); // Original VLAN preserved
        assert_eq!(sw.vlans[0].id, 10);
    }

    #[tokio::test]
    async fn test_patch_switch_config_add_vlan() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let patch = serde_json::json!({
            "id": "test-sw-01",
            "vlans": [
                {"id": 50, "name": "new-vlan-50"}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify new VLAN was added and existing preserved
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.vlans.len(), 2);
        assert!(sw.vlans.iter().any(|v| v.id == 10)); // Original
        assert!(sw.vlans.iter().any(|v| v.id == 50)); // New
    }

    #[tokio::test]
    async fn test_patch_switch_config_update_existing_vlan() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let patch = serde_json::json!({
            "id": "test-sw-01",
            "vlans": [
                {"id": 10, "name": "updated-vlan10-name"}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify VLAN was updated
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.vlans.len(), 1);
        assert_eq!(sw.vlans[0].id, 10);
        assert_eq!(sw.vlans[0].name, "updated-vlan10-name");
    }

    #[tokio::test]
    async fn test_patch_switch_config_not_found() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let patch = serde_json::json!({
            "id": "nonexistent",
            "hostname": "test"
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/nonexistent/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_patch_switch_config_id_mismatch() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let patch = serde_json::json!({
            "id": "different-id",
            "hostname": "test"
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_switch_config_success() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // Verify we start with 3 switches
        {
            let config = store.config.read().await;
            assert_eq!(config.switches.len(), 3);
        }

        let request = Request::builder()
            .method("DELETE")
            .uri("/switches/test-sw-01/desired-config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify switch was deleted
        let config = store.config.read().await;
        assert_eq!(config.switches.len(), 2);
        assert!(config.switches.iter().all(|s| s.id != "test-sw-01"));
    }

    #[tokio::test]
    async fn test_delete_switch_config_not_found() {
        let store = create_test_config_store();
        let app = api::create_router(store);

        let request = Request::builder()
            .method("DELETE")
            .uri("/switches/nonexistent/desired-config")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_patch_switch_config_add_port() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let patch = serde_json::json!({
            "id": "test-sw-01",
            "ports": [
                {"port_id": "2", "mode": "access", "vlan": 10, "enabled": true, "poe_enabled": false}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify new port was added
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.ports.len(), 2);
        assert!(sw.ports.iter().any(|p| p.port_id == "1")); // Original
        assert!(sw.ports.iter().any(|p| p.port_id == "2")); // New
    }

    // ========== Tests for optional id in body ==========

    #[tokio::test]
    async fn test_put_without_id_in_body() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // No "id" field in body - should use URL parameter
        let new_switch = serde_json::json!({
            "hostname": "no-id-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.200",
            "credentials": {"username": "admin", "password": "secret"}
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/no-id-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify switch was created with URL id
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "no-id-test");
        assert!(sw.is_some());
        assert_eq!(sw.unwrap().hostname, Some("no-id-switch".to_string()));
    }

    #[tokio::test]
    async fn test_patch_without_id_in_body() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // No "id" field in body - should use URL parameter
        let patch = serde_json::json!({
            "hostname": "patched-without-id"
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify hostname was updated
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.hostname, Some("patched-without-id".to_string()));
    }

    #[tokio::test]
    async fn test_put_with_matching_id_in_body() {
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // "id" in body matches URL - should work
        let new_switch = serde_json::json!({
            "id": "matching-id-test",
            "hostname": "matching-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.201",
            "credentials": {"username": "admin", "password": "secret"}
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/matching-id-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // ========== Validation Parity Tests ==========
    // These tests ensure API requests go through the same validation as file-based configs

    #[tokio::test]
    async fn test_put_port_range_expansion() {
        // Validation parity: Port ranges should be expanded just like file-based configs
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let new_switch = serde_json::json!({
            "hostname": "range-test-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.50",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "test-vlan"}],
            "ports": [
                {"port_id": "1-3", "mode": "access", "vlan": 100, "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/range-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify port range was expanded to 3 individual ports
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "range-test").unwrap();
        assert_eq!(sw.ports.len(), 3, "Port range '1-3' should expand to 3 ports");
        assert!(sw.ports.iter().any(|p| p.port_id == "1"));
        assert!(sw.ports.iter().any(|p| p.port_id == "2"));
        assert!(sw.ports.iter().any(|p| p.port_id == "3"));
    }

    #[tokio::test]
    async fn test_put_port_range_expansion_complex() {
        // Validation parity: Complex port ranges like "1-3,5,7-9" should work
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let new_switch = serde_json::json!({
            "hostname": "complex-range-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.51",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "test-vlan"}],
            "ports": [
                {"port_id": "1-3,5,7-9", "mode": "access", "vlan": 100, "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/complex-range-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify port range was expanded: 1,2,3,5,7,8,9 = 7 ports
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "complex-range-test").unwrap();
        assert_eq!(sw.ports.len(), 7, "Port range '1-3,5,7-9' should expand to 7 ports");

        let port_ids: Vec<&str> = sw.ports.iter().map(|p| p.port_id.as_str()).collect();
        assert!(port_ids.contains(&"1"));
        assert!(port_ids.contains(&"2"));
        assert!(port_ids.contains(&"3"));
        assert!(port_ids.contains(&"5"));
        assert!(port_ids.contains(&"7"));
        assert!(port_ids.contains(&"8"));
        assert!(port_ids.contains(&"9"));
        assert!(!port_ids.contains(&"4")); // 4 and 6 should not be included
        assert!(!port_ids.contains(&"6"));
    }

    #[tokio::test]
    async fn test_put_invalid_vlan_reference() {
        // Validation parity: Ports referencing non-existent VLANs should fail
        let store = create_test_config_store();
        let app = api::create_router(store);

        let invalid_switch = serde_json::json!({
            "hostname": "invalid-vlan-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.52",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "existing-vlan"}],
            "ports": [
                {"port_id": "1", "mode": "access", "vlan": 999, "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/invalid-vlan-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(invalid_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Should indicate validation failure related to VLAN
        assert!(json["error"].as_str().unwrap().contains("validation"));
        let details = json["details"].as_str().unwrap_or("");
        assert!(details.contains("999") || details.to_lowercase().contains("vlan"),
            "Error should mention the invalid VLAN reference, got: {}", details);
    }

    #[tokio::test]
    async fn test_put_trunk_vlan_filtering() {
        // Validation parity: Trunk ports with invalid tagged_vlans should be filtered (not rejected)
        // This matches the file-based config behavior which filters invalid VLANs and logs warnings
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let switch_with_invalid_vlans = serde_json::json!({
            "hostname": "trunk-filter-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.53",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "existing-vlan"}],
            "ports": [
                {"port_id": "1", "mode": "trunk", "vlan": 100, "tagged_vlans": [100, 200, 300], "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/trunk-filter-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(switch_with_invalid_vlans.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Should succeed - invalid VLANs are filtered out, not rejected
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify that invalid VLANs (200, 300) were filtered out
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "trunk-filter-test").unwrap();
        let port = &sw.ports[0];

        // Only VLAN 100 should remain (200 and 300 filtered out)
        assert_eq!(port.tagged_vlans.len(), 1, "Invalid VLANs should be filtered out");
        assert_eq!(port.tagged_vlans, vec![100], "Only valid VLAN 100 should remain");
    }

    #[tokio::test]
    async fn test_put_invalid_speed_duplex_for_model() {
        // Validation parity: Invalid speed/duplex for switch model should fail
        let store = create_test_config_store();
        let app = api::create_router(store);

        // Aruba2530_24G_POE doesn't support 10G
        let invalid_switch = serde_json::json!({
            "hostname": "invalid-speed-switch",
            "model": "Aruba2530_24G_POE",
            "management_ip": "192.168.1.54",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "test-vlan"}],
            "ports": [
                {"port_id": "1", "mode": "access", "vlan": 100, "enabled": true, "speed_duplex": "10g-full"}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/invalid-speed-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(invalid_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert!(json["error"].as_str().unwrap().contains("validation"));
        let details = json["details"].as_str().unwrap_or("");
        assert!(details.to_lowercase().contains("speed") || details.contains("10g"),
            "Error should mention speed/duplex issue, got: {}", details);
    }

    #[tokio::test]
    async fn test_put_valid_speed_duplex_for_model() {
        // Validation parity: Valid speed/duplex for switch model should succeed
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // Aruba2930F supports 10G on uplinks
        let valid_switch = serde_json::json!({
            "hostname": "valid-speed-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.55",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "test-vlan"}],
            "ports": [
                {"port_id": "1", "mode": "access", "vlan": 100, "enabled": true, "speed_duplex": "1000-full"}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/valid-speed-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(valid_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify the speed_duplex was preserved
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "valid-speed-test").unwrap();
        assert_eq!(sw.ports[0].speed_duplex, SpeedDuplex::ThousandFull);
    }

    #[tokio::test]
    async fn test_patch_port_range_expansion() {
        // Validation parity: PATCH should also expand port ranges
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let patch = serde_json::json!({
            "ports": [
                {"port_id": "5-8", "mode": "access", "vlan": 10, "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Verify port range was expanded (original port 1 + new ports 5,6,7,8)
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.ports.len(), 5, "Should have original port 1 plus expanded ports 5-8");
        assert!(sw.ports.iter().any(|p| p.port_id == "1")); // Original
        assert!(sw.ports.iter().any(|p| p.port_id == "5"));
        assert!(sw.ports.iter().any(|p| p.port_id == "6"));
        assert!(sw.ports.iter().any(|p| p.port_id == "7"));
        assert!(sw.ports.iter().any(|p| p.port_id == "8"));
    }

    #[tokio::test]
    async fn test_patch_invalid_vlan_reference() {
        // Validation parity: PATCH with invalid VLAN reference should fail and rollback
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        // Get original state
        let original_hostname = {
            let config = store.config.read().await;
            let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
            sw.hostname.clone()
        };

        let patch = serde_json::json!({
            "hostname": "should-not-persist",
            "ports": [
                {"port_id": "99", "mode": "access", "vlan": 9999, "enabled": true}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Verify rollback - hostname should NOT have changed
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "test-sw-01").unwrap();
        assert_eq!(sw.hostname, original_hostname, "Hostname should be rolled back on validation failure");
    }

    #[tokio::test]
    async fn test_patch_invalid_speed_duplex() {
        // Validation parity: PATCH with invalid speed/duplex should fail
        let store = create_test_config_store();
        let app = api::create_router(store);

        // test-sw-01 is Aruba2930F which supports 10G, but let's test with test-sw-02 (Cisco)
        // Actually, let's just verify the validation runs
        let patch = serde_json::json!({
            "ports": [
                {"port_id": "1", "mode": "access", "vlan": 10, "enabled": true, "speed_duplex": "invalid-speed"}
            ]
        });

        let request = Request::builder()
            .method("PATCH")
            .uri("/switches/test-sw-01/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(patch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Should fail because "invalid-speed" is not a valid SpeedDuplex variant
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_put_mirror_source_port_range_expansion() {
        // Validation parity: Port mirrors should also expand source port ranges
        let store = create_test_config_store();
        let app = api::create_router(store.clone());

        let new_switch = serde_json::json!({
            "hostname": "mirror-range-switch",
            "model": "Aruba2930F",
            "management_ip": "192.168.1.60",
            "credentials": {"username": "admin", "password": "secret"},
            "vlans": [{"id": 100, "name": "test-vlan"}],
            "ports": [
                {"port_id": "1-5", "mode": "access", "vlan": 100, "enabled": true}
            ],
            "port_mirrors": [
                {"session_id": "1", "source_ports": ["1-3"], "destination_port": "10", "direction": "both"}
            ]
        });

        let request = Request::builder()
            .method("PUT")
            .uri("/switches/mirror-range-test/desired-config")
            .header("Content-Type", "application/json")
            .body(Body::from(new_switch.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        // Verify source ports were expanded
        let config = store.config.read().await;
        let sw = config.switches.iter().find(|s| s.id == "mirror-range-test").unwrap();
        assert_eq!(sw.port_mirrors.len(), 1);

        let mirror = &sw.port_mirrors[0];
        assert_eq!(mirror.source_ports.len(), 3, "Source ports '1-3' should expand to 3 ports");
        assert!(mirror.source_ports.contains(&"1".to_string()));
        assert!(mirror.source_ports.contains(&"2".to_string()));
        assert!(mirror.source_ports.contains(&"3".to_string()));
    }

    #[tokio::test]
    async fn test_reload_switch_config_success() {
        // Test that reload_switch_config returns 202 Accepted when config is valid
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file: config_file.clone(),
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        let app = api::create_router(store.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 202 Accepted (async processing)
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify response structure
        assert_eq!(json["status"], "accepted");
        assert!(json["message"].as_str().unwrap().contains("reload and apply started"));
        assert_eq!(json["switch_id"], "test-sw-01");
        assert_eq!(json["poll_url"], "/api/status");
        assert!(json["hint"].as_str().unwrap().contains("Poll /api/status"));
    }

    #[tokio::test]
    async fn test_reload_switch_config_not_found() {
        // Test that reload_switch_config returns 404 for non-existent switch
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file: config_file.clone(),
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        let app = api::create_router(store.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/switches/nonexistent-switch/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 404 Not Found because switch doesn't exist in YAML
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert!(json["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_reload_switch_config_via_router() {
        // Test the route integration for /switches/:id/reload
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file,
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        let app = api::create_router(store);

        // Test with the switch ID from the fixture file
        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 202 Accepted
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_global_reload_config_success() {
        // Test that POST /config/reload returns 202 Accepted when config is valid
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file: config_file.clone(),
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        let app = api::create_router(store.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/config/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 202 Accepted (async processing)
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify response structure
        assert_eq!(json["status"], "accepted");
        assert!(json["message"].as_str().unwrap().contains("reload started"));
        assert!(json["switches_configuring"].is_array());
        assert!(json["switches_skipped"].is_array());
        assert_eq!(json["poll_url"], "/api/status");
        assert!(json["hint"].as_str().unwrap().contains("Poll /api/status"));
    }

    #[tokio::test]
    async fn test_global_reload_config_skips_busy_switches() {
        // Test that /config/reload skips switches that are already being configured
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file: config_file.clone(),
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        // Mark the switch as busy (being configured)
        store.status.set_currently_configuring("test-sw-01".to_string()).await;

        let app = api::create_router(store.clone());

        let request = Request::builder()
            .method("POST")
            .uri("/config/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should still return 202 Accepted (the reload proceeds, just skips busy switches)
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        // Verify that the busy switch is in the skipped list
        let skipped = json["switches_skipped"].as_array().unwrap();
        assert!(skipped.iter().any(|s| s.as_str() == Some("test-sw-01")));

        // Clean up
        store.status.clear_currently_configuring("test-sw-01").await;
    }

    #[tokio::test]
    async fn test_global_reload_config_via_router() {
        // Test the route integration for POST /config/reload
        use std::path::PathBuf;
        use crate::status::ConfigMetadata;
        use chrono::Utc;

        let store = create_test_config_store_async().await;

        // Set config metadata pointing to a real fixture file
        let config_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi-config/basic/main.yaml");

        store.status.set_config_metadata(ConfigMetadata {
            config_file,
            config_folders: vec![],
            last_loaded: Utc::now(),
            switches_count: 1,
        }).await;

        let app = api::create_router(store);

        let request = Request::builder()
            .method("POST")
            .uri("/config/reload")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 202 Accepted
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_unix_socket_health_endpoint() {
        let store = create_test_config_store();
        let app = crate::api::create_router(store);

        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("test-api.sock");

        let uds = tokio::net::UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            crate::api::server::serve_unix_socket(uds, app).await;
        });

        // Give server a moment to start accepting
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect via unix socket using hyper
        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(async move { let _ = conn.await; });

        let req = hyper::Request::builder()
            .uri("/health")
            .header("host", "localhost")
            .body(http_body_util::Empty::<hyper::body::Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        assert_eq!(resp.status(), 200, "Health endpoint should return 200 over unix socket");
    }

    #[tokio::test]
    async fn test_preview_diff_with_current_state() {
        let store = create_test_config_store();
        let app = crate::api::create_router(store);

        // Provide a current_state that differs from the desired config
        // The desired config has VLAN 10, port 1 on VLAN 10
        // We'll provide a current state with VLAN 10 but port 1 on VLAN 1
        let body = serde_json::json!({
            "current_state": {
                "vlans": [
                    {"id": 10, "name": "vlan10", "ip_config": "none"}
                ],
                "ports": [
                    {
                        "port_id": "1",
                        "mode": "access",
                        "vlan": 1,
                        "tagged_vlans": [],
                        "enabled": true,
                        "poe_enabled": false,
                        "mac_notify": false,
                        "speed_duplex": "auto"
                    }
                ],
                "port_mirrors": [],
                "warnings": []
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/preview-diff")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK,
                   "preview-diff should return 200 with current_state provided");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["switch_id"], "test-sw-01");
        assert!(json["has_changes"].as_bool().unwrap(), "Should detect changes");
        assert!(json["diff"].is_object(), "Should include diff object");
        assert!(json["commands"].is_object(), "Should include commands object");
    }

    #[tokio::test]
    async fn test_sse_events_endpoint() {
        use crate::config::SseEvent;

        let store = create_test_config_store();
        let events_tx = store.events.clone();
        let app = crate::api::create_router(store);

        // Start SSE server on a unix socket so we can test the streaming response
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("sse-test.sock");
        let uds = tokio::net::UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            crate::api::server::serve_unix_socket(uds, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect and send SSE request
        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await.unwrap();
        tokio::spawn(async move { let _ = conn.await; });

        let req = hyper::Request::builder()
            .uri("/api/events")
            .header("host", "localhost")
            .header("accept", "text/event-stream")
            .body(http_body_util::Empty::<hyper::body::Bytes>::new())
            .unwrap();

        let resp = sender.send_request(req).await.unwrap();
        assert_eq!(resp.status(), 200, "SSE endpoint should return 200");

        // Emit an event after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            events_tx.send(SseEvent::Status {
                switch_id: "sw-01".to_string(),
                state: "configuring".to_string(),
            }).unwrap();
        });

        // Read the first frame from the SSE stream with a timeout
        use http_body_util::BodyExt;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            resp.into_body().frame(),
        ).await;

        assert!(result.is_ok(), "Should receive SSE frame within timeout");
        let frame = result.unwrap();
        assert!(frame.is_some(), "Should have at least one frame");
        let data = frame.unwrap().unwrap().into_data().unwrap();
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("event: status") || text.contains("sw-01"),
                "SSE frame should contain status event data, got: {}", text);
    }

    #[tokio::test]
    async fn test_save_overlay_creates_file() {
        let store = create_test_config_store();

        // Set config metadata with a temp config folder
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let body = serde_json::json!({
            "filename": "test-overlay.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 99, "name": "new-vlan"}]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED,
                   "save-overlay should return 201");

        // Verify file was created
        let file_path = config_dir.path().join("test-overlay.yaml");
        assert!(file_path.exists(), "Overlay file should be created");

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("merge_priority: 200"), "File should contain priority");
        assert!(content.contains("new-vlan"), "File should contain the VLAN config");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_path_traversal() {
        let store = create_test_config_store();

        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let body = serde_json::json!({
            "filename": "../etc/passwd",
            "merge_priority": 200,
            "config": { "switches": [{"id": "test-sw-01"}] }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Path traversal should be rejected");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_invalid_vlan_reference() {
        let store = create_test_config_store();

        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        // Port references VLAN 999 which is not in the vlans list
        let body = serde_json::json!({
            "filename": "bad-overlay.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 10, "name": "test"}],
                    "ports": [{
                        "port_id": "1",
                        "vlan": 999,
                        "tagged_vlans": [],
                        "enabled": true
                    }]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Save with invalid VLAN reference should be rejected");

        // File should NOT have been created
        let file_path = config_dir.path().join("bad-overlay.yaml");
        assert!(!file_path.exists(), "Invalid config should not be written to disk");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_invalid_tagged_vlan_reference() {
        let store = create_test_config_store();

        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        // Port has tagged VLAN 3 which doesn't exist in vlans list
        let body = serde_json::json!({
            "filename": "bad-tagged.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 1, "name": "default"}, {"id": 10, "name": "mgmt"}],
                    "ports": [{
                        "port_id": "7",
                        "vlan": 1,
                        "tagged_vlans": [3],
                        "enabled": true
                    }]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Save with invalid tagged VLAN reference should be rejected");

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let error_msg = json["error"].as_str().unwrap();
        assert!(error_msg.contains("VLAN") || error_msg.contains("validation"),
                "Error should mention VLAN validation. Got: {}", error_msg);
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_duplicate_vlan_ids() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let body = serde_json::json!({
            "filename": "dup-vlan.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [
                        {"id": 10, "name": "first"},
                        {"id": 10, "name": "duplicate"}
                    ]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Duplicate VLAN IDs should be rejected");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_vlan_out_of_range() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let body = serde_json::json!({
            "filename": "bad-vlan-range.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 5000, "name": "out-of-range"}]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "VLAN ID out of range should be rejected");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_duplicate_port_ids() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let body = serde_json::json!({
            "filename": "dup-port.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 10, "name": "test"}],
                    "ports": [
                        {"port_id": "1", "vlan": 10, "enabled": true},
                        {"port_id": "1", "vlan": 10, "enabled": false}
                    ]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Duplicate port IDs should be rejected");
    }

    #[tokio::test]
    async fn test_save_overlay_expands_port_ranges() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        // Port range "1-3" should be expanded and saved
        let body = serde_json::json!({
            "filename": "range-test.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 10, "name": "test"}],
                    "ports": [
                        {"port_id": "1-3", "vlan": 10, "enabled": true}
                    ]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED,
                   "Port range should be accepted and expanded");

        // Verify the file was created
        let file_path = config_dir.path().join("range-test.yaml");
        assert!(file_path.exists(), "File should be created");
    }

    #[tokio::test]
    async fn test_save_overlay_rejects_mirror_dest_in_ports() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        // Port 22 is both in ports list AND mirror destination — should be rejected
        let body = serde_json::json!({
            "filename": "mirror-conflict.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 10, "name": "test"}],
                    "ports": [
                        {"port_id": "1", "vlan": 10, "enabled": true},
                        {"port_id": "22", "vlan": 10, "enabled": true}
                    ],
                    "port_mirrors": [
                        {"session_id": "1", "source_ports": ["1"], "destination_port": "22", "direction": "both"}
                    ]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST,
                   "Mirror dest port in ports list should be rejected");
    }

    #[tokio::test]
    async fn test_save_overlay_allows_vlan_1_implicitly() {
        // VLAN 1 is the default VLAN on all switches and cannot be removed.
        // Ports referencing VLAN 1 should be valid even if VLAN 1 is not
        // explicitly defined in the vlans list.
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        // VLAN 1 not in vlans list, but port references it — should be allowed
        let body = serde_json::json!({
            "filename": "vlan1-implicit.yaml",
            "merge_priority": 200,
            "config": {
                "switches": [{
                    "id": "test-sw-01",
                    "vlans": [{"id": 10, "name": "test"}],
                    "ports": [{
                        "port_id": "1",
                        "vlan": 1,
                        "tagged_vlans": [],
                        "enabled": true
                    }]
                }]
            }
        });

        let request = Request::builder()
            .method("POST")
            .uri("/switches/test-sw-01/save-overlay")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED,
                   "Port referencing VLAN 1 should be allowed even without VLAN 1 in vlans list");
    }

    #[tokio::test]
    async fn test_config_sources_returns_files() {
        let store = create_test_config_store();

        // Set config metadata
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/etc/main.yaml"),
            config_folders: vec![std::path::PathBuf::from("/etc/switch-configurator")],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("GET")
            .uri("/switches/test-sw-01/config-sources")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["switch_id"], "test-sw-01");
        assert!(json["sources"].is_array(), "Should include sources array");
        // At minimum, the main config should be listed
        let sources = json["sources"].as_array().unwrap();
        assert!(!sources.is_empty(), "Should have at least one source");
        assert!(sources[0]["file"].is_string(), "Source should have a file path");
        assert!(sources[0]["priority"].is_number(), "Source should have a priority");
    }

    #[tokio::test]
    async fn test_preview_diff_switch_not_found() {
        let store = create_test_config_store();
        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("POST")
            .uri("/switches/nonexistent/preview-diff")
            .header("Content-Type", "application/json")
            .body(Body::from("{}"))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ============================================================================
    // Overlay file management (delete + read)
    // ============================================================================

    #[tokio::test]
    async fn test_delete_overlay_success() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        // Create a file to delete
        let file_path = config_dir.path().join("test-overlay.yaml");
        std::fs::write(&file_path, "switches: []").unwrap();
        assert!(file_path.exists());

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("DELETE")
            .uri("/switches/test-sw-01/overlay/test-overlay.yaml")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "Delete should succeed");
        assert!(!file_path.exists(), "File should be deleted");
    }

    #[tokio::test]
    async fn test_delete_overlay_rejects_path_traversal() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("DELETE")
            .uri("/switches/test-sw-01/overlay/..%2Fetc%2Fpasswd")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_overlay_not_found() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("DELETE")
            .uri("/switches/test-sw-01/overlay/nonexistent.yaml")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_read_overlay_success() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let file_path = config_dir.path().join("test-overlay.yaml");
        std::fs::write(&file_path, "switches:\n  - id: test\n").unwrap();

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("GET")
            .uri("/switches/test-sw-01/overlay/test-overlay.yaml")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("switches:"), "Should return YAML content");
    }

    #[tokio::test]
    async fn test_read_overlay_rejects_path_traversal() {
        let store = create_test_config_store();
        let config_dir = tempfile::tempdir().unwrap();
        store.status.set_config_metadata(crate::status::ConfigMetadata {
            config_file: std::path::PathBuf::from("/tmp/main.yaml"),
            config_folders: vec![config_dir.path().to_path_buf()],
            last_loaded: chrono::Utc::now(),
            switches_count: 1,
        }).await;

        let app = crate::api::create_router(store);

        let request = Request::builder()
            .method("GET")
            .uri("/switches/test-sw-01/overlay/..%2Fetc%2Fpasswd")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_poe_reset_switch_not_found() {
        let store = create_test_config_store();
        let response = poe_reset(
            axum::extract::State(store),
            axum::extract::Path(("nonexistent".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_poe_reset_non_poe_switch() {
        let store = create_test_config_store();
        let response = poe_reset(
            axum::extract::State(store),
            axum::extract::Path(("test-sw-03".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("does not support PoE"));
    }

    #[tokio::test]
    async fn test_poe_reset_non_poe_port() {
        // Port 49 on Aruba2930F is SFP (no PoE)
        let store = create_test_config_store();
        let response = poe_reset(
            axum::extract::State(store),
            axum::extract::Path(("test-sw-01".to_string(), "49".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("does not support PoE"));
    }

    #[tokio::test]
    async fn test_poe_reset_switch_busy() {
        let store = create_test_config_store();
        store.status.set_currently_configuring("test-sw-01".to_string()).await;

        let response = poe_reset(
            axum::extract::State(store.clone()),
            axum::extract::Path(("test-sw-01".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_poe_reset_valid_returns_202() {
        // A valid Aruba PoE port returns 202 Accepted; the actual reset runs
        // in a background task (connection happens there).
        let store = create_test_config_store();
        let response = poe_reset(
            axum::extract::State(store),
            axum::extract::Path(("test-sw-01".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_poe_reset_emits_connecting_event() {
        // The first SSE event emitted by the background task is a PoeReset
        // "connecting" stage — this arrives before the (slow) connect attempt,
        // so the test is fast and deterministic regardless of connection outcome.
        let store = create_test_config_store();
        let mut rx = store.events.subscribe();

        let response = poe_reset(
            axum::extract::State(store.clone()),
            axum::extract::Path(("test-sw-01".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for SSE event")
            .expect("broadcast recv error");

        match ev {
            SseEvent::PoeReset {
                stage,
                port_id,
                switch_id,
                ..
            } => {
                assert_eq!(stage, "connecting");
                assert_eq!(port_id, "1");
                assert_eq!(switch_id, "test-sw-01");
            }
            other => panic!("expected PoeReset connecting event, got {:?}", other),
        }
    }

    #[test]
    fn test_sse_event_poe_reset_serialization() {
        let ev = SseEvent::PoeReset {
            switch_id: "sw1".to_string(),
            port_id: "5".to_string(),
            stage: "waiting".to_string(),
            detail: Some("3".to_string()),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "poe-reset");
        assert_eq!(json["data"]["switch_id"], "sw1");
        assert_eq!(json["data"]["port_id"], "5");
        assert_eq!(json["data"]["stage"], "waiting");
        assert_eq!(json["data"]["detail"], "3");
    }

    #[tokio::test]
    async fn test_poe_reset_unsupported_vendor() {
        // test-sw-02 is Cisco — not yet supported
        let store = create_test_config_store();
        let response = poe_reset(
            axum::extract::State(store),
            axum::extract::Path(("test-sw-02".to_string(), "1".to_string())),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("not yet supported"));
    }
}
