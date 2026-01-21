# Manual Testing Guide for Management VLAN Feature

## ✅ Unit Tests Status
- **Aruba**: 7/7 tests passing (parsing + diff computation)
- **Cisco**: 4/4 tests passing (diff computation)
- **FortiSwitch**: 4/4 tests passing (diff computation)
- **Total**: 15/15 tests passing

## Prerequisites

1. **Serial Access**: Ensure you have access to the serial devices:
```bash
ls -l /dev/serial_*
# Should show:
# /dev/serial_aruba-2530-48g-2sfp+
# /dev/serial_aruba-2530-8g-poe+
# /dev/serial_cisco_c9300-24u-a
# /dev/serial_fortiswitch_108f-poe
```

2. **Permissions**: Ensure you're in the dialout group:
```bash
groups | grep dialout
```

3. **Backup**: Document current configurations before testing

## Test Plan

For each vendor, we'll test:
1. ✅ Connect and read current state (NO management_vlan configured)
2. ✅ Add management_vlan configuration
3. ✅ Verify it was applied correctly
4. ✅ Make NO changes (idempotent test)
5. ✅ Remove management_vlan configuration
6. ✅ Verify it was removed

---

## Test 1: Aruba 2530-8G PoE+ (Simplest to start with)

### Device Info
- Serial: `/dev/serial_aruba-2530-8g-poe+`
- Baud: 115200
- User: admin
- Pass: admin

### Step 1A: Backup Current Configuration

Create a backup config file:
```yaml
# aruba-8g-backup.yaml
switches:
  - id: aruba-8g-backup
    hostname: aruba-2530-8g-test
    management_ip: 192.168.1.101
    model: Aruba2530_8G_POE
    credentials:
      connection_type: serial
      serial_device: /dev/serial_aruba-2530-8g-poe+
      baud_rate: 115200
      username: admin
      password: admin
    vlans: []
    ports: []
```

Get running config:
```bash
cargo run -- --config-file aruba-8g-backup.yaml --one-off --log-level debug 2>&1 | tee aruba-8g-before.log
```

### Step 1B: Read Current State (Dry-Run)

```bash
# This should show NO management-vlan in the current state
cargo run -- --config-file test-management-vlan.yaml --one-off --dry-run --switch aruba-2530-8g-test --log-level debug 2>&1 | grep -A 5 -B 5 "management"
```

**Expected Output**:
- Should parse current state
- `management_vlan: None` in current state
- Should detect `management_vlan_changed: true` in diff
- Should show command: `management-vlan 10`

### Step 1C: Apply Management VLAN 10

```bash
# Apply configuration (ACTUALLY EXECUTES COMMANDS)
cargo run -- --config-file test-management-vlan.yaml --one-off --switch aruba-2530-8g-test --log-level info 2>&1 | tee aruba-8g-add-mgmt-vlan.log
```

**Expected Results**:
- Connects via serial
- Parses current state (no management-vlan)
- Computes diff (management_vlan_changed: true)
- Executes commands:
  ```
  configure terminal
  management-vlan 10
  exit
  ```
- Shows success message

### Step 1D: Verify Application

Manually verify via serial console or run dry-run again:
```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --dry-run --switch aruba-2530-8g-test --log-level debug 2>&1 | grep -A 3 "management"
```

**Expected Output**:
- Should parse `management-vlan 10` from running config
- `management_vlan: Some(10)` in current state
- `management_vlan_changed: false` (no change needed)
- Should show "No changes needed" or "Configuration up to date"

### Step 1E: Idempotent Test (No Changes)

Run the same command again:
```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --switch aruba-2530-8g-test --log-level info 2>&1 | tee aruba-8g-idempotent.log
```

**Expected Results**:
- Connects and parses state: `management_vlan: Some(10)`
- Computes diff: `management_vlan_changed: false`
- **CRITICAL**: Should NOT execute any management-vlan commands
- Should show "No changes needed" or similar

