# Aruba Serial Connection Parsing Fixes

**Date**: December 2025
**Status**: FIXED
**Issues**: ANSI escape codes breaking parsing, PoE commands on non-PoE switches, PoE parser defaults

## Summary

Three bugs were discovered affecting Aruba switch configuration via serial connections, causing the switch configurator to constantly reconfigure the entire switch instead of detecting no changes.

### Issue 1: ANSI Escape Code Contamination

**Problem**: Serial connections inject ANSI escape codes (cursor positioning, color codes, etc.) into the running configuration output. These codes appear inline with configuration text, breaking the parser.

**Symptoms**:
- Some interfaces not detected during parsing
- Port descriptions showing `None` when they should have values
- Parser logs showing fewer ports than actually configured

**Example of contaminated output**:
```
\x1b[24;1H\x1b[2K\x1b[24;1H\x1b[1;24r\x1b[24;1Hinterface 11
   name "Zone 1"
   untagged vlan 4
   exit
```

The `interface 11` line starts with escape codes, so `line.starts_with("interface ")` returns false.

**Root Cause**: `parse_current_state()` and `parse_running_config()` were checking line prefixes without first stripping ANSI sequences.

**Fix**: Added ANSI escape code pre-processing at the beginning of both functions:

```rust
// Pre-process config to strip ANSI escape sequences (common with serial connections)
let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
let clean_config = ansi_regex.replace_all(&config, "");
let lines: Vec<&str> = clean_config.lines().collect();
```

**Location**: `src/vendors/aruba.rs`
- `parse_current_state()` - lines ~673-676
- `parse_running_config()` (test helper) - lines ~1754-1757

### Issue 2: PoE Commands on Non-PoE Switch Models

**Problem**: The Aruba2540_48G_4SFP and similar non-PoE switch models were receiving PoE configuration commands, which fail with "Invalid input" errors. This caused a constant diff between:
- Desired config: `poe_enabled: false`
- Parsed state: `poe_enabled: true` (default when PoE commands not found)

**Symptoms**:
- Every configuration run attempts to configure PoE settings
- Commands like `no power-over-ethernet` fail on the switch
- Configuration appears to always have changes even when nothing changed

**Root Cause**: `generate_port_commands()` and `reset_ports()` were unconditionally generating PoE commands without checking if the switch model supports PoE.

**Fix**: Added model capability check before generating PoE commands:

```rust
// Only generate PoE commands if the switch model supports PoE
if self.config.model().supports_poe() {
    if port.poe_enabled {
        commands.push("power-over-ethernet".to_string());
    } else {
        commands.push("no power-over-ethernet".to_string());
    }
}
```

**Location**: `src/vendors/aruba.rs`
- `generate_port_commands()` - lines ~209-220
- `reset_ports()` - lines ~1912-1916

### Issue 3: Parser Defaulting PoE to True for Non-PoE Switches

**Problem**: Even after fixing the command generation (Issue 2), the switch configurator was still reconfiguring all ports on every run. The parser was defaulting `poe_enabled: true` for all ports because non-PoE switches have no PoE configuration lines to parse.

**Root Cause**: In `parse_interface_name()`, the default value for `poe_enabled` was hardcoded to `true`:
```rust
let mut poe_enabled = true;  // Always true, even for non-PoE switches!
```

This caused a permanent diff when the desired config had `poe_enabled: false`:
- Parsed state: `poe=true` (from default)
- Desired config: `poe=false`
- Result: Port marked for reconfiguration every time

**Fix**: Made the parser model-aware by using `supports_poe()` for the default:

```rust
// For non-PoE switches, default to false since there's no PoE configuration to parse
// For PoE switches, default to true (PoE is enabled by default unless "no power-over-ethernet" is present)
let mut poe_enabled = self.config.model().supports_poe();
```

**Location**: `src/vendors/aruba.rs`
- `parse_interface_name()` - line ~502

### Issue 4: Diff Computation Not Model-Aware for PoE

**Problem**: Even after all the above fixes, the switch configurator was still detecting ports as needing configuration on every run. The YAML config file had `poe_enabled: true` for ports on a non-PoE switch (user mistake or default value), causing a permanent diff.

**Symptoms**:
- All ports constantly detected as needing configuration
- Debug logs showing: `Current: poe=false, Desired: poe=true`
- Unnecessary configuration traffic on every run

**Root Cause**: The `compute_diff()` function in `src/diff/mod.rs` was comparing `poe_enabled` values without considering whether the switch supports PoE. For non-PoE switches, this comparison is irrelevant.

**Fix**: Made the diff computation model-aware by adding `ports_equivalent_for_model()` function that skips `poe_enabled` comparison for non-PoE switches:

```rust
// Check if the switch model supports PoE
let supports_poe = desired.model.as_ref().map_or(true, |m| m.supports_poe());

// In ports_equivalent_for_model():
// Only compare poe_enabled if the switch supports PoE
// For non-PoE switches, ignore any poe_enabled differences
if supports_poe && current.poe_enabled != desired.poe_enabled {
    return false;
}
```

