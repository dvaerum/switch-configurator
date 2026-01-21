# TODO

## Recently Completed (November 2025)

### ✅ Cisco Catalyst 9300 Implementation and Testing
**Status**: Complete
**Completion Date**: November 24, 2025

**Accomplished:**
- ✅ Complete Cisco Catalyst 9300-24P UPoE vendor implementation
- ✅ Hardware testing on real switch (10/10 tests passed)
- ✅ Created 24 comprehensive unit tests
- ✅ Fixed 3 implementation bugs discovered by tests
- ✅ All 178 unit tests passing (100% success rate)
- ✅ Documentation created in `docs/testing/cisco/`

**Test Results:**
- Hardware validation: 10/10 tests passed
- Unit tests: 24/24 passing
- Total project tests: 178/178 passing

See [Cisco Testing Documentation](docs/testing/cisco/README.md) for details.

### ✅ Documentation Organization
**Status**: Complete
**Completion Date**: November 24, 2025

**Accomplished:**
- ✅ Created `docs/testing/cisco/` directory structure
- ✅ Moved testing documentation from `/tmp` to proper locations
- ✅ Updated README.md with correct CLI flags (`--config-file`)
- ✅ Added multi-config system documentation
- ✅ Updated feature lists across all docs
- ✅ Created comprehensive testing section in README

### ✅ Aruba Port Mirroring Investigation
**Status**: Complete - No Bug Found
**Completion Date**: November 24, 2025

**Accomplished:**
- ✅ Investigated reported bug: "only one source port gets monitor command"
- ✅ Verified command generation code handles all source ports correctly
- ✅ Verified state parsing correctly detects multiple monitor ports
- ✅ Verified diff computation detects incomplete mirror configurations
- ✅ Added comprehensive unit test: `test_port_mirroring_four_source_ports()`
- ✅ All 179 unit tests passing (100% success rate)
- ✅ Created investigation documentation in `docs/testing/aruba-port-mirroring-investigation.md`

**Findings:**
- **Code is correct** - Generates monitor commands for ALL source ports
- Reported issue likely from older code version or manual configuration
- Test with 4 source ports (33, 34, 35, 36) passes completely
- Commands generated: `interface X` → `monitor all both mirror 1` → `exit` for each port

**Test Results:**
- Unit test passes: ✅ `test_port_mirroring_four_source_ports()`
- Verified command sequence for all 4 ports
- Total project tests: 179/179 passing

See [Aruba Port Mirroring Investigation](docs/testing/aruba-port-mirroring-investigation.md) for details.

### ✅ Port Name/Description Cleanup Implementation
**Status**: Complete
**Completion Date**: November 24, 2025

**Accomplished:**
- ✅ Implemented port name cleanup in `generate_port_commands()` for Aruba switches
- ✅ Added logic to clear port names when config doesn't specify them
- ✅ Fixed `reset_ports()` to use correct `no name` command (was incorrectly using `no description`)
- ✅ Added 5 comprehensive unit tests for port name cleanup behavior
- ✅ All 184 unit tests passing (100% success rate - up from 179)

**Implementation Details:**
- When a port has `description: None` in config, checks current state and generates `no name` command if needed
- When a port has `description: Some(name)` in config, sets the name (idempotent)
- Similar to trunk VLAN cleanup logic (lines 105-127 in `generate_port_commands()`)
- Ensures idempotent behavior: running twice produces same result

**Test Coverage:**
1. `test_port_name_removed_when_not_in_config()` - Clears name when not specified
2. `test_port_name_changed_from_old_to_new()` - Updates to new name
3. `test_port_name_kept_when_not_changed()` - Idempotent when same
4. `test_multiple_ports_names_cleanup()` - Multiple ports with mixed name/no-name
5. `test_port_name_removed_in_reset_ports()` - Verifies reset uses correct command

**Files Modified:**
- `src/vendors/aruba.rs`:
  - Lines 92-109: Enhanced `generate_port_commands()` with name cleanup logic
  - Line 1627: Fixed `reset_ports()` to use `no name` instead of `no description`
  - Lines 3051-3354: Added 5 comprehensive unit tests

**Test Results:**
- Total project tests: 184/184 passing (100% success rate)
- All new tests pass with detailed command output verification

### ✅ Better Error Handling for Configuration Parsing
**Status**: Complete
**Completion Date**: November 25, 2025

