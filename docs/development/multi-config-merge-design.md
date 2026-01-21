# Multi-Config Merge Design Document

**Status:** Design Approved
**Date:** 2025-01-21
**Author:** System Design
**Primary Use Case:** Modular/reusable configurations

## Overview

This document describes the design for merging multiple configuration files in the switch-configurator project. The feature allows users to split large switch configurations into smaller, modular files that are merged together at runtime.

## Motivation

Users need to:
- Break large configurations into manageable, modular pieces
- Create reusable configuration snippets (e.g., common VLANs, standard port configs)
- Organize configs by logical grouping (per-switch, per-team, per-function)
- Override base configurations without editing the main file

## Core Concepts

### 1. Unique Identifiers

Components are merged based on unique identifiers:

| Component | Unique Identifier | Notes |
|-----------|------------------|-------|
| Switch | `id` | New required field, explicit merge key |
| VLAN | `id` | Existing field |
| Port | `port_id` | After port range expansion |
| Port Mirror | `session_id` | Existing field |

### 2. Priority System

**Priority Direction:** Lower number = higher priority (0 is highest, 9999 is lowest)

| Priority Range | Purpose | Allowed In |
|----------------|---------|------------|
| 0-10 | Highest priority overrides | `--config-file` only |
| 11-49 | High priority | Any file |
| **50** | **Default for main config** | `--config-file` (default) |
| 51-99 | Medium priority | Any file |
| **100** | **Default for folder configs** | `--config-folder` (default) |
| 101-9999 | Lower priorities | Any file |

**Priority Scope:** File-level only (not switch-level or component-level)

**Validation Rules:**
- Folder configs using priority 0-10 will error
- Main config can use any priority (0-9999)

### 3. Merge Strategy

**Philosophy:** Replace entire objects/lists (simple and predictable)

#### Component-Level Merge Behavior

| Component | Merge Strategy | Identifier | Details |
|-----------|---------------|------------|---------|
| **Switch** | Merge components within | `id` | Switch-level fields have special rules (see below) |
| **VLAN** | Replace entire VLAN | `id` | Higher priority replaces all fields |
| **Port** | Replace entire port | `port_id` | Higher priority replaces all fields |
| **Port Mirror** | Replace entire mirror | `session_id` | Higher priority replaces all fields |
| **SNMP** | Merge sub-components | N/A | See SNMP section below |
| **Validation** | Replace entire object | N/A | Higher priority replaces entire validation config |
| **Settings** | Replace entire object | N/A | Per-switch settings, higher priority replaces entire settings config |
| **Credentials** | Replace entire object | N/A | Higher priority replaces entire credentials config |

#### Switch-Level Fields: Identity Consistency

The following switch-level fields must be **identical across all configs** (or only defined once):

- `hostname`
- `management_ip`
- `model`

**Validation Rules:**
- If a field is defined in multiple configs for the same switch `id`, values must match exactly
- Priority is irrelevant for these fields
- Mismatch results in error (not override)

**Example:**
```yaml
# main.yaml
switches:
  - id: sw-01
    hostname: switch-01
    management_ip: 192.168.1.10
    model: Aruba2930F

# network.yaml
switches:
  - id: sw-01
    hostname: switch-01           # ✅ OK - matches main.yaml
    management_ip: 192.168.1.10   # ✅ OK - matches main.yaml
    # model not specified - ✅ OK - use from main.yaml

# BAD example:
switches:
  - id: sw-01
    hostname: switch-01-DIFFERENT  # ❌ ERROR - doesn't match main.yaml
```

**Error Message:**
```
Error: Switch identity field mismatch
  Switch ID: sw-01
  Field: hostname
  Conflict:
    - main.yaml: "switch-01"
    - network.yaml: "switch-01-DIFFERENT"

  Switch identity fields (hostname, management_ip, model) must be identical across all configs.
```

#### SNMP Merge Behavior

SNMP is special - merge sub-components separately:

```yaml
# main.yaml (priority 50)
snmp:
  communities: [...]
  trap_receivers: [...]
  enabled_traps: [...]

# override.yaml (priority 30, higher priority)
snmp:
  enabled_traps: [...]  # Replaces main.yaml's enabled_traps
  # communities and trap_receivers NOT specified, so use main.yaml's values
```

**Result:** Each list (communities, trap_receivers, enabled_traps) is independent. If a list is specified in higher priority, it replaces the lower priority list. If not specified, lower priority list is used.

#### Port Range Handling

