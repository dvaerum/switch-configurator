# Aruba SNMP Trap Behavior Documentation

**Date**: 2025-11-24
**Switch Model**: Aruba/HP J9855A 2530-48G-2SFP+
**Firmware**: ArubaOS-Switch (multiple versions tested: 16.05-16.11)

## Executive Summary

Link-change (linkUp/linkDown) SNMP traps are **ENABLED BY DEFAULT** on Aruba 2530 switches. This causes the enable command to not appear in `show running-config` because Aruba only shows non-default configurations. This is normal, documented behavior - not a bug.

## Default Trap Status

### Enabled by Default

- ✅ **link-change** (linkUp/linkDown) - Port state change notifications
- ✅ **SNMP Authentication** - Invalid community string attempts
- ✅ **Password change** - Manager password modifications

### Disabled by Default

- ❌ **mac-notify** - MAC address table changes (must be explicitly enabled)
- ❌ **Startup Config change** - Configuration file changes
- ❌ **Running Config Change** - Active configuration modifications
- ❌ **MAC Address Count** - MAC table size changes

## Link-Change Trap Behavior

### Configuration Visibility

Aruba switches follow this pattern for ALL configuration commands:
- **Default settings**: Do NOT appear in `show running-config`
- **Non-default settings**: DO appear in `show running-config`

### Example Behavior

```bash
# Initial state (factory default)
switch# show running-config | include link-change
(no output - enabled by default, so not shown)

# Enable command (redundant with default)
switch(config)# snmp-server enable traps link-change all
switch(config)# show running-config | include link-change
(no output - still default behavior)

# Disable command (non-default)
switch(config)# no snmp-server enable traps link-change all
switch(config)# show running-config | include link-change
no snmp-server enable traps link-change 1-50
```

**Key Observation**: The `no` command DOES appear because disabling is non-default!

### Port List Expansion

When disabling, Aruba expands "all" to the actual port range:
- `no snmp-server enable traps link-change all`
- → `no snmp-server enable traps link-change 1-50` (in running-config)

For Aruba 2530-48G-2SFP+:
- Ports 1-48: Copper GigE ports
- Ports 49-50: SFP+ uplink ports

## Verification Commands

### Method 1: show snmp-server traps

Most reliable method to see actual trap status:

```bash
switch(config)# show snmp-server traps

 Trap Receivers

  Link-Change Traps Enabled on Ports [All] : 1,4

  Traps Category                          Current Status
  _____________________________________   __________________
  SNMP Authentication                   : Extended
  Password change                       : Enabled
  Login failures                        : Enabled
  Port-Security                         : Enabled
  ...
  MAC address table changes             : Disabled
  MAC Address Count                     : Disabled

  Address                Community              Events   Type   Retry   Timeout
  ---------------------- ---------------------- -------- ------ ------- -------
  192.168.1.1            public                 None     trap   3       15
```

**What this shows**:
- `Link-Change Traps Enabled on Ports [All]` - Confirms default enabled state
- `: 1,4` - Ports currently with active link state being monitored
- `MAC address table changes : Disabled` - Confirms mac-notify is NOT default

### Method 2: Negative Test

Try to disable and check running-config:

```bash
switch(config)# no snmp-server enable traps link-change all
switch(config)# show running-config | include link-change
no snmp-server enable traps link-change 1-50    # ← Appears!

# Re-enable to restore default
switch(config)# snmp-server enable traps link-change all
switch(config)# show running-config | include link-change
(no output - back to default)
```

## Comparison: mac-notify vs link-change

| Aspect | mac-notify | link-change |
|--------|-----------|-------------|
| **Default State** | Disabled | **Enabled** |
| **Enable Command Visible** | Yes (non-default) | **No (is default)** |
| **Disable Command Visible** | No (is default) | **Yes (non-default)** |
| **Per-Port Control** | Yes (port-level) | Yes (port-list) |
| **Global Enable Required** | Yes | No (already on) |

### Why This Matters for switch-configurator

1. **mac-notify**:
   - Must send: `snmp-server enable traps mac-notify`
   - Will appear in running-config
   - Parser SHOULD find it