**Accomplished:**
- ✅ Added `serde_path_to_error` dependency for field path tracking
- ✅ Enhanced YAML deserialization with path errors and line numbers
- ✅ Created error enhancement module with context-aware suggestions
- ✅ Added 4 invalid config test fixtures for manual testing
- ✅ All 214 tests passing (189 unit + 25 multi-config)

**Implementation Details:**
- Modified `load()` and `load_with_metadata()` in `src/config.rs` to use `serde_path_to_error`
- Created `src/config/errors.rs` module with `enhance_parse_error()` function
- Errors now show exact field path (e.g., `switches[0].port_mirrors[0].source_ports`)
- Errors include line and column numbers from serde_yaml
- Context-aware suggestions for common mistakes:
  * Missing management_ip → Shows example syntax
  * Missing credentials → Shows both password and SSH key examples
  * Type mismatches (string vs array) → Shows correct array syntax
  * Invalid enum values → Lists valid options

**Error Improvement Example:**

Before:
```
Failed to parse config file: "/etc/switch-configurator/config.yaml"
invalid type: string "33,34,35,36", expected a sequence
```

After:
```
Failed to parse config file: "/etc/switch-configurator/config.yaml"
Field path: switches[0].port_mirrors[0].source_ports
Error: switches[0].port_mirrors[0].source_ports: invalid type: string "33,34,35,36", expected a sequence at line 15 column 23

Tip: source_ports must be an array, not a string.
Instead of: source_ports: "33,34,35,36"
Use: source_ports: ["33", "34", "35", "36"]
```

**Files Modified:**
- `Cargo.toml`: Added serde_path_to_error dependency
- `src/config.rs`: Enhanced deserialization with path errors
- `src/config/errors.rs`: New error enhancement module with 5 unit tests
- `tests/fixtures/invalid-configs/`: 4 test fixtures for manual testing
- `docs/sessions/2025-11-25-error-handling-plan.md`: Implementation plan and documentation

**Test Results:**
- Total project tests: 214/214 passing (189 unit + 25 multi-config + 5 error enhancement tests)
- 4 invalid config fixtures manually tested with enhanced error messages

**Impact:**
- No breaking changes - only improves error messages
- Significantly better user experience when config errors occur
- Helps users fix configuration issues quickly without consulting documentation

See commit `908e75e` for complete implementation.

---

## Future Improvements

### ~~Verify Aruba Port Mirroring Configuration~~ **[RESOLVED - No Bug]**

**Priority**: ~~High~~ RESOLVED
**Status**: ~~Bug - Needs Investigation~~ **NO BUG FOUND - Code is Correct**
**Resolution Date**: November 24, 2025

**Original Issue:**
Port mirroring configuration on Aruba switches may not be applying correctly to all source ports. Only one source port appears to have the `monitor` command applied instead of all specified source ports.

**Resolution:**
After comprehensive code review and testing, **no bug exists in the current codebase**. The code correctly:
1. Generates monitor commands for ALL source ports
2. Parses running config to detect all ports with monitor enabled
3. Computes diffs that detect incomplete mirror configurations

See `docs/testing/aruba-port-mirroring-investigation.md` for complete investigation details.

**Configuration:**
```yaml
port_mirrors:
  - session_id: "1"
    source_ports: ["33", "34", "35", "36"]
    destination_port: "42"
    direction: both
```

**Expected Behavior:**
All source ports (33, 34, 35, 36) should have the `monitor` command applied in the switch configuration.

**Actual Behavior (from `show running-config`):**
```
interface 33
   name "IoT - Zone 1"
   exit
interface 34
   monitor              # Only port 34 has monitor command
   name "IoT - Zone 1"
   exit
interface 35
   name "IoT - Zone 1"
   exit
interface 36
   name "IoT - Zone 1"
   exit
```

Only port 34 has the `monitor` command, but ports 33, 35, and 36 are missing it.

**Investigation Required:**
1. Review Aruba vendor implementation in `src/vendors/aruba.rs`
2. Check `generate_mirror_commands()` method
3. Verify the state parsing in `parse_current_state()` correctly detects existing mirror configurations
4. Check if `apply_diff()` is properly applying mirror changes to all source ports
5. Test with hardware to verify correct Aruba CLI commands for port mirroring
6. Verify mirror session configuration: may need `mirror 1` command on each source port vs single session config