Port ranges (e.g., `"1-5"`) are expanded **before merging**:

```yaml
# main.yaml (priority 50)
ports:
  - port_id: "1-5"
    vlan: 10

# Expanded to:
ports:
  - port_id: "1"
    vlan: 10
  - port_id: "2"
    vlan: 10
  - port_id: "3"
    vlan: 10
  - port_id: "4"
    vlan: 10
  - port_id: "5"
    vlan: 10

# override.yaml (priority 30, higher priority)
ports:
  - port_id: "2"
    vlan: 20

# After merge:
ports:
  - port_id: "1": vlan 10  # From main
  - port_id: "2": vlan 20  # From override (replaced)
  - port_id: "3": vlan 10  # From main
  - port_id: "4": vlan 10  # From main
  - port_id: "5": vlan 10  # From main

# Warning logged:
# "Port '2' from override.yaml (priority 30) overrides port from range '1-5' in main.yaml (priority 50)"
```

## CLI Interface

### New Arguments

```bash
# Rename existing argument
--config-fileFILE          # Renamed to --config-file
--config-file FILE     # Path to main configuration file (default: config.yaml)

# New arguments
--config-folder DIR    # Directory containing additional config files (repeatable)
--show-merged-config   # Output final merged config and exit (no apply)
--show-merge-trace     # Output detailed merge trace and exit (no apply)
```

### Examples

```bash
# Basic usage
cargo run -- --config-file main.yaml --config-folder ./configs.d/

# Multiple folders
cargo run -- --config-file main.yaml \
             --config-folder ./base-configs/ \
             --config-folder ./overrides/

# Debug merged config
cargo run -- --config-file main.yaml \
             --config-folder ./configs.d/ \
             --show-merged-config

# Debug merge trace
cargo run -- --config-file main.yaml \
             --config-folder ./configs.d/ \
             --show-merge-trace
```

## Configuration File Format

### File-Level Priority

```yaml
# network.yaml
merge_priority: 80  # Optional, defaults: 50 for main, 100 for folder configs

switches:
  - id: sw-01  # New required field for merge identification
    hostname: switch-01
    management_ip: 192.168.1.10
    model: Aruba2930F
    vlans: [...]
    ports: [...]
```

### Complete Example

```yaml
# main.yaml (default priority: 50)
merge_priority: 50  # Optional, this is the default

switches:
  - id: sw-core-01
    hostname: core-switch-01
    management_ip: 192.168.1.10
    model: Aruba2930F
    credentials:
      username: admin
      password: secret
      connection_type: ssh
    vlans:
      - id: 1
        name: default
      - id: 10
        name: management
        ip_config: dhcp

settings:
  ssh_timeout_secs: 30
  max_retries: 3
```

```yaml
# configs.d/common-vlans.yaml (default priority: 100, lower than main)
merge_priority: 100

switches:
  - id: sw-core-01
    vlans:
      - id: 20
        name: guest
      - id: 30
        name: iot
```

```yaml
# configs.d/port-config.yaml (default priority: 100)
merge_priority: 100

switches:
  - id: sw-core-01
    ports:
      - port_id: "1-10"
        mode: access
        vlan: 10
        enabled: true
```

```yaml
# configs.d/emergency-override.yaml (high priority)
merge_priority: 20  # Higher priority than main (50)

switches:
  - id: sw-core-01
    ports:
      - port_id: "5"
        enabled: false  # Emergency disable, overrides port-config.yaml
```

**Merged Result:**
- Switch identity from main.yaml
- VLANs: 1, 10 (from main), 20, 30 (from common-vlans)
- Ports: 1-4,6-10 enabled on VLAN 10, port 5 disabled (emergency override)
- Settings from main.yaml

## Loading Process

### Step-by-Step Algorithm

