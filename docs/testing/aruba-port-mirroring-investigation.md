# Aruba Port Mirroring Bug Investigation

**Date**: November 25, 2025
**Status**: ✅ FIXED
**Issue**: Only one source port retains the `monitor` command after configuration

## Summary

The bug was **confirmed and fixed**. When configuring port mirroring with multiple source ports (33, 34, 35, 36), only one port (34) would retain the `monitor` command in the running configuration. The other three ports would lose their monitor settings.

### Root Cause

Ports were being configured **twice**:
1. First pass: Port properties (name, VLAN, speed, PoE, etc.) via `generate_port_commands()`
2. Second pass: Mirror monitor commands via `generate_mirror_commands()`

Aruba switches **clear previous interface settings** when an interface block is reconfigured. The second configuration pass (adding monitor commands) was wiping out settings from the first pass, and vice versa.

### The Fix

Modified `src/vendors/aruba.rs` to include monitor commands **within the same interface configuration block** as other port properties:

**Before**:
```rust
// First: configure port properties
interface 33
  name "IoT - Zone 1"
  speed-duplex auto
exit

// Later: add monitor command (CLEARS previous settings!)
interface 33
  monitor all both mirror 1
exit
```

**After**:
```rust
// All settings in ONE interface block
interface 33
  name "IoT - Zone 1"
  speed-duplex auto
  monitor all both mirror 1   // ← Added here!
exit
```

## Investigation Steps

### 1. Confirming the Bug

**Manual Test on Real Hardware** (test-aruba-2540-48g-4sfp):
- Configured port mirroring: source ports 33-36 → destination port 42
- Ran `show running-config`
- **Result**: Only port 34 had `monitor` command, ports 33, 35, 36 missing

### 2. Analyzing Command Execution

**Debug Log Analysis**:
```
Command: interface 33
Command: name "IoT - Zone 1"
Command: exit
...
(Later)
Command: interface 33   ← Reconfiguring the SAME port!
Command: monitor all both mirror 1
Command: exit
```

**Problem**: Entering the same interface twice causes Aruba to reset uncommitted settings.

### 3. Root Cause Identification

**Code Structure** (`src/vendors/aruba.rs`):
- `apply_diff()` line 1319: Calls `configure_ports()` → generates port property commands
- `apply_diff()` line 1337: Calls `configure_port_mirrors()` → generates monitor commands
- Both functions call `generate_port_commands()` and `generate_mirror_commands()` separately
- Each generates independent `interface X` blocks → **double configuration**

## The Solution

### Code Changes

**1. Modified `generate_port_commands()` signature** (line 84):
```rust
// Before
fn generate_port_commands(&self, ports: &[Port]) -> Vec<String>

// After
fn generate_port_commands(&self, ports: &[Port], mirrors: &[PortMirror]) -> Vec<String>
```

**2. Added monitor command generation to port config** (lines 87-97, 216-226):
```rust
// Build lookup map of which ports are mirror sources
let mut port_mirror_map = HashMap::new();
for mirror in mirrors {
    for source_port in &mirror.source_ports {
        port_mirror_map.insert(source_port.clone(), (mirror.session_id.clone(), mirror.direction.clone()));
    }
}

// In port configuration loop, before exit:
if let Some((session_id, direction)) = port_mirror_map.get(&port.port_id) {
    let direction_cmd = match direction {
        MirrorDirection::Both => format!("monitor all both mirror {}", session_id),
        MirrorDirection::Rx => format!("monitor all in mirror {}", session_id),
        MirrorDirection::Tx => format!("monitor all out mirror {}", session_id),
    };
    commands.push(direction_cmd);
}
```

**3. Simplified `generate_mirror_commands()`** (lines 235-253):
- Now ONLY generates global mirror destination: `mirror <session> port <dest>`
- Removed all per-interface monitor command generation
- Added comment explaining the change

**4. Updated `configure_ports()` call** (line 1396):
```rust
// Pass mirrors so port config knows which ports need monitor commands
self.generate_port_commands(&diff.ports_to_configure, &self.config.port_mirrors)
```

### Test Updates

**Updated 3 existing tests**:
1. `test_generate_mirror_commands()`: Verify NO interface commands generated
2. `test_bug_fix_port_mirroring_syntax()`: Same verification
3. `test_bug_fix_port_mirroring_directions()`: Changed to test via `generate_port_commands()`