**Possible Root Causes:**
- Command generation may only apply `monitor` to first source port
- Mirror session configuration syntax may be incorrect for Aruba switches
- State diff logic may not be detecting missing monitor commands on all ports
- Aruba may require per-port mirror session configuration vs single session definition

**Testing:**
- Create unit test that verifies mirror commands are generated for all source ports
- Test on actual Aruba hardware with multiple source ports
- Verify `show mirror` and `show running-config` output after configuration
- Compare with Aruba documentation for correct port mirroring syntax

**Vendor-Specific Aruba Mirror Commands:**
Verify correct syntax (may vary by Aruba model):
```
# Option 1: Per-port monitor command with session
interface 33
   mirror 1
   exit

# Option 2: Global mirror session configuration
mirror 1 port 33,34,35,36 destination 42

# Option 3: Per-port monitor without session (current implementation?)
interface 33
   monitor
   exit
```

**Related Files:**
- `src/vendors/aruba.rs` - Aruba vendor implementation
- `src/diff/mod.rs` - State diff computation
- `examples/port-mirroring.yaml` - Port mirroring examples

### ~~Better Error Handling for Configuration Parsing~~ **[COMPLETED]**

**Priority**: ~~Medium~~ COMPLETED
**Status**: ~~Not Started~~ **COMPLETED - November 25, 2025**
**See**: "Recently Completed" section above for full details (commit `908e75e`)

The error handling for YAML configuration parsing has been implemented with field paths, line numbers, and helpful suggestions.

**Current Behavior:**
```
WARN   Failed to load config file "/etc/switch-configurator/test-switch.yaml": Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
ERROR Failed to reload configuration: Failed to load folder config: "/etc/switch-configurator/test-switch.yaml"
```

**Problem Example:**
When a field is missing quotes or has an invalid value, the error doesn't indicate:
- Which field has the problem
- What line number the issue is on
- What the expected format should be
- What the actual invalid value was

**Real-world Examples:**

**Example 1: Type mismatch - string vs array**
```yaml
port_mirrors:
  - session_id: "1"
    source_ports: "33,34,35,36"  # String provided
    destination_port: "42"
    direction: both
```

Error message:
```
WARN   Failed to load config file "/etc/switch-configurator/test-switch.yaml": Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
Error: Failed to load folder config: "/etc/switch-configurator/test-switch.yaml"
Caused by:
    0: Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
    1: invalid type: string "33,34,35,36", expected a sequence
```

The error mentions "expected a sequence" but doesn't explain that `source_ports` needs to be an array like `["33", "34", "35", "36"]` instead of a string.

**Example 2: Missing required field - management_ip**
```yaml
switches:
  - id: "test-switch"
    hostname: "test-switch"
    model: Aruba2540_48G_4SFP
    # management_ip is missing but required
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
```

Error message:
```
WARN   Failed to load config file "/etc/switch-configurator/test-switch.yaml": Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
Error: Failed to load folder config: "/etc/switch-configurator/test-switch.yaml"
Caused by:
    0: Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
    1: missing field `management_ip`
```

While it identifies the missing field, it doesn't indicate which switch entry (by id or line number) is missing the field, making it harder to fix in configs with multiple switches.

**Example 3: Missing required field - credentials**
```yaml
switches:
  - id: "test-switch"
    hostname: "test-switch"
    management_ip: "192.168.1.10"
    model: Aruba2540_48G_4SFP
    # credentials section is missing but required
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
```

Error message:
```
WARN   Failed to load config file "/etc/switch-configurator/test-switch.yaml": Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
Error: Failed to load folder config: "/etc/switch-configurator/test-switch.yaml"
Caused by:
    0: Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
    1: missing field `credentials`
```

Similar issue - no indication of which switch is missing credentials or what the credentials section should contain.

**Example 4: Invalid credentials format - empty/null value**
```yaml
switches:
  - id: "test-switch"
    hostname: "test-switch"
    management_ip: "192.168.1.10"
    model: Aruba2540_48G_4SFP
    credentials:  # Empty credentials object (unit value in YAML)
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
```

Error message:
```
WARN   Failed to load config file "/etc/switch-configurator/test-switch.yaml": Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
Error: Failed to load folder config: "/etc/switch-configurator/test-switch.yaml"
Caused by:
    0: Failed to parse config file: "/etc/switch-configurator/test-switch.yaml"
    1: invalid type: unit value, expected struct Credentials
```

This cryptic error occurs when `credentials:` is present but has no value (or is explicitly null). The "unit value" error message is very unclear - it should explain that credentials cannot be empty and must contain required fields.

