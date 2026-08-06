//! Integration tests for referencing VLANs by name (instead of numeric id)
//! on a port's untagged `vlan` and `tagged_vlans` fields.
//!
//! Covers the full config-load pipeline: single-file (`AppConfig::load`) and
//! multi-config merge (`AppConfig::load_multi`) where VLANs and the ports that
//! reference them by name may live in different files.

use std::fs;
use std::path::PathBuf;
use switch_configurator::config::AppConfig;
use tempfile::TempDir;

fn write(dir: &std::path::Path, name: &str, contents: &str) -> PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&p, contents).unwrap();
    p
}

const SINGLE_FILE_NAMED: &str = r#"
switches:
  - id: "IT-04249"
    hostname: "IT-04249"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials:
      username: admin
      password: admin
      connection_type: ssh
    vlans:
      - id: 1020
        name: "APC-Zone1"
      - id: 30
        name: "Voice"
      - id: 40
        name: "Guest"
    ports:
      - port_id: "15"
        vlan: "APC-Zone1"
        description: "APC v1 - Zone 1"
      - port_id: "16"
        vlan: 1020
        tagged_vlans: ["Voice", 40]
"#;

#[test]
fn test_single_file_named_untagged_and_tagged_resolve() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "config.yaml", SINGLE_FILE_NAMED);

    let (config, failures) = AppConfig::load(&path).expect("load should succeed");
    assert!(failures.is_empty(), "no validation failures expected: {:?}", failures);
    assert_eq!(config.switches.len(), 1);
    let sw = &config.switches[0];

    let p15 = sw.ports.iter().find(|p| p.port_id == "15").unwrap();
    assert_eq!(p15.vlan, 1020, "named untagged VLAN resolves to id");

    let p16 = sw.ports.iter().find(|p| p.port_id == "16").unwrap();
    assert_eq!(p16.vlan, 1020);
    // Voice(30) resolved from name; 40 numeric passthrough. Order preserved.
    assert_eq!(p16.tagged_vlans, vec![30, 40]);
}

#[test]
fn test_single_file_unknown_untagged_name_skips_switch() {
    let bad = r#"
switches:
  - id: "IT-04249"
    hostname: "IT-04249"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials: { username: admin, password: admin, connection_type: ssh }
    vlans:
      - id: 1020
        name: "APC-Zone1"
    ports:
      - port_id: "15"
        vlan: "Zone-DOES-NOT-EXIST"
"#;
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), "config.yaml", bad);

    // Single-file load surfaces the failure via the failures vec (switch skipped).
    let (config, failures) = AppConfig::load(&path).expect("load returns, switch skipped");
    assert!(
        config.switches.is_empty(),
        "switch with unknown untagged VLAN name must be skipped"
    );
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].error.contains("Zone-DOES-NOT-EXIST"),
        "failure should name the bad VLAN: {}",
        failures[0].error
    );
}

#[test]
fn test_single_file_numeric_and_named_are_equivalent() {
    // Prove transparency: the resolved config from a name-based file equals the
    // resolved config from the numeric equivalent.
    let numeric = r#"
switches:
  - id: "sw"
    hostname: "sw"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials: { username: admin, password: admin, connection_type: ssh }
    vlans:
      - { id: 10, name: "Users" }
      - { id: 20, name: "Voice" }
    ports:
      - { port_id: "1", vlan: 10, tagged_vlans: [20] }
"#;
    let named = r#"
switches:
  - id: "sw"
    hostname: "sw"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials: { username: admin, password: admin, connection_type: ssh }
    vlans:
      - { id: 10, name: "Users" }
      - { id: 20, name: "Voice" }
    ports:
      - { port_id: "1", vlan: "Users", tagged_vlans: ["Voice"] }
"#;
    let dir = TempDir::new().unwrap();
    let (cn, _) = AppConfig::load(&write(dir.path(), "n.yaml", numeric)).unwrap();
    let (cm, _) = AppConfig::load(&write(dir.path(), "m.yaml", named)).unwrap();

    let pn = &cn.switches[0].ports[0];
    let pm = &cm.switches[0].ports[0];
    assert_eq!(pn.vlan, pm.vlan);
    assert_eq!(pn.tagged_vlans, pm.tagged_vlans);
    assert_eq!(pm.vlan, 10);
    assert_eq!(pm.tagged_vlans, vec![20]);
}

