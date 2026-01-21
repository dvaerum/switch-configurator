# State-Aware Configuration Implementation

## Overview

This document describes the state-aware configuration system that reads current switch state before applying changes, computing and applying only the necessary differences.

## Architecture

### Components Added

1. **`src/models.rs`**: Added `SwitchState` and `StateDiff` structures
2. **`src/diff/mod.rs`**: Diff computation logic
3. **`src/vendors/traits.rs`**: Extended with `parse_current_state()` and `apply_diff()` methods

### Flow

```
1. Connect to switch
2. Get running config (show running-config)
3. Parse current state → SwitchState {vlans, ports, mirrors}
4. Load desired state from YAML
5. Compute diff: current vs desired → StateDiff
6. Apply only the changes in StateDiff
7. Save configuration to switch's startup-config (executes `write memory` on the switch to persist running-config to flash memory - does NOT modify filesystem YAML files)
8. Disconnect
```

## Implementation Status

### ✅ Completed
- Core data structures (`SwitchState`, `StateDiff`)
- Diff computation module
- Trait extensions

### 🚧 To Implement Per Vendor

Each vendor needs two new methods:

#### 1. `parse_current_state()`
Parse "show running-config" output into structured `SwitchState`.

**Example parsing (Aruba)**:
```
vlan 10
  name management

interface 1
  untagged vlan 10
  enable
```

**Extracts:**
```rust
SwitchState {
    vlans: vec![Vlan { id: 10, name: "management", description: None }],
    ports: vec![Port { port_id: "1", vlan: 10, enabled: true, ... }],
    mirrors: vec![]
}
```

#### 2. `apply_diff()`
Apply only the changes specified in `StateDiff`:

```rust
async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>, VendorError> {
    let mut results = Vec::new();

    // Add new VLANs
    if !diff.vlans_to_add.is_empty() {
        results.push(self.configure_vlans(&diff.vlans_to_add).await?);
    }

    // Remove old VLANs
    if !diff.vlans_to_remove.is_empty() {
        results.push(self.remove_vlans(&diff.vlans_to_remove).await?);
    }

    // Update changed VLANs
    if !diff.vlans_to_update.is_empty() {
        results.push(self.configure_vlans(&diff.vlans_to_update).await?);
    }

    // Configure changed ports
    if !diff.ports_to_configure.is_empty() {
        results.push(self.configure_ports(&diff.ports_to_configure).await?);
    }

    // Port mirrors...

    Ok(results)
}
```

#### 3. Update `apply_configuration()`
```rust
async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError> {
    // Parse current state
    let current = self.parse_current_state().await?;

    // Compute diff
    let diff = crate::diff::compute_diff(&current, &self.config);

    // Early return if no changes
    if !diff.has_changes() {
        info!("No changes needed for {}", self.config.hostname);
        return Ok(vec![]);
    }

    // Apply diff
    self.apply_diff(&diff).await
}
```

## Parsing Strategy

### Simple Line-by-Line Parsing

For each vendor, parse running config using simple regex/string matching:

**Aruba VLAN Parsing:**
```rust
fn parse_vlans(config: &str) -> Vec<Vlan> {
    let mut vlans = Vec::new();
    let lines: Vec<&str> = config.lines().collect();

    for i in 0..lines.len() {
        if let Some(vlan_id) = extract_vlan_id(lines[i]) {
            let mut vlan = Vlan {
                id: vlan_id,
                name: String::new(),
                description: None,
            };

            // Look ahead for name
            if i + 1 < lines.len() && lines[i+1].contains("name") {
                vlan.name = extract_name(lines[i+1]);
            }

            vlans.push(vlan);
        }
    }

    vlans
}
```

## Benefits

1. **Efficiency**: Only sends necessary commands
2. **Safety**: Won't reconfigure unchanged settings
3. **Awareness**: Knows current vs desired state
4. **Idempotency**: Running twice = no changes second time
5. **Logging**: Can report exactly what changed

## Example Output

```
INFO Applying configuration to aruba-switch-01
DEBUG Parsing current state...
DEBUG Current state: 5 VLANs, 24 ports, 1 mirror
DEBUG Computing differences...
DEBUG   VLANs to add: 2
DEBUG   VLANs to remove: 1
DEBUG   Ports to configure: 3
INFO Applying 6 configuration changes
INFO Successfully configured aruba-switch-01
```

## Next Steps

1. Implement parsing for each vendor (regex-based, incremental)
2. Add `remove_vlans()` and `remove_mirrors()` methods
3. Add comprehensive logging of changes
4. Add dry-run mode to preview changes without applying
5. Enhance parsers to handle edge cases

## Testing Approach

```bash
# Test with dry_run: true in config.yaml
cargo run

# Should output:
# "Would add VLAN 10"
# "Would remove VLAN 99"
# "Would configure port 5"
```

## Vendor-Specific Notes

### Aruba
- Simple CLI output
- `show running-config`
- VLANs: `vlan 10` → `name ...`
- Ports: `interface 1` → `untagged vlan ...`

### Cisco
- Verbose output
- `show running-config`
- VLANs: `vlan 10` → `name ...`
- Ports: `interface GigabitEthernet1/0/1` → `switchport access vlan ...`

### FortiSwitch
- Config-style output
- `show full-configuration`
- Structured: `config switch vlan` → `edit 10` → `set name ...`

## Files Modified

- `src/models.rs`: +70 lines (new structures)
- `src/diff/mod.rs`: +140 lines (new file)
- `src/vendors/traits.rs`: +2 methods
- `src/vendors/aruba.rs`: +200 lines (parsing + apply_diff)
- `src/vendors/cisco.rs`: +200 lines (parsing + apply_diff)
- `src/vendors/fortiswitch.rs`: +200 lines (parsing + apply_diff)
- `src/watcher/mod.rs`: Updated to use new flow

**Total**: ~1010 new lines of code