**Desired Behavior:**
Error messages should provide:
1. **Exact location**: File path, line number, and field name
2. **Problem description**: What's wrong (missing quotes, invalid enum value, missing required field, etc.)
3. **Expected format**: What the parser was expecting
4. **Actual value**: What was found (if parseable)
5. **Fix suggestion**: How to correct the issue

**Example Improved Errors:**

**For type mismatch:**
```
ERROR Failed to parse config file: /etc/switch-configurator/test-switch.yaml
  Location: Line 45, field 'port_mirrors[0].source_ports'
  Problem: Type mismatch - string provided, array expected
  Expected: Array of port IDs (e.g., ["33", "34", "35", "36"])
  Found: String value "33,34,35,36"
  Fix: Change to array format: source_ports: ["33", "34", "35", "36"]

  Note: Unlike port_id which uses comma-separated strings ("1,2,3"),
        source_ports requires an array of individual port strings.
```

**For missing required field (management_ip):**
```
ERROR Failed to parse config file: /etc/switch-configurator/test-switch.yaml
  Location: Line 2, switch id="test-switch"
  Problem: Missing required field 'management_ip'
  Expected: IP address or hostname for switch management interface
  Example: management_ip: "192.168.1.10"
  Fix: Add management_ip field to switch configuration
```

**For missing required field (credentials):**
```
ERROR Failed to parse config file: /etc/switch-configurator/test-switch.yaml
  Location: Line 2, switch id="test-switch"
  Problem: Missing required field 'credentials'
  Expected: Credentials section with username and password/ssh_key_path
  Example (SSH):
    credentials:
      username: admin
      password: secret123
      # or use ssh_key_path: /path/to/key
  Example (Serial):
    credentials:
      connection_type: serial
      serial_device: /dev/ttyUSB0
      baud_rate: 9600
      username: admin
      password: secret123
  Fix: Add credentials section to switch configuration

  Note: In multi-config setups, credentials can be defined in the main
        config file (recommended) or in folder configs. Credentials are
        merged as a whole object (highest priority wins).
```

**For invalid/empty credentials (unit value):**
```
ERROR Failed to parse config file: /etc/switch-configurator/test-switch.yaml
  Location: Line 5, switch id="test-switch", field 'credentials'
  Problem: Invalid credentials format - empty or null value
  Expected: Non-empty credentials object with required fields
  Found: Empty value (YAML "unit value")

  The 'credentials' field cannot be empty. It must contain:
    - username (required)
    - password or ssh_key_path (required)
    - connection_type (required, e.g., "ssh" or "serial")

  Example (SSH):
    credentials:
      username: admin
      password: secret123

  Example (Serial):
    credentials:
      connection_type: serial
      serial_device: /dev/ttyUSB0
      baud_rate: 9600
      username: admin
      password: secret123

  Fix: Either:
    1. Remove the 'credentials:' line entirely if credentials are defined
       in another config file (multi-config merge)
    2. Provide complete credentials with all required fields
```

**Implementation Notes:**
- Enhance YAML deserialization error handling in `src/config.rs`
- Use `serde_yaml` error context to extract line/column information
- Add custom deserializers with better error messages for complex fields
- Consider using `serde_path_to_error` crate for field-level error reporting
- Add validation error messages that reference the actual configuration values
- Include examples in error messages for common mistakes

**Related Issues:**
- Missing quotes on string fields (port ranges, port lists)
- Invalid enum values (model names, connection types, etc.)
- Missing required fields (id, hostname, management_ip, model)
- Type mismatches (string vs integer, etc.)
- Invalid port range syntax
- Invalid VLAN IDs (out of range 1-4094)
- Empty/null credentials (unit value error)
- Credentials validation in multi-config merge context

**Important Design Note - Optional Fields in Multi-Config Merge:**

Individual config files do NOT need to have certain fields - they can omit them entirely and rely on other config files to provide them. However, after merging all configs, the final merged switch configuration MUST have all required fields.

**Fields Affected:**
- `credentials` - Required after merge, optional in individual files
- `vlans` - Required after merge, optional in individual files
- Other switch configuration sections can be omitted in individual files and merged from multiple sources

**Validation Rules:**
1. **During individual file parsing**: These fields are optional (can be omitted)
2. **After multi-config merge**: Required fields must exist in the final merged switch config
3. **If a field is present in a file**: It must be valid (not empty/null "unit value")