2. **link-change**:
   - CAN send: `snmp-server enable traps link-change all` (harmless)
   - Will NOT appear in running-config
   - Parser will NOT find it (this is correct!)

## Implementation Considerations

### Current Implementation (As of 2025-11-24)

The switch-configurator now uses `show snmp-server traps` command to detect actual trap status:

1. ✅ Executes `show snmp-server traps` to get real trap status (not assumptions)
2. ✅ Parses output to determine if link-change is enabled:
   - `Link-Change Traps Enabled on Ports [All] : None` → disabled
   - `Link-Change Traps Enabled on Ports [All] : All` or port list → enabled
3. ✅ Only sends enable/disable commands when state needs to change
4. ✅ Fully idempotent - no unnecessary commands sent
5. ✅ Works correctly with Aruba's default-enabled behavior

### Implementation Details

**New Method**: `parse_snmp_trap_status()` (src/vendors/aruba.rs:604-653)
- Executes `show snmp-server traps` command
- Parses both link-change and mac-notify trap status
- Returns `(link_change_enabled: bool, mac_notify_enabled: bool)`

**Updated Method**: `parse_snmp_config()` (src/vendors/aruba.rs:655-730)
- Now accepts `link_change_enabled` and `mac_notify_enabled` parameters
- Adds traps to `enabled_traps` list based on actual status (not assumptions)
- Still parses communities and trap receivers from running-config

**Integration Points**:
- `parse_current_state()`: Calls `parse_snmp_trap_status()` before parsing config
- `configure_snmp()`: Uses actual trap status to determine what changes are needed

### Parser Logic (Implemented)

```rust
// Actual implementation in src/vendors/aruba.rs
async fn parse_snmp_trap_status(&mut self) -> Result<(bool, bool), VendorError> {
    let show_cmd = vec!["show snmp-server traps".to_string()];
    let outputs = self.client.execute_commands(&show_cmd).await?;
    let output = outputs.get(0).unwrap_or(&String::new());

    let mut link_change_enabled = false;
    let mut mac_notify_enabled = false;

    for line in output.lines() {
        if line.contains("Link-Change Traps Enabled on Ports") {
            link_change_enabled = !line.contains(": None");
        }
        if line.contains("MAC address table changes") {
            mac_notify_enabled = line.contains("Enabled");
        }
    }

    Ok((link_change_enabled, mac_notify_enabled))
}
```

### Testing

**Unit Tests** (7 new tests added):
- `test_parse_snmp_config_with_link_change_enabled`
- `test_parse_snmp_config_with_link_change_disabled`
- `test_parse_snmp_config_with_both_traps_enabled`
- `test_parse_snmp_config_with_both_traps_disabled`
- `test_generate_snmp_commands_link_change_already_enabled`
- `test_generate_snmp_commands_disable_link_change`
- `test_generate_snmp_commands_enable_link_change_when_disabled`

All 122 library unit tests passing as of 2025-11-24.

## Command Syntax Reference

### Enable Commands

```bash
# Enable on all ports (redundant with default, but accepted)
snmp-server enable traps link-change all

# Enable on specific ports
snmp-server enable traps link-change 1-24
snmp-server enable traps link-change 1,5,10-20,48
```

### Disable Commands

```bash
# Disable on all ports
no snmp-server enable traps link-change all
# Result in config: "no snmp-server enable traps link-change 1-50"

# Disable on specific ports
no snmp-server enable traps link-change 1-24
```

## Official Documentation References

### HPE/Aruba Sources

From HPE Aruba Command Reference (AOS-S 16.05-16.11):

> "By default, a switch is enabled to send a trap when the link state on a port changes from up to down (linkDown) or down to up (linkUp)."

### Configuration Guides

- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.05
- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.06
- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.07
- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.09
- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.10
- Aruba 2530 Management and Configuration Guide for AOS-Switch 16.11

All versions confirm the default enablement of link-change traps.

## Testing & Verification

### Manual Testing Performed

**Date**: 2025-11-24
**Switch**: Aruba J9855A 2530-48G-2SFP+
**Connection**: Serial console (115200 baud)

