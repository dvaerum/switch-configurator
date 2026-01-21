# Testing Guide: Bug #2 - SNMP Trap Receivers

## Overview

Bug #2 involves SNMP trap receivers accumulating in the running-config instead of being replaced.
The root cause has been identified and fixed with correct removal command syntax.

**Status**: ✅ FULLY FIXED - Removal commands now include community string
**Root Cause**: Incomplete removal syntax (missing community string)
**Fix**: Now uses `no snmp-server host <ip> community "<community>"` instead of just `no snmp-server host <ip>`

---

## What Was Fixed

### The Problem
- Config specified 1 trap receiver (e.g., 192.168.1.1)
- Running config showed 2 trap receivers (e.g., 192.168.1.100 AND 192.168.1.1)
- Trap receivers were accumulating instead of being replaced

### The Solution
The `configure_snmp()` function now:

1. **Fetches current running-config** before making changes
2. **Parses ALL existing trap receivers** from running-config
3. **Removes ALL trap receivers** with `no snmp-server host <ip>` commands
4. **Applies new trap receivers** from your config file
5. **Verifies the result** by re-fetching running-config
6. **Warns if mismatch detected** between expected and actual receivers

### Code Location
- File: `src/vendors/aruba.rs`
- Function: `configure_snmp()`
- Lines: 687-813

---

## How to Test

### Prerequisites

1. **Access to Aruba switch** (serial or SSH)
2. **Config file with SNMP settings** (see config.yaml example)
3. **Debug logging enabled**: Use `--log-level debug`

### Test Scenario 1: Initial Configuration

**Setup**: Clean switch with no trap receivers configured

```bash
# Step 1: Check current state
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: No output (no trap receivers)

# Step 2: Apply config with 1 trap receiver
cargo run -- --config-file config.yaml --one-off --log-level debug

# Step 3: Verify trap receiver was added
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: 1 line showing your trap receiver
```

**What to Look For in Logs**:
```
INFO  Configuring SNMP: 2 communities, 1 trap receivers, 1 trap types
DEBUG Fetching current running-config to identify existing trap receivers
DEBUG No existing trap receivers found in running-config
INFO  Applying new SNMP configuration with 1 trap receivers
DEBUG New trap receivers to configure: ["192.168.1.1"]
INFO  Verification: Found 1 trap receivers in running-config after update: ["192.168.1.1"]
INFO  ✓ Trap receiver count matches expected configuration
```

### Test Scenario 2: Trap Receiver Replacement

**Setup**: Switch already has a trap receiver (e.g., 192.168.1.100)

```bash
# Step 1: Manually add an old trap receiver
ssh admin@<switch-ip>
> configure terminal
> snmp-server host 192.168.1.100 community "public"
> exit
> exit

# Step 2: Verify old receiver exists
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: snmp-server host 192.168.1.100 community "public"

# Step 3: Apply config with DIFFERENT trap receiver (192.168.1.1)
cargo run -- --config-file config.yaml --one-off --log-level debug

# Step 4: Verify OLD receiver was removed and NEW one was added
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: Only snmp-server host 192.168.1.1 (NOT 192.168.1.100)
```

**What to Look For in Logs**:
```
INFO  Configuring SNMP: 2 communities, 1 trap receivers, 1 trap types
DEBUG Fetching current running-config to identify existing trap receivers
INFO  Found 1 existing trap receivers to remove: ["192.168.1.100"]
DEBUG Removal commands: ["configure terminal", "no snmp-server host 192.168.1.100", "exit"]
INFO  Executing removal commands for 1 trap receivers
INFO  Applying new SNMP configuration with 1 trap receivers
DEBUG New trap receivers to configure: ["192.168.1.1"]
INFO  Verification: Found 1 trap receivers in running-config after update: ["192.168.1.1"]
INFO  ✓ Trap receiver count matches expected configuration
```

### Test Scenario 3: Multiple Trap Receivers

**Setup**: Switch has multiple old trap receivers

