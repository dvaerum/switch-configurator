# Per-Port MAC Notification Control Implementation

## Summary

Successfully implemented and tested per-port MAC notification control for Aruba switches. Individual ports can now enable or disable MAC notification traps independently, even when the global SNMP `mac-notify` trap is enabled.

## Implementation Details

### Changes Made

#### 1. Removed Global `mac-notify traps all` Command
**File**: `src/vendors/aruba.rs` (lines 254-277)

**Problem**: The global `mac-notify traps all` command was enabling MAC notifications on ALL ports, overriding per-port settings.

**Solution**: Removed this command. Now only the global SNMP trap type is enabled (`snmp-server enable traps mac-notify`), allowing per-port control.

```rust
// Before (BROKEN):
if has_mac_notify {
    commands.push("mac-notify traps all".to_string());  // This breaks per-port control!
}

// After (FIXED):
// NOTE: We do NOT use "mac-notify traps all" here because it would enable
// MAC notifications globally on ALL ports, preventing per-port control.
```

#### 2. Fixed Per-Port Disable Commands
**File**: `src/vendors/aruba.rs` (lines 165-174)

**Problem**: The `no mac-notify` command doesn't remove explicit trap commands that were previously configured.

**Solution**: Use explicit disable commands for both trap types:

```rust
// Before (BROKEN):
else {
    commands.push("no mac-notify".to_string());  // Doesn't work!
}

// After (FIXED):
else {
    // Disable MAC notifications - must explicitly disable both trap types
    // Note: "no mac-notify" alone doesn't remove explicit trap commands
    commands.push("no mac-notify traps learned".to_string());
    commands.push("no mac-notify traps removed".to_string());
}
```

### How It Works

1. **Global SNMP Configuration**:
   - `snmp-server enable traps mac-notify` - Enables the SNMP trap type globally
   - **NO** `mac-notify traps all` - Removed to allow per-port control

2. **Per-Port Configuration**:
   - **Enabled** (`mac_notify: true`):
     ```
     interface X
       mac-notify traps learned
       mac-notify traps removed
     ```
   - **Disabled** (`mac_notify: false`):
     ```
     interface X
       no mac-notify traps learned
       no mac-notify traps removed
     ```

## Testing

### Unit Tests Created

Added 8 comprehensive unit tests in `src/vendors/aruba.rs`:

1. **test_per_port_mac_notify_enabled** - Verifies enable commands
2. **test_per_port_mac_notify_disabled** - Verifies disable commands
3. **test_per_port_mac_notify_mixed_configuration** - Tests multiple ports with different settings
4. **test_snmp_global_mac_notify_without_all_command** - Critical test: verifies no global override
5. **test_per_port_mac_notify_port_range** - Tests port range expansion with mixed settings
6. **test_per_port_mac_notify_parsing** - Verifies parser correctly identifies port states
7. **test_bug_fix_mac_notify_both_traps** - Updated existing test for enable
8. **test_bug_fix_mac_notify_disabled** - Updated existing test for disable

**All tests pass**: ✅ 8/8 tests passed

### Physical Device Testing

**Device**: Aruba 2930F (test-aruba-2930f)
**Connection**: Serial (`/dev/serial_aruba-2930F`)
**Date**: 2025-11-20

#### Test 1: Initial Implementation Test
- **Config**: `test-port-trap-disable.yaml`
- **Ports tested**: 1-4
- **Result**: ✅ Configuration applied successfully

#### Test 2: Current Production Config
- **Config**: `config.yaml`
- **Result**: ✅ Applied successfully with mixed MAC notify settings

#### Test 3: Final Verification Test
- **Config**: `final-verify-test.yaml`
- **Ports tested**: 5-8 with alternating settings
- **Commands verified**:
  - Port 5: `mac-notify traps learned` + `removed` ✅
  - Port 6: `no mac-notify traps learned` + `removed` ✅
  - Port 7: `mac-notify traps learned` + `removed` ✅
  - Port 8: `no mac-notify traps learned` + `removed` ✅
  - Global: `snmp-server enable traps mac-notify` (NO "traps all") ✅

### Verification via Parser

The configuration parser correctly identifies ports with different MAC notify settings:
- Ports 23-24: `mac_notify=false` ✅
- Other ports: `mac_notify=true` ✅

## Configuration Example

See `examples/per-port-mac-notify-control.yaml` for a complete example showing:
- Global SNMP trap configuration
- Ports with `mac_notify: true` (monitored devices)
- Ports with `mac_notify: false` (devices that don't need tracking)

## Benefits

1. **Fine-grained control**: Enable MAC tracking only on specific ports
2. **Reduced noise**: Disable traps on stable devices (security cameras, printers, etc.)
3. **Selective monitoring**: Track only dynamic/guest ports
4. **Backwards compatible**: Existing configs work unchanged

## Technical Notes

### Aruba Command Behavior

- **`mac-notify traps all`**: Enables MAC notifications on ALL ports globally (bad for per-port control)
- **`mac-notify traps learned/removed`**: Enables per-interface (good)
- **`no mac-notify traps learned/removed`**: Disables per-interface (good)
- **`no mac-notify`**: Does NOT remove explicit trap commands (doesn't work)

### Why This Implementation is Correct

The key insight is that Aruba switches have **two levels** of MAC notification control:

1. **Global SNMP trap type** (`snmp-server enable traps mac-notify`): Must be enabled to send any MAC traps
2. **Per-port notification** (`mac-notify traps learned/removed`): Controls which ports actually send traps

**BOTH settings must be enabled for a port to send MAC notification SNMP traps:**

| Global SNMP Trap | Port mac_notify | Result |
|------------------|-----------------|--------|
| ✅ Enabled | ✅ true | Port **SENDS** MAC notification traps |
| ✅ Enabled | ❌ false | Port **DOES NOT** send traps |
| ❌ Not enabled | ✅ true | Port **DOES NOT** send traps (global disabled) |
| ❌ Not enabled | ❌ false | Port **DOES NOT** send traps |

Think of it as **two gates in series**: the global SNMP trap opens the channel, and per-port settings allow specific ports to use that channel. Without the global trap enabled, no MAC notification traps will be sent at all, regardless of per-port settings.

The previous implementation mistakenly used `mac-notify traps all` which operates at a third level - overriding all per-port settings.

## Files Modified

- `src/vendors/aruba.rs`: Implementation and tests
- `examples/per-port-mac-notify-control.yaml`: Example configuration

## Files Created

- `test-port-trap-disable.yaml`: Initial test configuration
- `final-verify-test.yaml`: Final verification test
- `examples/per-port-mac-notify-control.yaml`: Example for documentation

## Conclusion

The per-port MAC notification control feature is **fully implemented, tested, and verified** on physical hardware. The implementation is robust with comprehensive unit tests ensuring it continues to work in future releases.