#[test]
fn test_multi_config_cross_file_name_resolution() {
    // VLANs defined in a folder file; the port that references them BY NAME lives
    // in the main file. Resolution must happen after merge.
    let dir = TempDir::new().unwrap();
    let main = r#"
switches:
  - id: "sw-cross"
    hostname: "sw-cross"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials: { username: admin, password: admin, connection_type: ssh }
    ports:
      - port_id: "5"
        vlan: "Servers"
        tagged_vlans: ["Mgmt"]
"#;
    let vlans_file = r#"
switches:
  - id: "sw-cross"
    vlans:
      - { id: 100, name: "Servers" }
      - { id: 200, name: "Mgmt" }
"#;
    let main_path = write(dir.path(), "main.yaml", main);
    let folder = dir.path().join("common");
    write(&folder, "vlans.yaml", vlans_file);

    let (config, failures) = AppConfig::load_multi(&main_path, &[folder]).unwrap();
    assert!(failures.is_empty(), "no failures expected: {:?}", failures);
    let sw = config.switches.iter().find(|s| s.id == "sw-cross").unwrap();
    let p5 = sw.ports.iter().find(|p| p.port_id == "5").unwrap();
    assert_eq!(p5.vlan, 100, "cross-file named untagged VLAN resolves");
    assert_eq!(p5.tagged_vlans, vec![200], "cross-file named tagged VLAN resolves");
}

#[test]
fn test_multi_config_duplicate_name_across_files_is_error() {
    // Same VLAN name defined for two different ids across files → ambiguous.
    let dir = TempDir::new().unwrap();
    let main = r#"
switches:
  - id: "sw-dup"
    hostname: "sw-dup"
    model: Aruba2930F
    management_ip: "10.0.0.1"
    credentials: { username: admin, password: admin, connection_type: ssh }
    vlans:
      - { id: 10, name: "Dup" }
    ports:
      - port_id: "1"
        vlan: "Dup"
"#;
    let extra = r#"
switches:
  - id: "sw-dup"
    vlans:
      - { id: 99, name: "Dup" }
"#;
    let main_path = write(dir.path(), "main.yaml", main);
    let folder = dir.path().join("common");
    write(&folder, "extra.yaml", extra);

    let (config, failures) = AppConfig::load_multi(&main_path, &[folder]).unwrap();
    // Switch must be skipped due to ambiguous VLAN name.
    let loaded = config.switches.iter().any(|s| s.id == "sw-dup");
    assert!(!loaded, "ambiguous-name switch should be skipped");
    assert!(
        failures.iter().any(|f| f.error.contains("ambiguous")),
        "should report ambiguous name failure: {:?}",
        failures
    );
}

#[test]
fn test_example_vlan_by_name_yaml_loads() {
    // The shipped example must always resolve cleanly.
    let example =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/vlan-by-name.yaml");
    let (config, failures) = AppConfig::load(&example).expect("example should load");
    assert!(failures.is_empty(), "example should have no failures: {:?}", failures);
    let sw = &config.switches[0];
    let p15 = sw.ports.iter().find(|p| p.port_id == "15").unwrap();
    assert_eq!(p15.vlan, 1020);
    let p16 = sw.ports.iter().find(|p| p.port_id == "16").unwrap();
    let p17 = sw.ports.iter().find(|p| p.port_id == "17").unwrap();
    assert_eq!(p16.vlan, p17.vlan);
    assert_eq!(p16.tagged_vlans, p17.tagged_vlans, "named form equals numeric form");
}