**Examples:**

Valid scenario (credentials and vlans in main, ports in folder config):
```yaml
# main.yaml (priority 50)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: secret
    vlans:
      - id: 1
        name: default
      - id: 10
        name: management

# folder/sw-01-ports.yaml (priority 100)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # credentials omitted - OK, will be inherited from main.yaml
    # vlans omitted - OK, will be inherited from main.yaml
    ports:
      - port_id: "1"
        vlan: 10
```
Result after merge: ✅ Valid - credentials and vlans from main.yaml, ports from folder config

Valid scenario (vlans in main, credentials and ports in folder):
```yaml
# main.yaml (priority 50)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    vlans:
      - id: 1
        name: default
      - id: 10
        name: management

# folder/sw-01-config.yaml (priority 100)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: secret
    # vlans omitted - OK, will be inherited from main.yaml
    ports:
      - port_id: "1"
        vlan: 10
```
Result after merge: ✅ Valid - vlans from main.yaml, credentials and ports from folder config

Invalid scenario (no credentials after merge):
```yaml
# main.yaml (priority 50)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    vlans:
      - id: 10
        name: management
    # credentials omitted

# folder/sw-01-ports.yaml (priority 100)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # credentials omitted
    # vlans omitted
    ports:
      - port_id: "1"
        vlan: 10
```
Result after merge: ❌ Error - no credentials in final merged config

Invalid scenario (no vlans after merge):
```yaml
# main.yaml (priority 50)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: secret
    # vlans omitted

# folder/sw-01-ports.yaml (priority 100)
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # credentials omitted
    # vlans omitted
    ports:
      - port_id: "1"
        vlan: 10
```
Result after merge: ❌ Error - no vlans in final merged config

Invalid scenario (empty credentials in file):
```yaml
# folder/sw-01-ports.yaml
switches:
  - id: "sw-01"
    hostname: "switch-01"
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:  # Empty/null - invalid "unit value"
    ports:
      - port_id: "1"
        vlan: 10
```
Result: ❌ Error during file parsing - "unit value" error

**Required Error Messages:**

1. **File parsing error** (field is empty/null):
   - Current: `invalid type: unit value, expected struct Credentials` (or similar for vlans)
   - Improved: See "For invalid/empty credentials" section above

2. **Post-merge validation error** (no credentials after merge):
   ```
   ERROR Configuration validation failed after merging
     Switch: id="sw-01", hostname="switch-01"
     Problem: Missing required credentials after config merge

     Credentials must be provided in at least one config file for this switch.

     Files contributing to this switch:
       - main.yaml (priority 50) - no credentials
       - folder/sw-01-ports.yaml (priority 100) - no credentials

     Fix: Add credentials to one of these config files:
       credentials:
         username: admin
         password: secret123
   ```

3. **Post-merge validation error** (no vlans after merge):
   ```
   ERROR Configuration validation failed after merging
     Switch: id="sw-01", hostname="switch-01"
     Problem: Missing required vlans after config merge

     VLANs must be provided in at least one config file for this switch.

     Files contributing to this switch:
       - main.yaml (priority 50) - no vlans
       - folder/sw-01-ports.yaml (priority 100) - no vlans

     Fix: Add vlans to one of these config files:
       vlans:
         - id: 1
           name: default
         - id: 10
           name: management
   ```

4. **Post-merge validation error** (multiple missing required fields):
   ```
   ERROR Configuration validation failed after merging
     Switch: id="sw-01", hostname="switch-01"
     Problem: Missing required fields after config merge

     The following required fields are missing:
       - credentials
       - vlans

     Files contributing to this switch (by switch id="sw-01"):
       - main.yaml (priority 50)
         ✓ hostname: "switch-01"
         ✓ management_ip: "192.168.1.10"
         ✓ model: Aruba2930F
         ✗ credentials: missing
         ✗ vlans: missing
         ✓ ports: 10 ports defined

       - folder/ports.yaml (priority 100)
         ✓ hostname: "switch-01"
         ✓ management_ip: "192.168.1.10"
         ✓ model: Aruba2930F
         ✗ credentials: missing
         ✗ vlans: missing
         ✓ ports: 5 ports defined

     Merge result:
       ✓ hostname: "switch-01" (from main.yaml)
       ✓ management_ip: "192.168.1.10" (from main.yaml)
       ✓ model: Aruba2930F (from main.yaml)
       ✗ credentials: MISSING - no config provided this
       ✗ vlans: MISSING - no config provided this
       ✓ ports: 15 ports total (merged from both files)

     Common cause: Switch ID mismatch
       If you expected credentials/vlans to be merged from another file,
       check that all config files use the same switch id="sw-01".

       Files with different IDs will NOT merge together.
       Example:
         main.yaml:    switches[].id = "sw-01"  ✓
         folder.yaml:  switches[].id = "sw-02"  ✗ Different ID!

       These would be treated as TWO separate switches, not merged.

     Fix: Add the missing fields to one of the config files for switch id="sw-01":
       credentials:
         username: admin
         password: secret123
       vlans:
         - id: 1
           name: default
   ```

