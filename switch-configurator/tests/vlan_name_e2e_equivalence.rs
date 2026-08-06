//! Hardware-independent E2E equivalence check for VLAN name references.
//!
//! The feature's core guarantee is that a config using VLAN *names* produces the
//! exact same switch commands as the equivalent config using numeric *IDs*.
//!
//! These tests prove that by loading the paired e2e configs
//! (`tests/e2e/vlan-by-name/*-numeric.yaml` vs `*-named.yaml`), computing the
//! diff against an empty starting state, and asserting the generated command
//! preview is byte-identical. This runs in CI without any hardware; the shell
//! runner (`run.sh`) performs the same comparison live over serial on IT-03400.

use std::path::PathBuf;
use switch_configurator::config::AppConfig;
use switch_configurator::diff::compute_diff;
use switch_configurator::models::{CommandPreview, SwitchState};
use switch_configurator::vendors::create_vendor;

fn e2e_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/vlan-by-name")
        .join(name)
}

/// Flatten a CommandPreview into an ordered list of all commands for comparison.
fn flatten(p: &CommandPreview) -> Vec<String> {
    let mut all = Vec::new();
    all.extend(p.vlan_commands.clone());
    all.extend(p.port_commands.clone());
    all.extend(p.mirror_commands.clone());
    all.extend(p.snmp_commands.clone());
    all.extend(p.reset_commands.clone());
    all.extend(p.other_commands.clone());
    all
}

/// Generate the command preview a config would produce against an empty switch.
/// Returns (all_commands_sorted, port_commands_in_order).
///
/// VLAN *definition* ordering in the diff is derived from hash-set iteration and
/// is therefore non-deterministic run-to-run (independent of this feature), so
/// the full-set comparison is order-insensitive. The *port* commands — where
/// VLAN names are actually consumed — are compared in exact order.
fn commands_for(config_file: &str) -> (Vec<String>, Vec<String>) {
    let (config, failures) = AppConfig::load(&e2e_path(config_file)).expect("config loads");
    assert!(failures.is_empty(), "{}: unexpected failures {:?}", config_file, failures);
    let switch = &config.switches[0];

    let vendor = create_vendor(switch).expect("vendor created");
    // Diff against a completely empty current state → full configuration.
    let diff = compute_diff(&SwitchState::default(), switch, switch.settings.enforce_port_config);
    let preview = vendor.generate_commands_for_diff(&diff);

    let mut all = flatten(&preview);
    all.sort();
    (all, preview.port_commands.clone())
}

#[test]
fn test_aruba_2530_8g_named_equals_numeric() {
    let (numeric_all, numeric_ports) = commands_for("aruba-2530-8g-numeric.yaml");
    let (named_all, named_ports) = commands_for("aruba-2530-8g-named.yaml");
    assert!(!numeric_ports.is_empty(), "expected some port commands to be generated");
    assert_eq!(
        numeric_ports, named_ports,
        "Aruba: name-based port commands must be identical to numeric config"
    );
    assert_eq!(
        numeric_all, named_all,
        "Aruba: full command set (order-insensitive) must match"
    );
}

#[test]
fn test_cisco_c9300_named_equals_numeric() {
    let (numeric_all, numeric_ports) = commands_for("cisco-c9300-numeric.yaml");
    let (named_all, named_ports) = commands_for("cisco-c9300-named.yaml");
    assert!(!numeric_ports.is_empty(), "expected some port commands to be generated");
    assert_eq!(
        numeric_ports, named_ports,
        "Cisco: name-based port commands must be identical to numeric config"
    );
    assert_eq!(
        numeric_all, named_all,
        "Cisco: full command set (order-insensitive) must match"
    );
}

#[test]
fn test_unknown_name_config_is_rejected_before_connect() {
    // The unknown-name switch must be skipped at load time (a failure entry),
    // so the tool never even attempts to connect to it.
    let (config, failures) = AppConfig::load(&e2e_path("aruba-unknown-name.yaml"))
        .expect("load returns");
    assert!(
        config.switches.is_empty(),
        "switch with unknown VLAN name must not be loaded"
    );
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].error.contains("DOES-NOT-EXIST"),
        "failure should name the bad VLAN: {}",
        failures[0].error
    );
}