```
1. Parse CLI arguments
   ├─ --config-file (main config, default priority 50)
   └─ --config-folder (can be repeated, discover *.yaml files)

2. Load main config file
   ├─ Parse YAML
   ├─ Extract merge_priority (default: 50)
   ├─ Validate priority (can be 0-9999)
   ├─ Expand port ranges
   └─ Store as ConfigWithMetadata

3. Discover and load folder configs
   ├─ For each --config-folder (in order specified):
   │  ├─ Scan directory for *.yaml files (not *.yml)
   │  ├─ Sort files alphabetically
   │  ├─ For each file:
   │  │  ├─ Parse YAML
   │  │  ├─ Extract merge_priority (default: 100)
   │  │  ├─ Validate priority (must be 11-9999)
   │  │  ├─ Expand port ranges
   │  │  └─ Store as ConfigWithMetadata
   └─ Build flat list of all loaded configs

4. Validate priority constraints
   ├─ Check: Folder configs must use priority >= 11
   └─ Error if violated

5. Group switches by ID
   ├─ Create HashMap<switch_id, Vec<SwitchConfigWithMetadata>>
   ├─ Each Vec contains all definitions of that switch across files
   └─ NOTE: Switches can be introduced in any file (not required to be in main)

6. Validate switch identity fields and credentials
   For each switch ID:
   ├─ Collect all definitions of hostname, management_ip, model
   ├─ Check: If field defined multiple times, values must be identical
   └─ Error if mismatch found

7. Merge each switch
   For each switch ID:
   ├─ Sort switch configs by priority (ascending: 0 first, 9999 last)
   ├─ Start with empty SwitchConfig
   ├─ Merge identity fields (hostname, management_ip, model)
   │  └─ Use first non-None value (already validated identical)
   └─ For each config (lowest priority first, highest priority last):
      ├─ Merge VLANs (by id, replace entire VLAN)
      ├─ Merge Ports (by port_id, replace entire port, warn on range overlap)
      ├─ Merge Port Mirrors (by session_id, replace entire mirror)
      ├─ Merge SNMP (sub-component lists, replace each list)
      ├─ Merge Validation (replace entire object if present)
      ├─ Merge Settings (replace entire object if present)
      └─ Merge Credentials (replace entire object if present)

8. Detect conflicts (PRE-MERGE VALIDATION)
   ├─ Before merging, scan all components for conflicts
   ├─ If two configs have same priority and define same component with different values:
   │  └─ Collect conflict in error list
   ├─ After scanning all components, if conflicts found:
   │  └─ Report ALL conflicts at once with descriptive messages
   │  └─ Abort before any merging (fail fast with complete error picture)

9. Handle debug flags
   ├─ If --show-merged-config: Output merged YAML and exit
   ├─ If --show-merge-trace: Output detailed trace and exit
   └─ Otherwise: Continue with normal operation

10. Validate merged config
    ├─ Run all existing validation rules
    ├─ Check for required fields
    └─ Return final merged AppConfig
```

### Conflict Detection

When two components have **identical priority** and conflicting values, produce detailed error:

```
Error: Merge conflict detected
  Switch: sw-01
  Component: VLAN 10, field 'name'
  Priority: 100 (same priority in both files)
  Conflict:
    - configs.d/network.yaml: "management"
    - configs.d/security.yaml: "mgmt-vlan"

  Resolution: Change merge_priority in one of the files, or make values identical.
```

**Conflict Scenarios:**
- Same switch ID, same VLAN ID, different VLAN fields, same priority
- Same switch ID, same port ID, different port fields, same priority
- Same switch ID, same mirror session ID, different fields, same priority
- Same switch ID, same settings/validation/credentials defined, same priority

**Not Conflicts:**
- Different priorities (higher priority wins, no error)
- Same values (no conflict, values match)
- Non-overlapping components (VLAN 10 in one, VLAN 20 in another)

## Data Structures

### ConfigWithMetadata

```rust
#[derive(Debug, Clone)]
pub struct ConfigWithMetadata {
    /// The parsed configuration
    pub config: AppConfig,

    /// Merge priority (0 = highest, 9999 = lowest)
    /// Defaults: 50 for MainConfig, 100 for FolderConfig
    pub merge_priority: u16,

    /// Source file path for debugging/error messages
    pub source_file: PathBuf,

    /// Is this from main config or folder config?
    pub source_type: ConfigSourceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceType {
    MainConfig,
    FolderConfig,
}

impl ConfigSourceType {
    pub fn default_priority(&self) -> u16 {
        match self {
            ConfigSourceType::MainConfig => 50,
            ConfigSourceType::FolderConfig => 100,
        }
    }
}
```

### SwitchConfig Changes

Add `id` field and move `settings` to `SwitchConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct SwitchConfig {
    /// Unique identifier for merging switches across configs
    #[validate(length(min = 1))]
    pub id: String,  // NEW FIELD

    pub hostname: String,
    pub management_ip: String,
    pub model: SwitchModel,

    /// Per-switch settings (moved from AppConfig)
    #[serde(default)]
    pub settings: Settings,  // MOVED FROM AppConfig

    // ... rest of fields (credentials, vlans, ports, etc.)
}
```

### AppConfig Changes