### Step 1F: Remove Management VLAN

Create a config with no management_vlan:
```yaml
# aruba-8g-remove-mgmt.yaml
switches:
  - id: aruba-8g-remove
    hostname: aruba-2530-8g-test
    management_ip: 192.168.1.101
    model: Aruba2530_8G_POE
    # NO management_vlan field (defaults to None)
    credentials:
      connection_type: serial
      serial_device: /dev/serial_aruba-2530-8g-poe+
      baud_rate: 115200
      username: admin
      password: admin
    vlans:
      - id: 1
        name: default
      - id: 10
        name: mgmt
        ip_config: dhcp
    ports: []
```

Apply:
```bash
cargo run -- --config-file aruba-8g-remove-mgmt.yaml --one-off --switch aruba-8g-remove --log-level info 2>&1 | tee aruba-8g-remove-mgmt-vlan.log
```

**Expected Results**:
- Parses current state: `management_vlan: Some(10)`
- Desired config: `management_vlan: None`
- Computes diff: `management_vlan_changed: true`, `management_vlan: None`
- Executes commands:
  ```
  configure terminal
  no management-vlan
  exit
  ```
- Shows success

### Step 1G: Verify Removal

```bash
cargo run -- --config-file aruba-8g-remove-mgmt.yaml --one-off --dry-run --switch aruba-8g-remove --log-level debug 2>&1 | grep -A 3 "management"
```

**Expected**:
- Should parse NO management-vlan from running config
- `management_vlan: None` in current state
- `management_vlan_changed: false`

---

## Test 2: Cisco Catalyst 9300 (SVI Configuration)

### Device Info
- Serial: `/dev/serial_cisco_c9300-24u-a`
- Baud: 9600
- User: admin
- Pass: admin

### Step 2A: Dry-Run Test

```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --dry-run --switch cisco-c9300-test --log-level debug 2>&1 | grep -A 10 "management"
```

**Expected Commands**:
```
configure terminal
interface vlan 88
ip address 192.168.88.1 255.255.255.0
no shutdown
exit
end
```

### Step 2B: Apply Configuration

```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --switch cisco-c9300-test --log-level info 2>&1 | tee cisco-add-mgmt-vlan.log
```

### Step 2C: Verify via Show Commands

Manually connect to switch:
```bash
screen /dev/serial_cisco_c9300-24u-a 9600
```

Run:
```
show ip interface vlan 88
show running-config interface vlan 88
```

**Expected**:
- Interface Vlan88 exists
- IP address: 192.168.88.1 255.255.255.0
- Status: up

### Step 2D: Idempotent Test

```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --switch cisco-c9300-test --log-level info 2>&1 | tee cisco-idempotent.log
```

**Expected**: No changes (note: current state parsing not fully implemented for Cisco, may show empty state)

---

## Test 3: FortiSwitch (VLAN Interface + Allowaccess)

### Device Info
- Serial: `/dev/serial_fortiswitch_108f-poe`
- Baud: 115200
- User: admin
- Pass: adminadmin

### Step 3A: Dry-Run Test

```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --dry-run --switch fortiswitch-108f-test --log-level debug 2>&1 | grep -A 10 "management"
```

**Expected Commands**:
```
config system interface
edit vlan77
set ip 192.168.77.1 255.255.255.0
set allowaccess ping https ssh snmp
next
end
```

### Step 3B: Apply Configuration

```bash
cargo run -- --config-file test-management-vlan.yaml --one-off --switch fortiswitch-108f-test --log-level info 2>&1 | tee fortiswitch-add-mgmt-vlan.log
```

### Step 3C: Verify via Show Commands

Manually connect:
```bash
screen /dev/serial_fortiswitch_108f-poe 115200
```

Run:
```
show system interface vlan77
get system interface
```

**Expected**:
- Interface vlan77 exists
- IP: 192.168.77.1/24
- Allowaccess shows: ping, https, ssh, snmp

---