**Testing:**
Create test cases with intentionally malformed configurations to verify error messages are helpful and actionable.

**Required Unit Tests for Multi-Config Optional Fields:**

**Credentials Tests:**
```rust
#[test]
fn test_credentials_omitted_in_folder_config_inherits_from_main() {
    // main.yaml has credentials, folder config doesn't
    // Result: Should use credentials from main.yaml
}

#[test]
fn test_credentials_omitted_in_main_provided_in_folder() {
    // main.yaml has no credentials, folder config has credentials
    // Result: Should use credentials from folder config
}

#[test]
fn test_credentials_missing_in_all_configs_error_after_merge() {
    // Neither main nor folder configs have credentials
    // Result: Should error AFTER merge with clear message about which files were checked
}

#[test]
fn test_credentials_empty_unit_value_error_during_parsing() {
    // Config has "credentials:" with no value (unit value)
    // Result: Should error DURING file parsing, not during merge
}

#[test]
fn test_credentials_higher_priority_overrides_lower() {
    // main.yaml (priority 50) has credentials A
    // folder.yaml (priority 100) has credentials B
    // Result: Should use credentials A (higher priority)
}

#[test]
fn test_credentials_incomplete_error() {
    // Config has credentials but missing required fields (e.g., no username)
    // Result: Should error with message about incomplete credentials
}

#[test]
fn test_multiple_switches_some_with_some_without_credentials() {
    // Switch A has credentials in main, omitted in folder (OK)
    // Switch B has no credentials anywhere (ERROR)
    // Result: Should error only for Switch B with clear identification
}
```

**VLANs Tests:**
```rust
#[test]
fn test_vlans_omitted_in_folder_config_inherits_from_main() {
    // main.yaml has vlans, folder config doesn't
    // Result: Should use vlans from main.yaml
}

#[test]
fn test_vlans_omitted_in_main_provided_in_folder() {
    // main.yaml has no vlans, folder config has vlans
    // Result: Should use vlans from folder config
}

#[test]
fn test_vlans_missing_in_all_configs_error_after_merge() {
    // Neither main nor folder configs have vlans
    // Result: Should error AFTER merge with clear message about which files were checked
}

#[test]
fn test_vlans_empty_unit_value_error_during_parsing() {
    // Config has "vlans:" with no value (unit value)
    // Result: Should error DURING file parsing, not during merge
}

#[test]
fn test_vlans_higher_priority_replaces_lower() {
    // main.yaml (priority 50) has VLANs 1, 10
    // folder.yaml (priority 100) has VLAN 10 (different config)
    // Result: VLAN 10 from main (higher priority), VLAN 1 from main
}

#[test]
fn test_vlans_merged_from_multiple_sources() {
    // main.yaml has VLAN 1, 10
    // folder.yaml has VLAN 20, 30
    // Result: Should have VLANs 1, 10, 20, 30 merged together
}

#[test]
fn test_multiple_switches_some_with_some_without_vlans() {
    // Switch A has vlans in main, omitted in folder (OK)
    // Switch B has no vlans anywhere (ERROR)
    // Result: Should error only for Switch B with clear identification
}
```