Remove `settings` field (moved to per-switch):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub switches: Vec<SwitchConfig>,
    // settings field REMOVED (now per-switch)
}
```

Note: `merge_priority` is NOT in AppConfig - it lives in the ConfigWithMetadata wrapper.

### YAML Schema Changes

```yaml
# Add merge_priority at file level (optional)
merge_priority: 50  # Optional, 0-9999, defaults: 50 (main), 100 (folder)

# Add id field to each switch, move settings to per-switch
switches:
  - id: sw-core-01  # NEW REQUIRED FIELD
    hostname: switch-01
    management_ip: 192.168.1.10
    model: Aruba2930F

    # Settings now per-switch (moved from global)
    settings:
      ssh_timeout_secs: 30
      max_retries: 3
      dry_run: false
      enforce_port_config: false

    credentials: { ... }
    vlans: [ ... ]
    ports: [ ... ]
```

## Debug Output Formats

### --show-merged-config

Output the final merged configuration as valid YAML:

```yaml
# Merged configuration from:
#   - main.yaml (priority 50)
#   - configs.d/network.yaml (priority 100)
#   - configs.d/overrides.yaml (priority 30)

switches:
  - id: sw-01
    hostname: switch-01
    management_ip: 192.168.1.10
    model: Aruba2930F
    vlans:
      - id: 10
        name: mgmt
    ports:
      - port_id: "1"
        vlan: 10
        enabled: true

settings:
  ssh_timeout_secs: 30
  max_retries: 3
```

### --show-merge-trace

Output detailed audit trail showing source of each value:

```
Switch: sw-01
  Identity Fields:
    id: "sw-01"
      └─ Source: main.yaml (priority 50)
    hostname: "switch-01"
      └─ Source: main.yaml (priority 50)
    management_ip: "192.168.1.10"
      └─ Source: main.yaml (priority 50)
    model: Aruba2930F
      └─ Source: main.yaml (priority 50)

  VLANs:
    VLAN 10:
      └─ Source: configs.d/network.yaml (priority 80)
      └─ Replaced: main.yaml (priority 50)
      Fields:
        - id: 10
        - name: "mgmt"
        - ip_config: dhcp

    VLAN 20:
      └─ Source: configs.d/network.yaml (priority 80)
      Fields:
        - id: 20
        - name: "guest"

  Ports:
    Port "1":
      └─ Source: configs.d/ports.yaml (priority 100)
      └─ Expanded from range: "1-10" in configs.d/ports.yaml
      Fields:
        - port_id: "1"
        - vlan: 10
        - enabled: true

    Port "2":
      └─ Source: configs.d/emergency.yaml (priority 30)
      └─ Replaced: configs.d/ports.yaml range "1-10" (priority 100)
      └─ Warning: Port '2' overrides port from range '1-10'
      Fields:
        - port_id: "2"
        - enabled: false

Settings:
  └─ Source: main.yaml (priority 50)
  Fields:
    - ssh_timeout_secs: 30
    - max_retries: 3