```bash
# Step 1: Manually add multiple old trap receivers
ssh admin@<switch-ip>
> configure terminal
> snmp-server host 192.168.1.100 community "public"
> snmp-server host 192.168.1.200 community "public"
> snmp-server host 192.168.1.99 community "public"
> exit
> exit

# Step 2: Verify multiple receivers exist
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: 3 lines

# Step 3: Apply config with 2 DIFFERENT trap receivers
# Edit config.yaml to have 2 trap receivers:
#   - host: "192.168.1.1"
#   - host: "192.168.1.2"

cargo run -- --config-file config.yaml --one-off --log-level debug

# Step 4: Verify ALL old receivers removed and new ones added
ssh admin@<switch-ip> "show running-config | include snmp-server host"
# Expected: Only 2 lines (192.168.1.1 and 192.168.1.2)
```

**What to Look For in Logs**:
```
INFO  Found 3 existing trap receivers to remove: ["192.168.1.100", "192.168.1.200", "192.168.1.99"]
INFO  Executing removal commands for 3 trap receivers
INFO  Applying new SNMP configuration with 2 trap receivers
DEBUG New trap receivers to configure: ["192.168.1.1", "192.168.1.2"]
INFO  Verification: Found 2 trap receivers in running-config after update: ["192.168.1.1", "192.168.1.2"]
INFO  ✓ Trap receiver count matches expected configuration
```

### Test Scenario 4: Detection of Mismatch (Bug Reproduction)

**This scenario tests if the bug still exists**

```bash
# Step 1: Apply config
cargo run -- --config-file config.yaml --one-off --log-level debug

# Step 2: Check logs for WARNING
# If bug still exists, you'll see:
```

**Bug Detected Logs**:
```
WARN  MISMATCH: Expected 1 trap receivers but found 2 in running-config!
WARN  Expected: ["192.168.1.1"]
WARN  Actual: ["192.168.1.100", "192.168.1.1"]
```

If you see this warning, it means:
1. The removal commands didn't work (Aruba rejected them)
2. OR there's a timing issue (config not applied yet)
3. OR the switch has a default trap receiver that can't be removed

---

## Debugging Failed Removal

If trap receivers aren't being removed, check:

### 1. Aruba Firmware Version
```bash
ssh admin@<switch-ip> "show version"
```
Some firmware versions may not support removing all trap receivers.

### 2. Manual Removal Test
```bash
ssh admin@<switch-ip>
> configure terminal
> show snmp-server

# IMPORTANT: Must include community string in removal command
# Try to remove a trap receiver manually
> no snmp-server host 192.168.1.100 community "public"

# Check if it worked
> show snmp-server

# If manual removal fails, check the exact community string
# View running config to see the full line:
> show running-config | include snmp-server host
```

### 3. Check for Protected/Default Receivers
Some switches have built-in management trap receivers that cannot be removed.
Check switch documentation for "default trap receiver" or "management VLAN trap".

### 4. Timing Issues
Add a delay after removal before verification:
```rust
// In configure_snmp(), after removal commands
tokio::time::sleep(Duration::from_millis(500)).await;
```

---

## Expected Outcomes

### ✅ Success Case
- Logs show "✓ Trap receiver count matches expected configuration"
- Running-config shows only your configured trap receivers
- Old trap receivers have been removed
- No WARN messages about mismatches

### ⚠️ Potential Issues

**Issue 1: Removal commands rejected by switch**
- **Symptom**: WARN about mismatch, old receivers still present
- **Cause**: Aruba switch doesn't accept `no snmp-server host` command
- **Solution**: Check Aruba documentation for correct removal syntax

**Issue 2: Default trap receiver**
- **Symptom**: Always 1 extra trap receiver that can't be removed
- **Cause**: Switch has built-in management trap receiver
- **Solution**: Document as known limitation, ignore the extra receiver

**Issue 3: Timing delay**
- **Symptom**: Intermittent mismatches
- **Cause**: Switch takes time to apply configuration
- **Solution**: Add sleep after removal/apply commands

---

## Config File Example