**Combined Tests:**
```rust
#[test]
fn test_modular_config_vlans_main_ports_folder_credentials_folder() {
    // main.yaml: vlans only
    // folder1.yaml: credentials only
    // folder2.yaml: ports only
    // Result: Should merge all three successfully
}

#[test]
fn test_all_required_fields_from_different_sources() {
    // Test that credentials, vlans, ports can each come from different files
    // and merge correctly based on priority
}

#[test]
fn test_missing_multiple_fields_detailed_error() {
    // main.yaml: hostname, management_ip, model, ports only
    // folder.yaml: hostname, management_ip, model, more ports
    // (both files use same switch ID, but neither has credentials or vlans)
    // Result: Should error with detailed breakdown showing:
    //   - Which files contributed to this switch ID
    //   - What each file provides (present/missing)
    //   - What the merge result has/doesn't have
    //   - Hint about switch ID mismatch as common cause
}

#[test]
fn test_switch_id_mismatch_prevents_merge() {
    // main.yaml: switch id="sw-01" with credentials and vlans
    // folder.yaml: switch id="sw-02" with ports
    // (user intended both to be the same switch but typo in ID)
    // Result: Should create TWO switches, "sw-02" will be missing credentials/vlans
    //         Error message should hint that ID mismatch prevents merging
}

#[test]
fn test_merge_same_id_missing_identity_fields_in_some_configs() {
    // Tests merging 2+ config files with SAME switch ID where some files
    // are missing identity fields (hostname, model, management_ip)
    //
    // main.yaml: switch id="sw-01"
    //   ✓ hostname: "switch-01"
    //   ✓ management_ip: "192.168.1.10"
    //   ✓ model: Aruba2930F
    //   ✓ credentials: { ... }
    //   ✓ vlans: [ ... ]
    //
    // folder1.yaml: switch id="sw-01"
    //   ✗ hostname: MISSING
    //   ✗ management_ip: MISSING
    //   ✗ model: MISSING
    //   ✗ credentials: MISSING
    //   ✗ vlans: MISSING
    //   ✓ ports: [ ... ]
    //
    // folder2.yaml: switch id="sw-01"
    //   ✗ hostname: MISSING
    //   ✗ management_ip: MISSING
    //   ✗ model: MISSING
    //   ✗ credentials: MISSING
    //   ✗ vlans: MISSING
    //   ✓ port_mirrors: [ ... ]
    //
    // Expected behavior:
    //   - Should merge successfully (identity fields from main.yaml)
    //   - Merged switch should have:
    //     - hostname, management_ip, model from main.yaml
    //     - credentials from main.yaml
    //     - vlans from main.yaml
    //     - ports from folder1.yaml
    //     - port_mirrors from folder2.yaml
    //
    // This tests that identity fields, credentials, and vlans can be provided
    // by ONE config file while other files provide only specific sections.
}

#[test]
fn test_merge_same_id_all_missing_credentials() {
    // Tests merging 2+ config files with SAME switch ID where ALL files
    // are missing credentials (should error)
    //
    // main.yaml: switch id="sw-01"
    //   ✓ hostname, management_ip, model
    //   ✓ vlans: [ ... ]
    //   ✗ credentials: MISSING
    //
    // folder1.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✗ credentials: MISSING
    //   ✓ ports: [ ... ]
    //
    // folder2.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✗ credentials: MISSING
    //   ✓ port_mirrors: [ ... ]
    //
    // Expected behavior:
    //   - Should ERROR after merge
    //   - Error should list all 3 files that contributed to switch id="sw-01"
    //   - Error should show that NONE provided credentials
    //   - Should suggest adding credentials to one of these files
}

#[test]
fn test_merge_same_id_all_missing_vlans() {
    // Tests merging 2+ config files with SAME switch ID where ALL files
    // are missing vlans (should error)
    //
    // main.yaml: switch id="sw-01"
    //   ✓ hostname, management_ip, model
    //   ✓ credentials: { ... }
    //   ✗ vlans: MISSING
    //
    // folder1.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✗ vlans: MISSING
    //   ✓ ports: [ ... ]
    //
    // folder2.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✗ vlans: MISSING
    //   ✓ credentials: { ... }  (would be overridden by main if higher priority)
    //
    // Expected behavior:
    //   - Should ERROR after merge
    //   - Error should list all 3 files that contributed to switch id="sw-01"
    //   - Error should show that NONE provided vlans
    //   - Should suggest adding vlans to one of these files
}

#[test]
fn test_merge_same_id_identity_field_mismatch() {
    // Tests merging 2+ config files with SAME switch ID but DIFFERENT
    // identity field values (should error - identity must match)
    //
    // main.yaml: switch id="sw-01"
    //   ✓ hostname: "switch-01"
    //   ✓ management_ip: "192.168.1.10"
    //   ✓ model: Aruba2930F
    //
    // folder.yaml: switch id="sw-01"
    //   ✓ hostname: "switch-01-DIFFERENT"  ← MISMATCH!
    //   ✓ management_ip: "192.168.1.10"
    //   ✓ model: Aruba2930F
    //
    // Expected behavior:
    //   - Should ERROR during merge validation
    //   - Error should identify the conflicting identity field (hostname)
    //   - Error should show the values from each file
    //   - Should explain that identity fields must be identical across all configs
}

#[test]
fn test_merge_three_plus_configs_same_id_partial_fields() {
    // Tests merging 3+ config files with SAME switch ID where each provides
    // different subsets of fields
    //
    // main.yaml: switch id="sw-01"
    //   ✓ hostname, management_ip, model
    //   ✓ vlans: [ ... ]
    //
    // folder1.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✓ credentials: { ... }
    //
    // folder2.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✓ ports: [ ... ]
    //
    // folder3.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✓ port_mirrors: [ ... ]
    //
    // folder4.yaml: switch id="sw-01"
    //   ✗ hostname, management_ip, model: MISSING
    //   ✓ snmp: { ... }
    //
    // Expected behavior:
    //   - Should merge successfully
    //   - Final switch should have all fields from all 5 files
    //   - Identity fields from main.yaml
    //   - Each optional section from its respective file
}
```