## Test 4: Aruba 2530-48G (Full Test)

Repeat all steps from Test 1 with the 48G model:
- Use `--switch aruba-2530-48g-test`
- Serial: `/dev/serial_aruba-2530-48g-2sfp+`
- Management VLAN: 99

---

## Success Criteria

For each vendor, ALL of the following must pass:

### ✅ Parsing
- [ ] Correctly parses `management-vlan X` from running config (Aruba)
- [ ] Returns `Some(X)` when configured
- [ ] Returns `None` when not configured

### ✅ Diff Computation
- [ ] Detects when adding management VLAN (None → Some(X))
- [ ] Detects when changing management VLAN (Some(A) → Some(B))
- [ ] Detects when removing management VLAN (Some(X) → None)
- [ ] No false positives (Some(X) → Some(X) should be no change)

### ✅ Command Generation
- [ ] Generates correct vendor-specific commands
- [ ] Commands execute without errors
- [ ] Configuration persists after execution

### ✅ Idempotency
- [ ] Running same config twice does NOT execute commands second time
- [ ] No errors when configuration already matches desired state

### ✅ Removal
- [ ] Successfully removes management VLAN when config specifies None
- [ ] Removal verified in running config

---

## Troubleshooting

### Issue: Serial device busy
```bash
# Check what's using the device
lsof | grep /dev/serial_

# Kill any screen sessions
screen -ls
screen -X -S <session> quit
```

### Issue: Permission denied on serial device
```bash
# Add yourself to dialout group
sudo usermod -a -G dialout $USER
# Log out and back in
```

### Issue: Timeout waiting for prompt
- Check baud rate matches switch (Aruba: 115200, Cisco: 9600)
- Press Enter a few times to get a fresh prompt
- Try manually with `screen` to verify connectivity

### Issue: Commands fail to execute
- Check credentials are correct
- Ensure you have privilege level 15 (enable mode)
- Review error messages in log output with `--log-level debug`

---

## Recording Results

For each test, record:

1. **Test Name**: (e.g., "Aruba 8G - Add Management VLAN")
2. **Command Used**: (full cargo run command)
3. **Expected Behavior**: (what should happen)
4. **Actual Behavior**: (what actually happened)
5. **Status**: ✅ Pass / ❌ Fail
6. **Log File**: (path to saved log output)
7. **Notes**: (any observations, errors, warnings)

Create a test results file:
```bash
cat > TEST_RESULTS.md << 'EOF'
# Management VLAN Manual Test Results

## Date: $(date)
## Tester:

## Aruba 2530-8G PoE+
- [ ] Test 1A: Backup current config
- [ ] Test 1B: Read current state (dry-run)
- [ ] Test 1C: Apply management VLAN 10
- [ ] Test 1D: Verify application
- [ ] Test 1E: Idempotent test
- [ ] Test 1F: Remove management VLAN
- [ ] Test 1G: Verify removal

## Cisco Catalyst 9300
- [ ] Test 2A: Dry-run test
- [ ] Test 2B: Apply configuration
- [ ] Test 2C: Verify via show commands
- [ ] Test 2D: Idempotent test

## FortiSwitch 108F
- [ ] Test 3A: Dry-run test
- [ ] Test 3B: Apply configuration
- [ ] Test 3C: Verify via show commands

## Aruba 2530-48G
- [ ] Test 4: Full test cycle

## Summary
Total Tests: X
Passed: X
Failed: X

## Issues Found
(List any bugs, unexpected behavior, or improvements needed)

## Recommendations
(List any changes needed before production use)
EOF
```

---

## Next Steps After Manual Testing

1. If all tests pass → Feature is production-ready
2. If tests fail → Debug, fix, update unit tests, re-test
3. Update documentation with any findings
4. Consider adding integration tests based on manual test results
5. Update CLAUDE.md with management_vlan usage examples

---

**Ready to start manual testing?** Begin with Aruba 2530-8G (Test 1) since it's the simplest and smallest switch.