**Test Sequence**:

1. **Baseline State Check**:
   ```bash
   show running-config | include link-change
   # Result: (empty)

   show snmp-server traps
   # Result: Link-Change Traps Enabled on Ports [All] : 1,4
   ```

2. **Enable Command Test**:
   ```bash
   configure terminal
   snmp-server enable traps link-change all
   exit
   show running-config | include link-change
   # Result: (empty - command accepted but not shown)
   ```

3. **Disable Command Test**:
   ```bash
   configure terminal
   no snmp-server enable traps link-change all
   exit
   show running-config | include link-change
   # Result: no snmp-server enable traps link-change 1-50
   ```

4. **Re-Enable Test**:
   ```bash
   configure terminal
   snmp-server enable traps link-change all
   exit
   show running-config | include link-change
   # Result: (empty - back to default)
   ```

### Automated Testing

Comprehensive tests performed with switch-configurator:

**Test 1**: Enable traps (dry-run)
- Commands generated correctly
- No `linkUp-linkDown` legacy command ✓

**Test 2**: Enable traps (actual hardware)
- Commands executed successfully
- Configuration saved
- No errors reported ✓

**Test 3**: Disable traps (dry-run)
- Disable commands generated correctly
- `no snmp-server enable traps mac-notify` ✓

**Test 4**: Disable traps (actual hardware)
- Disable commands executed successfully
- Configuration saved
- Verified with `show snmp-server traps` ✓

All tests passing. See test logs in `/tmp/test*.log`.

## Historical Context

### Legacy `linkUp-linkDown` Syntax

Previous code attempted to send both syntaxes:
- `snmp-server enable traps link-change all`
- `snmp-server enable traps linkUp-linkDown`

**Investigation Results**:
- The `linkUp-linkDown` syntax causes "Invalid input" errors on Aruba 2530-48G
- It was removed from command generation (2025-11-24)
- Parser still recognizes it for backward compatibility with other models

### Bug Investigation and Resolution History

- **2025-11-11**: Initial bug report - "link-change command not persisting"
- **2025-11-24**: Root cause discovered - default enabled behavior
- **2025-11-24**: Confirmed via web search of official documentation
- **2025-11-24**: Verified manually on hardware
- **2025-11-24**: Updated documentation and bug report
- **2025-11-24**: **IMPLEMENTED SOLUTION**:
  - Added `parse_snmp_trap_status()` method to query actual trap status
  - Updated `parse_snmp_config()` to accept trap status parameters
  - Modified `parse_current_state()` and `configure_snmp()` to use new method
  - Added 7 comprehensive unit tests
  - All 122 library tests passing
  - Implementation now fully idempotent

## Conclusion

**This is NOT a bug** - it's normal, documented Aruba switch behavior. The solution is to query actual trap status using `show snmp-server traps`:

1. ✅ Link-change traps are enabled by default
2. ✅ Default settings don't appear in running-config
3. ✅ The enable command is accepted but has no visible effect
4. ✅ The disable command DOES appear (non-default)
5. ✅ Traps function correctly in both states

**Implementation Status (COMPLETED 2025-11-24)**:
- ✅ Uses `show snmp-server traps` to determine actual trap status
- ✅ Only sends enable/disable commands when state needs to change
- ✅ Fully idempotent - no unnecessary commands
- ✅ Handles both link-change (default enabled) and mac-notify (default disabled)
- ✅ Comprehensive unit test coverage
- ✅ Verified on actual hardware

---

**Document Version**: 2.0
**Last Updated**: 2025-11-24
**Status**: IMPLEMENTATION COMPLETE
**Verified By**:
- Manual hardware testing
- Web search of official HPE documentation
- Unit test coverage (7 new tests, 122 total passing)
- Hardware verification of enable/disable functionality

**Related Files**:
- `src/vendors/aruba.rs` (SNMP parser/generator implementation)
  - Lines 604-653: `parse_snmp_trap_status()` method
  - Lines 655-730: Updated `parse_snmp_config()` method
  - Lines 2651-2853: 7 new comprehensive unit tests
- `docs/development/bug-investigation-report.md` (Bug #3)