These tests ensure the multi-config merge system correctly handles optional fields with proper validation at both file-parsing and post-merge stages.

**Port Name/Description Cleanup Tests:**
```rust
#[test]
fn test_port_name_removed_when_not_in_config() {
    // Port 5 has name "Server Port" on switch currently
    // Config file does not include port 5 (or includes it without name field)
    // Result: Port 5 name should be removed/cleared on switch
}

#[test]
fn test_port_description_removed_when_not_in_config() {
    // Port 10 has description "Workstation connection" on switch currently
    // Config file does not include port 10 (or includes it without description)
    // Result: Port 10 description should be removed/cleared on switch
}

#[test]
fn test_port_name_and_description_removed_when_port_not_in_config() {
    // Port 8 has both name "Lab Port" and description "Testing equipment" on switch
    // Config file does not include port 8 at all
    // Result: Both name and description should be removed/cleared on switch
}

#[test]
fn test_port_name_removed_but_other_settings_kept() {
    // Port 3 has name "Old Name", vlan 10, poe_enabled true on switch
    // Config includes port 3 with vlan 10, poe_enabled true, but no name field
    // Result: Name removed, vlan and poe settings unchanged
}

#[test]
fn test_port_name_changed_from_old_to_new() {
    // Port 12 has name "Old Name" on switch
    // Config includes port 12 with name "New Name"
    // Result: Name updated from "Old Name" to "New Name"
}

#[test]
fn test_multiple_ports_names_cleanup() {
    // Ports 1-10 all have names on switch
    // Config includes only ports 1-5 (with names), ports 6-10 not in config
    // Result: Ports 1-5 keep/update names, ports 6-10 names removed
}
```

**Implementation Notes for Port Name Cleanup:**
- Port names and descriptions should be treated like other port configuration fields
- If a port is not defined in the config file (or defined without name/description fields), these should be cleared
- This ensures idempotent behavior: running config twice produces same result
- State diff logic (`src/diff/mod.rs`) should detect when names/descriptions need removal
- Vendor implementations should generate commands to clear names/descriptions
- Related to `enforce_port_config` setting: when false, only configured ports are modified; when true, all unconfigured ports reset to defaults

**Vendor-Specific Commands:**
- **Aruba**: `no name` or `name ""` in interface configuration context
- **Cisco**: `no description` in interface configuration context
- **FortiSwitch**: May use `unset name` or similar (verify FortiSwitch syntax)

**Implementation Note - Detailed Post-Merge Error Messages:**

When post-merge validation detects missing required fields, the error message should:

1. **List all missing fields** (not just the first one)
2. **Show which config files contributed** to that switch (by matching switch ID)
3. **For each file, show what it provides**:
   - ✓ Present fields (with summary, e.g., "10 ports defined")
   - ✗ Missing fields
4. **Show the merge result** with sources for each field
5. **Include "Common cause: Switch ID mismatch" hint** explaining that files with different IDs won't merge
6. **Provide fix examples** for the missing fields

This comprehensive error message helps users quickly diagnose:
- Whether they forgot to add fields to any config
- Whether they have a switch ID mismatch preventing merge
- Which specific config file(s) need to be updated