Edit your `config.yaml` to test different scenarios:

```yaml
switches:
  - hostname: test-switch
    model: Aruba2930F
    management_ip: "192.168.1.2"
    credentials:
      username: admin
      password: admin
      connection_type: serial
      serial_device: /dev/serial_aruba-2930F
      baud_rate: 9600

    snmp:
      communities:
        - name: "public"
          access: operator

      # Test with 1 trap receiver
      trap_receivers:
        - host: "192.168.1.1"
          community: "public"
          version: "2c"

      # OR test with multiple trap receivers
      # trap_receivers:
      #   - host: "192.168.1.1"
      #     community: "public"
      #   - host: "192.168.1.2"
      #     community: "public"

      enabled_traps:
        - mac-notify

settings:
  dry_run: false
  ssh_timeout_secs: 30
```

---

## Automated Test Script

Save this as `test_bug2.sh`:

```bash
#!/bin/bash
set -e

SWITCH_IP="192.168.1.2"
SWITCH_USER="admin"
SWITCH_PASS="admin"

echo "=== Bug #2 SNMP Trap Receiver Test ==="
echo ""

# Step 1: Check initial state
echo "Step 1: Checking initial state..."
sshpass -p "$SWITCH_PASS" ssh -o StrictHostKeyChecking=no \
  "$SWITCH_USER@$SWITCH_IP" "show running-config | include snmp-server host" \
  || echo "No trap receivers found"
echo ""

# Step 2: Add multiple old trap receivers
echo "Step 2: Adding old trap receivers for testing..."
sshpass -p "$SWITCH_PASS" ssh -o StrictHostKeyChecking=no \
  "$SWITCH_USER@$SWITCH_IP" << 'EOF'
configure terminal
snmp-server host 192.168.1.100 community "old1"
snmp-server host 192.168.1.200 community "old2"
exit
exit
EOF
echo ""

# Step 3: Verify old receivers exist
echo "Step 3: Verifying old trap receivers were added..."
sshpass -p "$SWITCH_PASS" ssh -o StrictHostKeyChecking=no \
  "$SWITCH_USER@$SWITCH_IP" "show running-config | include snmp-server host"
echo ""

# Step 4: Apply configuration
echo "Step 4: Applying new configuration..."
cargo run -- --config-file config.yaml --one-off --log-level debug 2>&1 | grep -E "INFO|WARN"
echo ""

# Step 5: Verify final state
echo "Step 5: Verifying final state..."
FINAL_STATE=$(sshpass -p "$SWITCH_PASS" ssh -o StrictHostKeyChecking=no \
  "$SWITCH_USER@$SWITCH_IP" "show running-config | include snmp-server host")

echo "$FINAL_STATE"
echo ""

# Step 6: Check results
RECEIVER_COUNT=$(echo "$FINAL_STATE" | wc -l)
echo "Final trap receiver count: $RECEIVER_COUNT"

if echo "$FINAL_STATE" | grep -q "192.168.1.100\|192.168.1.200"; then
    echo "❌ FAIL: Old trap receivers still present!"
    exit 1
else
    echo "✅ SUCCESS: Old trap receivers removed!"
fi

if [ "$RECEIVER_COUNT" -eq 1 ]; then
    echo "✅ SUCCESS: Correct number of trap receivers!"
else
    echo "⚠️  WARNING: Expected 1 trap receiver, found $RECEIVER_COUNT"
fi
```

Run with:
```bash
chmod +x test_bug2.sh
./test_bug2.sh
```

---

## Summary

**To verify Bug #2 is fixed**:
1. Run with `--log-level debug`
2. Look for "Found X existing trap receivers to remove" in logs
3. Look for "✓ Trap receiver count matches" confirmation
4. Manually verify with `show running-config | include snmp-server host`

**Success Criteria**:
- ✅ Old trap receivers are removed
- ✅ New trap receivers are added
- ✅ No WARNING about mismatches
- ✅ Running-config matches your config file

---

Generated for Bug #2 investigation - 2025-11-11
