# Known Issues

## SNMP Configuration Idempotency (Minor)

**Status**: Non-blocking, cosmetic issue
**Priority**: Low
**Affected Component**: Aruba vendor SNMP parsing (`src/vendors/aruba.rs`)

### Description
The SNMP configuration parsing detects 2 trap receivers on subsequent runs even when only 1 is configured, causing the system to think SNMP configuration needs updating on every run.

### Impact
- SNMP configuration section gets reconfigured on every run (~10 seconds extra)
- No functional impact - correct configuration is still applied
- Switch state ends up correct after each run

### Current Behavior
```
Parsed SNMP: 2 communities, 2 trap receivers, 1 traps
SNMP config changed: true
```

Expected behavior: Should detect `1 trap receiver` and `SNMP config changed: false` on second run.

### Root Cause
The `parse_snmp_config()` function in `src/vendors/aruba.rs` may be:
1. Parsing trap receiver configuration from multiple sections of running-config
2. Not correctly detecting when old trap receivers have been removed
3. Possibly caching or reading stale state

### Workaround
None needed - system functions correctly despite the issue.

### Future Fix
To resolve this issue:
1. Add debug logging to `parse_snmp_config()` to see exactly what lines it's parsing
2. Verify that `show running-config` output doesn't contain duplicate trap receiver entries
3. Check if the removal commands in `configure_snmp()` are working correctly
4. Consider adding more specific parsing logic to handle removal/addition of trap receivers

### Test Case
```bash
# Run twice and verify second run shows no changes
./result/bin/switch-configurator --config-file config.yaml --one-off --log-level debug
./result/bin/switch-configurator --config-file config.yaml --one-off --log-level debug
# Expected: Second run should show "No state differences detected"
# Actual: Second run shows "SNMP config changed: true"
```

### Related Files
- `src/vendors/aruba.rs` - `parse_snmp_config()` and `configure_snmp()`
- `src/diff/mod.rs` - `diff_snmp()` comparison logic
- `src/models.rs` - `SnmpConfig`, `SnmpTrapReceiver` structures

---

Last Updated: 2025-11-07