```

## Migration Path

### Breaking Changes

1. **New required field:** `id` must be added to all switches
2. **CLI argument rename:** `--config` becomes `--config-file`
3. **Settings moved from global to per-switch:** `settings` is now a field on each switch instead of global in `AppConfig`

### Migration Steps

1. Add `id` field to all existing switch definitions:
   ```yaml
   # Before:
   switches:
     - hostname: switch-01
       management_ip: 192.168.1.10

   # After:
   switches:
     - id: sw-01  # Add this
       hostname: switch-01
       management_ip: 192.168.1.10
   ```

2. Move `settings` from global to per-switch:
   ```yaml
   # Before:
   switches:
     - hostname: switch-01
       ...

   settings:
     ssh_timeout_secs: 30
     max_retries: 3

   # After:
   switches:
     - id: sw-01
       hostname: switch-01
       settings:  # Moved here
         ssh_timeout_secs: 30
         max_retries: 3
       ...
   ```

3. Update CLI invocations:
   ```bash
   # Before:
   cargo run -- --config-file config.yaml

   # After:
   cargo run -- --config-file config.yaml
   ```

4. Optional: Split large configs into modular files using `--config-folder`

### Backward Compatibility

**Not maintained.** This is a breaking change requiring:
- Adding `id` field to all switches
- Moving `settings` from global to per-switch
- Updating CLI arguments from `--config` to `--config-file` (hard break, no backward compatibility)

Users must update their configurations. A migration guide will be provided in documentation.

## Implementation Phases

### Phase 1: Core Data Structures
- Add `id` field to `SwitchConfig` with validation
- Move `settings` field from `AppConfig` to `SwitchConfig`
- Add `merge_priority` to YAML schema
- Create `ConfigWithMetadata` struct
- Update `src/models.rs` and `src/config.rs`

### Phase 2: Config Loading
- Implement multi-config loading in `src/config.rs`
- Add `--config-folder` CLI argument (repeatable)
- Rename `--config` to `--config-file`
- Implement folder scanning (*.yaml only, alphabetical)
- Add priority validation (0-10 restriction for folder configs)

### Phase 3: Merge Logic
- Implement switch identity field validation
- Implement VLAN merge (replace entire VLAN)
- Implement port merge (replace entire port, expand ranges first)
- Implement port mirror merge (replace entire mirror)
- Implement SNMP merge (sub-component lists)
- Implement validation/settings/credentials merge (replace entire object)
- Add conflict detection with detailed errors

### Phase 4: Debug Features
- Implement `--show-merged-config`
- Implement `--show-merge-trace`
- Add warning logging for port range overlaps

### Phase 5: Testing & Documentation
- Unit tests for merge logic
- Integration tests with example configs
- Update CLAUDE.md
- Update examples/ directory with multi-config examples
- Update docs/guides/configuration.md
- Migration guide for existing users

## Additional Implementation Details

### Port Range Metadata Tracking

When expanding port ranges, track origin information:

```rust
#[derive(Debug, Clone)]
struct PortWithMetadata {
    port: Port,
    expanded_from_range: Option<String>,  // e.g., Some("1-5")
    source_file: PathBuf,
    priority: u16,
}
```

During merge, if explicit port overrides port from range, emit warning:
```
Warning: Port '2' from override.yaml (priority 30) overrides port from range '1-5' in main.yaml (priority 50)
```

### Credentials Validation

When a config specifies `credentials`, validate completeness:
- Must have: `username`
- Must have: `password` OR `ssh_key_path`
- Must have: `connection_type`
- Optional: `port`, `serial_device`, `baud_rate` (depending on connection_type)

Error if credentials incomplete:
```
Error: Incomplete credentials for switch 'sw-01' in override.yaml
  Missing required fields: username, connection_type
  Credentials must be complete if specified.
```

### SNMP Merge Edge Cases

Handle three cases distinctly:

1. **No `snmp` field:** `switch.snmp` not present in YAML
   - Result: Use entire `snmp` object from lower priority config

2. **Empty `snmp` object:** `snmp: {}`
   - Result: Clear all SNMP configuration (no communities, no traps)

3. **Partial `snmp` object:** `snmp: { enabled_traps: [...] }`
   - Result: Merge sub-components independently
   - Specified sub-components replace lower priority
   - Unspecified sub-components use lower priority

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    // Test priority ordering
    #[test]
    fn test_priority_lower_number_wins() { }

    // Test component replacement
    #[test]
    fn test_vlan_replacement() { }

    #[test]
    fn test_port_replacement() { }

    #[test]
    fn test_port_replacement_resets_unspecified_fields() { }

    // Test switch identity validation
    #[test]
    fn test_switch_identity_must_match() { }

    #[test]
    fn test_switch_identity_mismatch_errors() { }

    // Test port range handling
    #[test]
    fn test_port_range_expansion_before_merge() { }

    #[test]
    fn test_port_range_overlap_warning() { }

    // Test SNMP sub-component merge
    #[test]
    fn test_snmp_communities_replace() { }

    #[test]
    fn test_snmp_partial_override() { }

    // Test conflict detection (all at once)
    #[test]
    fn test_same_priority_conflict_errors() { }

    #[test]
    fn test_multiple_conflicts_reported_together() { }

    // Test priority restrictions
    #[test]
    fn test_folder_config_cannot_use_0_to_10() { }

    // Test new switches from folder configs
    #[test]
    fn test_folder_config_can_introduce_new_switch() { }

    #[test]
    fn test_main_config_can_be_empty() { }

    // Test credentials validation
    #[test]
    fn test_incomplete_credentials_error() { }

    // Test SNMP edge cases
    #[test]
    fn test_snmp_not_present_uses_lower_priority() { }

    #[test]
    fn test_snmp_empty_clears_config() { }

    #[test]
    fn test_snmp_partial_merges_subcomponents() { }

    // Test port range metadata
    #[test]
    fn test_port_range_overlap_warning() { }

    // Test settings per-switch
    #[test]
    fn test_settings_per_switch_merge() { }
}
```