**Rewrote comprehensive test**:
4. `test_port_mirroring_four_source_ports()`:
   - Creates 4 Port structs (33, 34, 35, 36) with correct field order
   - Calls `generate_port_commands()` with mirrors
   - Verifies each port has monitor command in its interface block
   - Verifies exactly 4 monitor commands total
   - Verifies `generate_mirror_commands()` only has global command

**All 45 Aruba vendor tests pass** ✅

## Hardware Validation

**Test Configuration**:
```yaml
port_mirrors:
  - session_id: "1"
    source_ports: ["33", "34", "35", "36"]
    destination_port: "42"
    direction: both
```

**Commands Generated** (via `generate_port_commands()`):
```
interface 33
  name "IoT - Zone 1"
  monitor all both mirror 1
exit
interface 34
  name "IoT - Zone 1"
  monitor all both mirror 1
exit
interface 35
  name "IoT - Zone 1"
  monitor all both mirror 1
exit
interface 36
  name "IoT - Zone 1"
  monitor all both mirror 1
exit
```

**Commands Generated** (via `generate_mirror_commands()`):
```
mirror 1 port 42
```

**Hardware Test Result**: ✅ **All 4 ports retain monitor commands in running-config**

```
switch# show running-config

interface 33
   name "IoT - Zone 1"
   monitor              ✅
   exit
interface 34
   name "IoT - Zone 1"
   monitor              ✅
   exit
interface 35
   name "IoT - Zone 1"
   monitor              ✅
   exit
interface 36
   name "IoT - Zone 1"
   monitor              ✅
   exit

mirror-port 42          ✅
```

## Key Insights

1. **Aruba Behavior**: Aruba switches reset uncommitted interface settings when you enter the same interface block again
2. **Fix Strategy**: Consolidate ALL port settings (properties + monitor) into a single interface block
3. **Architecture**: Port mirroring is now split into two operations:
   - **Global destination**: Set via `generate_mirror_commands()` → `mirror <session> port <dest>`
   - **Source monitoring**: Set via `generate_port_commands()` → `monitor all <direction> mirror <session>`

## Aruba Mirror Syntax Variations

**Date Updated**: December 2025

Aruba switches use two different syntaxes for port mirroring depending on the model:

### Legacy Syntax (2530/2540 Series)
```
mirror-port 42
interface 33
   monitor
   exit
```

### Newer Syntax (2930F and Later)
```
mirror 1 port 42
interface 33
   monitor all both mirror 1
   exit
```

### Parser Support

The parser (`parse_current_state()`) now handles **both syntaxes** automatically:
- Detects `mirror-port <destination>` (legacy)
- Detects `mirror <session-id> port <destination>` (newer)
- Per-interface `monitor` commands identify source ports in both cases

### Command Generation

When generating commands, the service always uses the **newer syntax**:
```
mirror 1 port <destination>
```

This is compatible with all supported Aruba models.

### Unit Tests

The following tests verify mirror parsing (in `src/vendors/aruba.rs`):
- `test_parse_legacy_mirror_port_syntax` - Legacy syntax with multiple sources
- `test_parse_legacy_mirror_port_syntax_single_source` - Single source port
- `test_parse_legacy_mirror_port_no_monitor_sources` - Destination without sources
- `test_parse_new_mirror_syntax` - Newer `mirror 1 port` syntax
- `test_parse_mirror_port_with_whitespace` - Whitespace handling
- `test_parse_no_mirror_config` - No mirror configuration

## Benefits of the Fix

1. ✅ **Persistence**: Monitor commands survive port reconfigurations
2. ✅ **Idempotency**: Running the same config twice produces identical results
3. ✅ **Consistency**: All 4 source ports get configured identically
4. ✅ **Efficiency**: One interface entry per port instead of two

## Files Modified

- `src/vendors/aruba.rs`:
  - Modified `generate_port_commands()` to include monitor commands
  - Simplified `generate_mirror_commands()` to only set global destination
  - Updated `configure_ports()` to pass mirrors
  - Rewrote/updated 4 mirror-related tests
  - All 45 tests passing

## Related Documentation

- examples/port-mirroring.yaml - Port mirroring configuration examples
- docs/development/architecture.md - Vendor implementation patterns
- CLAUDE.md - Port mirroring syntax documentation