**Location**: `src/diff/mod.rs`
- `diff_ports()` - lines ~121-122
- `ports_equivalent_for_model()` - lines ~176-215

### Non-PoE Switch Models

The following Aruba switch models do NOT support PoE:
- `Aruba2530_48G_2SFP` (J9855A)
- `Aruba2540_24G`
- `Aruba2540_48G_4SFP` (JL355A)

PoE-capable models:
- `Aruba2530_24G_POE`
- `Aruba2530_8G_POE`
- `Aruba2930F`

## Unit Tests Added

### ANSI Escape Code Tests

1. **`test_ansi_escape_code_stripping_in_interface_lines`**
   - Verifies ports are parsed correctly when ANSI codes appear before "interface" keyword
   - Uses real-world example from Aruba switch serial output

2. **`test_ansi_escape_code_stripping_various_sequences`**
   - Tests multiple ANSI sequence types: cursor positioning, clear line, scroll region, colors
   - Verifies VLANs and ports parse correctly with embedded codes

3. **`test_ansi_escape_code_stripping_preserves_valid_content`**
   - Ensures legitimate bracket content (like `[rack-1]` in descriptions) is NOT stripped
   - Only ANSI sequences (starting with ESC char `\x1b`) are removed

### PoE Command Tests

1. **`test_non_poe_switch_no_poe_commands_in_port_config`**
   - Verifies non-PoE switches generate zero PoE-related commands
   - Uses Aruba2540_48G_4SFP model

2. **`test_non_poe_switch_no_poe_commands_with_poe_disabled`**
   - Verifies even with `poe_enabled: false`, no PoE commands generated

3. **`test_poe_switch_generates_poe_commands`**
   - Verifies PoE switches (Aruba2930F) DO generate `power-over-ethernet` command

4. **`test_poe_switch_generates_no_poe_command_when_disabled`**
   - Verifies PoE switches generate `no power-over-ethernet` when disabled

5. **`test_non_poe_switch_model_detection`**
   - Verifies `SwitchModel::supports_poe()` returns correct values for all models

### PoE Parser Default Tests

1. **`test_non_poe_switch_parser_defaults_poe_false`**
   - Verifies non-PoE switches parse `poe_enabled` as `false` by default
   - Uses Aruba2540_48G_4SFP model (non-PoE)

2. **`test_poe_switch_parser_defaults_poe_true`**
   - Verifies PoE switches parse `poe_enabled` as `true` by default
   - Uses Aruba2930F model (PoE)

3. **`test_poe_switch_parser_respects_no_poe_command`**
   - Verifies `no power-over-ethernet` in config correctly sets `poe_enabled: false`
   - Tests both explicit disable and default enable on same PoE switch

### Diff Model-Aware PoE Tests (in `src/diff/mod.rs`)

1. **`test_non_poe_switch_ignores_poe_enabled_diff`**
   - Verifies non-PoE switches ignore `poe_enabled` differences in diff computation
   - Uses Aruba2540_48G_4SFP model (non-PoE)

2. **`test_poe_switch_detects_poe_enabled_diff`**
   - Verifies PoE switches DO detect `poe_enabled` differences
   - Uses Aruba2930F model (PoE)

3. **`test_ports_equivalent_for_model_non_poe_ignores_poe`**
   - Direct unit test for `ports_equivalent_for_model()` function
   - Tests both supports_poe=true and supports_poe=false

4. **`test_non_poe_switch_still_detects_other_differences`**
   - Verifies non-PoE switches still detect VLAN, description, speed_duplex changes
   - Only PoE differences should be ignored

## Testing Commands

Run all Aruba vendor tests including the new ones:
```bash
cargo test --lib vendors::aruba::tests
```

Run just the ANSI tests:
```bash
cargo test --lib vendors::aruba::tests::test_ansi
```

Run just the PoE tests:
```bash
cargo test --lib vendors::aruba::tests::test_non_poe
cargo test --lib vendors::aruba::tests::test_poe_switch
```

## Files Modified

- `src/vendors/aruba.rs`:
  - Added ANSI stripping to `parse_current_state()` and `parse_running_config()`
  - Added `supports_poe()` check in `generate_port_commands()` and `reset_ports()`
  - Made `parse_interface_name()` model-aware for PoE default
  - Added 11 new unit tests (3 ANSI + 5 PoE command + 3 PoE parser)

- `src/diff/mod.rs`:
  - Added `ports_equivalent_for_model()` function for model-aware PoE comparison
  - Modified `diff_ports()` to skip PoE comparison for non-PoE switches
  - Added 4 new unit tests for model-aware diff computation

## Impact

After these fixes:
- Serial connections to Aruba switches parse configuration correctly
- Non-PoE switches no longer receive invalid PoE commands
- Configuration runs are truly idempotent (no changes = no commands)
- Significant reduction in unnecessary configuration traffic
- User config errors (like `poe_enabled: true` on non-PoE switches) no longer cause constant reconfiguration