### Integration Tests

Create example config sets in `tests/fixtures/`:

```
tests/fixtures/multi-config/
  main.yaml           # Priority 50
  configs.d/
    network.yaml      # Priority 100
    security.yaml     # Priority 100
    emergency.yaml    # Priority 20
```

Test scenarios:
1. Basic merge (main + one folder config)
2. Multiple folder configs
3. Priority ordering (verify lower number wins)
4. Conflict detection - multiple conflicts reported together
5. Port range overlap warnings with metadata tracking
6. Switch identity validation (hostname, management_ip, model must match)
7. New switches introduced in folder configs
8. Empty main config (no switches)
9. SNMP sub-component merge (three edge cases)
10. Credentials validation (completeness check)
11. Settings per-switch merge
12. Debug output formats (--show-merged-config, --show-merge-trace)
13. Replace entire object behavior (ports, vlans, etc.)

## Documentation Updates

### Files to Update

1. **CLAUDE.md**
   - Add section on multi-config merging
   - Update "Configuration File Format" section
   - Add examples of modular configs
   - Document merge strategy and priority system

2. **docs/guides/configuration.md**
   - Add "Multi-Config Merging" section
   - Explain use cases (modular configs)
   - Show examples
   - Document priority system

3. **docs/reference/cli.md**
   - Document new CLI arguments
   - Show usage examples

4. **examples/**
   - Create `examples/multi-config/` directory
   - Add example main.yaml + folder configs
   - Show common patterns (modular VLANs, per-switch configs, overrides)

5. **README.md**
   - Brief mention of multi-config support in features list

### Example Documentation Structure

```markdown
## Multi-Config Merging

Switch Configurator supports splitting configuration across multiple files that are merged at runtime.

### Use Cases

- **Modular Configs**: Break large configs into manageable pieces
- **Reusable Snippets**: Define common VLANs once, use across switches
- **Team Organization**: Different teams manage different config aspects
- **Emergency Overrides**: Quickly override without editing main config

### Priority System

Lower number = higher priority (0 = highest, 9999 = lowest)

- Main config default: priority 50
- Folder configs default: priority 100
- Priority 0-10 reserved for main config only

### Example

[Include complete working example]
```

## Future Enhancements

Potential future additions (not in initial implementation):

1. **Switch-level and component-level priorities**
   - Override file-level priority for specific switches or components
   - More granular control

2. **Explicit merge actions**
   - `merge_action: replace|merge|append|remove`
   - More explicit control over merge behavior

3. **Field-level merge for some components**
   - Option to merge VLANs/ports field-by-field instead of replace
   - More complex but more flexible

4. **Config includes/imports**
   ```yaml
   includes:
     - common-vlans.yaml
     - network-settings.yaml
   ```

5. **Merge validation rules**
   - Custom validation rules for merge conflicts
   - Warnings vs errors configurable

6. **Config templating**
   - Variables and templating for reusable configs
   - Jinja2-style templates

## Design Clarifications

All design questions have been answered through Q&A review:

1. **Settings Scope:** Moved from global (AppConfig) to per-switch (SwitchConfig)
2. **New Switches:** Folder configs CAN introduce entirely new switches (don't need to exist in main config)
3. **Empty Main Config:** Valid for main config to have no switches (fully modular configs allowed)
4. **Conflict Detection:** Pre-merge validation pass - collect ALL conflicts and report together before aborting
5. **Port Range Tracking:** Add metadata during expansion to track which ports came from ranges (for better warnings)
6. **SNMP Missing Sub-Components:**
   - `snmp` field not present: use entire snmp from lower priority
   - `snmp: {}` (empty): clear all SNMP config
   - `snmp` with some fields: merge sub-components independently
7. **Credentials Validation:** Require complete credentials if specified (all required fields must be present)
8. **Replace Entire Port:** Confirmed - all unspecified fields reset to defaults/None (users must specify all fields they want)
9. **Port Range Expansion:** During initial file load (matches current behavior at `src/config.rs:115`)
10. **merge_priority Location:** Separate wrapper struct (ConfigWithMetadata), not in AppConfig itself
11. **CLI Backward Compatibility:** Hard break - remove `--config` entirely, only support `--config-file`
12. **Conflict Reporting:** Collect and show ALL conflicts at once (not just first one)

## References

- Original feature request discussion
- Existing config loading code: `src/config.rs`
- Existing models: `src/models.rs`
- Port range expansion: `src/config.rs:115`
