# Manual Testing Plan - TODO.md Task Verification

**Purpose**: Verify all 5 completed TODO.md tasks work correctly in production scenarios
**Target**: Real hardware switches (Aruba recommended for most complete testing)
**Estimated Time**: 4-6 hours for complete test suite
**Prerequisites**: Access to at least one Aruba switch for comprehensive testing

---

## Test Environment Setup

### Required Hardware
- **Minimum**: 1 Aruba switch (2930F, 2540, or 2530 series)
- **Recommended**: 1 Aruba + 1 Cisco for multi-vendor testing
- SSH or serial access to switches
- Test workstation with network connectivity

### Required Software
```bash
# Build the application
nix develop
cargo build --release

# Or use the binary directly
./target/release/switch-configurator
```

### Test Data Location
All test configurations should be created in: `tests/manual/configs/`

---

## Task 1: Aruba Port Mirroring - 4 Tests

### Test 1.1: Four Source Ports Mirror Configuration
**Objective**: Verify all 4 source ports get monitor commands (the original TODO.md bug report)

**Test Config**: `tests/manual/configs/mirror-four-sources.yaml`
```yaml
switches:
  - id: test-mirror-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
    ports: []
    port_mirrors:
      - session_id: 1
        source_ports: ["33", "34", "35", "36"]
        destination_port: "42"
        direction: both
```

**Execution Steps**:
1. Clear any existing mirror configuration on switch
2. Run: `./switch-configurator --config-file tests/manual/configs/mirror-four-sources.yaml --one-off`
3. SSH to switch and run: `show mirror`
4. Run: `show running-config` and check interfaces 33, 34, 35, 36

**Expected Results**:
- ✅ Mirror session 1 shows all 4 source ports
- ✅ Interface 33 has `monitor all both mirror 1` command
- ✅ Interface 34 has `monitor all both mirror 1` command
- ✅ Interface 35 has `monitor all both mirror 1` command
- ✅ Interface 36 has `monitor all both mirror 1` command
- ✅ Interface 42 is configured as destination

**Pass Criteria**: All 4 source ports show monitor command in running-config

---

### Test 1.2: Port Mirroring Idempotency
**Objective**: Verify running twice produces no changes

**Execution Steps**:
1. Run test 1.1 to configure mirroring
2. Run same command again: `./switch-configurator --config-file tests/manual/configs/mirror-four-sources.yaml --one-off`
3. Check output for "No changes needed" or similar

**Expected Results**:
- ✅ Second run detects existing configuration
- ✅ No commands are sent to switch
- ✅ `show mirror` output unchanged

**Pass Criteria**: Second run makes no changes

---

### Test 1.3: Port Mirroring Direction Change
**Objective**: Verify direction changes are applied correctly

**Test Config**: `tests/manual/configs/mirror-direction-change.yaml`
```yaml
# Same as 1.1 but with direction: rx instead of both
port_mirrors:
  - session_id: 1
    source_ports: ["33", "34", "35", "36"]
    destination_port: "42"
    direction: rx  # Changed from both
```

**Execution Steps**:
1. Start with test 1.1 configuration (direction: both)
2. Apply new config with direction: rx
3. Check `show mirror` output

**Expected Results**:
- ✅ Monitor commands updated to `monitor all rx mirror 1`
- ✅ All 4 source ports still monitored
- ✅ Direction changed from both to rx

**Pass Criteria**: Direction successfully changed, all ports still monitored

---

### Test 1.4: Port Mirroring Removal
**Objective**: Verify mirror configuration can be completely removed

**Test Config**: `tests/manual/configs/mirror-removed.yaml`
```yaml
# Same as 1.1 but with empty port_mirrors: []
switches:
  - id: test-mirror-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
    ports: []
    port_mirrors: []  # Empty - should remove mirroring
```

**Execution Steps**:
1. Start with test 1.1 configuration (mirroring active)
2. Apply config with empty port_mirrors
3. Check `show mirror` - should show no sessions
4. Check interface configs - should have no monitor commands

**Expected Results**:
- ✅ Mirror session removed
- ✅ All monitor commands removed from interfaces 33, 34, 35, 36
- ✅ `show mirror` shows no active sessions

**Pass Criteria**: All mirroring completely removed

---

## Task 2: Port Name/Description Cleanup - 5 Tests

### Test 2.1: Port Name Removed When Not in Config
**Objective**: Verify port names are cleared when omitted from config

**Setup**:
1. Manually set port names on switch:
   ```
   interface 5
     name "Old Server Port"
   interface 10
     name "Old Workstation"
   ```

**Test Config**: `tests/manual/configs/port-name-cleanup.yaml`
```yaml
switches:
  - id: test-names-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
      - id: 10
        name: data
    ports:
      - port_id: "5"
        mode: access
        vlan: 10
        enabled: true
        # Note: no description field - should clear existing name
      - port_id: "10"
        mode: access
        vlan: 10
        enabled: true
        # Note: no description field - should clear existing name
```

**Execution Steps**:
1. Verify ports 5 and 10 have names manually configured
2. Apply config (ports defined without description field)
3. Check `show running-config interface 5`
4. Check `show running-config interface 10`

**Expected Results**:
- ✅ Port 5 name "Old Server Port" is removed
- ✅ Port 10 name "Old Workstation" is removed
- ✅ Ports still have correct VLAN and enabled state

**Pass Criteria**: Port names cleared when not specified in config

---

### Test 2.2: Port Name Changed from Old to New
**Objective**: Verify port names are updated when changed

**Test Config**: `tests/manual/configs/port-name-change.yaml`
```yaml
ports:
  - port_id: "5"
    description: "New Server Port"  # Changed from "Old Server Port"
    mode: access
    vlan: 10
    enabled: true
```

**Execution Steps**:
1. Start with port 5 having name "Old Server Port"
2. Apply config with new name "New Server Port"
3. Check running-config

**Expected Results**:
- ✅ Port 5 name updated to "New Server Port"
- ✅ Old name "Old Server Port" no longer present

**Pass Criteria**: Port name successfully updated

---

### Test 2.3: Port Name Kept When Unchanged
**Objective**: Verify idempotent behavior - no changes when name already correct

**Execution Steps**:
1. Apply test 2.2 configuration (sets name to "New Server Port")
2. Apply same configuration again
3. Check logs for unnecessary commands

**Expected Results**:
- ✅ No name change commands sent
- ✅ Configuration detection shows port already correct
- ✅ Port name remains "New Server Port"

**Pass Criteria**: No unnecessary updates when name already matches

---

### Test 2.4: Multiple Ports Mixed Name Operations
**Objective**: Verify complex scenario with add/remove/update

**Setup**:
1. Manually configure:
   ```
   interface 1
     name "Port 1 Old"
   interface 2
     name "Port 2 Old"
   interface 3
     name "Port 3 Old"
   ```

**Test Config**: `tests/manual/configs/port-name-mixed.yaml`
```yaml
ports:
  - port_id: "1"
    description: "Port 1 New"  # UPDATE name
    mode: access
    vlan: 1
  - port_id: "2"
    # NO description field - REMOVE name
    mode: access
    vlan: 1
  - port_id: "3"
    description: "Port 3 Old"  # KEEP name (unchanged)
    mode: access
    vlan: 1
```

**Expected Results**:
- ✅ Port 1: Name updated to "Port 1 New"
- ✅ Port 2: Name removed (no name command in config)
- ✅ Port 3: Name kept as "Port 3 Old" (no change needed)

**Pass Criteria**: Correct mix of add/remove/keep operations

---

### Test 2.5: Reset Ports Uses Correct Command
**Objective**: Verify `reset_ports()` uses "no name" not "no description"

**Test Config**: `tests/manual/configs/reset-ports-test.yaml`
```yaml
switches:
  - id: test-reset-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
    ports:
      - port_id: "1-5"
        mode: access
        vlan: 1
        enabled: true
    settings:
      enforce_port_config: true  # This will reset unconfigured ports
```

**Setup**:
1. Configure names on ports 6-10:
   ```
   interface 6
     name "Test Port 6"
   # ... through port 10
   ```

**Execution Steps**:
1. Apply config with enforce_port_config: true
2. Monitor commands sent (use --debug mode)
3. Check that "no name" is used, not "no description"

**Expected Results**:
- ✅ Ports 6-10 are reset to defaults
- ✅ Command used is "no name" (Aruba syntax)
- ✅ NOT "no description" (incorrect command)

**Pass Criteria**: Correct "no name" command used during reset

---

## Task 3: Multi-Config Optional Fields - 3 Tests

### Test 3.1: Credentials from Main, Ports from Folder
**Objective**: Verify credentials can be omitted in folder config

**Test Configs**:

`tests/manual/configs/multi-config/main.yaml`:
```yaml
merge_priority: 50

switches:
  - id: multi-test-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
```

`tests/manual/configs/multi-config/folder/ports.yaml`:
```yaml
merge_priority: 100

switches:
  - id: multi-test-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # NO credentials field - should inherit from main
    vlans:
      - id: 1
        name: default
    ports:
      - port_id: "1"
        description: "Port from folder config"
        mode: access
        vlan: 1
```

**Execution Steps**:
1. Run with multi-config:
   ```bash
   ./switch-configurator \
     --config-file tests/manual/configs/multi-config/main.yaml \
     --config-folder tests/manual/configs/multi-config/folder \
     --one-off
   ```
2. Verify it connects successfully (uses credentials from main)
3. Check port 1 has description from folder config

**Expected Results**:
- ✅ Connection succeeds using credentials from main.yaml
- ✅ Port 1 configured with description from folder config
- ✅ No error about missing credentials

**Pass Criteria**: Successful merge and configuration

---

### Test 3.2: VLANs from Folder, Credentials from Main
**Objective**: Verify VLANs can be omitted in main config

**Test Configs**:

`tests/manual/configs/multi-config-vlans/main.yaml`:
```yaml
merge_priority: 50

switches:
  - id: multi-test-02
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    # NO vlans field - should inherit from folder
```

`tests/manual/configs/multi-config-vlans/folder/vlans.yaml`:
```yaml
merge_priority: 100

switches:
  - id: multi-test-02
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    vlans:
      - id: 1
        name: default
      - id: 10
        name: data
      - id: 20
        name: voice
```

**Execution Steps**:
1. Run multi-config setup
2. Verify VLANs 1, 10, 20 are created
3. Check that credentials from main were used

**Expected Results**:
- ✅ All 3 VLANs created successfully
- ✅ Connection used credentials from main.yaml
- ✅ No error about missing VLANs in main

**Pass Criteria**: Successful VLAN configuration from folder

---

### Test 3.3: Missing VLANs in All Configs Fails
**Objective**: Verify post-merge validation catches empty VLANs

**Test Configs**:

`tests/manual/configs/missing-vlans/main.yaml`:
```yaml
switches:
  - id: fail-test-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    # NO vlans field
```

`tests/manual/configs/missing-vlans/folder/ports.yaml`:
```yaml
switches:
  - id: fail-test-01
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # NO vlans field - should FAIL after merge
    ports:
      - port_id: "1"
        mode: access
        vlan: 1
```

**Execution Steps**:
1. Run multi-config (should fail):
   ```bash
   ./switch-configurator \
     --config-file tests/manual/configs/missing-vlans/main.yaml \
     --config-folder tests/manual/configs/missing-vlans/folder \
     --one-off
   ```
2. Check error message

**Expected Results**:
- ✅ Configuration fails before connecting to switch
- ✅ Error message mentions "no VLANs defined"
- ✅ Error message identifies switch by hostname and id
- ✅ Error is clear and actionable

**Pass Criteria**: Validation error with helpful message

---

## Task 4: Post-Merge VLAN Validation - 2 Tests

### Test 4.1: Empty VLANs After Merge Fails
**Objective**: Verify validation catches switches with no VLANs

**Test Config**: `tests/manual/configs/empty-vlans.yaml`
```yaml
switches:
  - id: empty-vlan-test
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans: []  # Empty - should fail
    ports:
      - port_id: "1"
        mode: access
        vlan: 1
```

**Execution Steps**:
1. Try to apply config with empty VLANs
2. Check error message

**Expected Results**:
- ✅ Configuration rejected before connecting
- ✅ Error: "has no VLANs defined"
- ✅ Error includes switch hostname "aruba-test"
- ✅ Error includes switch id "empty-vlan-test"
- ✅ Suggests adding at least one VLAN

**Pass Criteria**: Clear validation error with switch identification

---

### Test 4.2: At Least One VLAN Required
**Objective**: Verify minimal valid config has at least VLAN 1

**Test Config**: `tests/manual/configs/minimal-valid.yaml`
```yaml
switches:
  - id: minimal-test
    hostname: aruba-test
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: your_password
    vlans:
      - id: 1
        name: default
    ports: []
```

**Execution Steps**:
1. Apply minimal config with only VLAN 1
2. Verify it succeeds

**Expected Results**:
- ✅ Configuration accepted (has VLAN 1)
- ✅ VLAN 1 configured on switch
- ✅ No validation errors

**Pass Criteria**: Minimal valid config with one VLAN succeeds

---

## Task 5: Enhanced Error Handling - 6 Tests

### Test 5.1: Missing management_ip Error
**Objective**: Verify helpful error for missing required field

**Test Config**: `tests/manual/configs/errors/missing-mgmt-ip.yaml`
```yaml
switches:
  - id: error-test-01
    hostname: test-switch
    # management_ip MISSING
    model: Aruba2930F
    credentials:
      username: admin
      password: test
    vlans:
      - id: 1
        name: default
```

**Execution Steps**:
1. Try to load config
2. Read error message

**Expected Results**:
- ✅ Error message shows: "Field path: switches[0]"
- ✅ Error mentions "missing field `management_ip`"
- ✅ Error includes line number
- ✅ Helpful tip: "Every switch must have a management_ip field"
- ✅ Example shown: `management_ip: "192.168.1.10"`

**Pass Criteria**: Enhanced error with field path and helpful tip

---

### Test 5.2: Missing credentials Error
**Objective**: Verify helpful error for missing credentials

**Test Config**: `tests/manual/configs/errors/missing-creds.yaml`
```yaml
switches:
  - id: error-test-02
    hostname: test-switch
    management_ip: "192.168.1.10"
    model: Aruba2930F
    # credentials MISSING
    vlans:
      - id: 1
        name: default
```

**Execution Steps**:
1. Try to load config
2. Read error message

**Expected Results**:
- ✅ Error shows: "Field path: switches[0]"
- ✅ Error: "missing field `credentials`"
- ✅ Helpful tip with example credentials
- ✅ Shows both password and SSH key options

**Pass Criteria**: Enhanced error with examples

---

### Test 5.3: Type Mismatch - source_ports String vs Array
**Objective**: Verify enhanced error for the TODO.md example case

**Test Config**: `tests/manual/configs/errors/type-mismatch.yaml`
```yaml
switches:
  - id: error-test-03
    hostname: test-switch
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: test
    vlans:
      - id: 1
        name: default
    port_mirrors:
      - session_id: 1
        source_ports: "33,34,35,36"  # WRONG - should be array
        destination_port: "42"
        direction: both
```

**Execution Steps**:
1. Try to load config
2. Read error message

**Expected Results**:
- ✅ Field path: "switches[0].port_mirrors[0].source_ports"
- ✅ Error: "invalid type: string, expected a sequence"
- ✅ Line number shown: "line X column Y"
- ✅ Helpful tip: "source_ports must be an array, not a string"
- ✅ Shows correct format: `["33", "34", "35", "36"]`

**Pass Criteria**: This is THE example from TODO.md - must show enhanced error

---

### Test 5.4: Invalid Enum - Port Mode
**Objective**: Verify helpful error for invalid enum value

**Test Config**: `tests/manual/configs/errors/invalid-mode.yaml`
```yaml
switches:
  - id: error-test-04
    hostname: test-switch
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: test
    vlans:
      - id: 1
        name: default
    ports:
      - port_id: "1"
        mode: wrong_mode  # Invalid - should be 'access' or 'trunk'
        vlan: 1
```

**Execution Steps**:
1. Try to load config
2. Read error message

**Expected Results**:
- ✅ Field path: "switches[0].ports[0].mode"
- ✅ Error: "unknown variant `wrong_mode`"
- ✅ Lists valid options: "access" or "trunk"
- ✅ Line number included
- ✅ Helpful tip about valid port modes

**Pass Criteria**: Clear error with valid options listed

---

### Test 5.5: Multi-Config Parse Error Shows File Path
**Objective**: Verify errors show which file has the problem

**Test Configs**:

`tests/manual/configs/multi-error/main.yaml` (VALID):
```yaml
switches:
  - id: multi-error-test
    hostname: test-switch
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: test
```

`tests/manual/configs/multi-error/folder/bad.yaml` (INVALID):
```yaml
switches:
  - id: multi-error-test
    hostname: test-switch
    management_ip: "192.168.1.10"
    model: Aruba2930F
    vlans:
      - id: 1
        name: default
    ports:
      - port_id: "1"
        mode: bad_mode  # ERROR
        vlan: 1
```

**Execution Steps**:
1. Run multi-config
2. Check error shows problematic file

**Expected Results**:
- ✅ Error identifies file: "multi-error/folder/bad.yaml"
- ✅ Field path shown
- ✅ Line number in that specific file
- ✅ Helpful error message

**Pass Criteria**: Error clearly identifies which config file has the problem

---

### Test 5.6: Line Numbers Accurate
**Objective**: Verify line numbers match actual file content

**Test Config**: `tests/manual/configs/errors/line-number-test.yaml`
```yaml
# Line 1: comment
# Line 2: comment
switches:  # Line 3
  - id: line-test  # Line 4
    hostname: test  # Line 5
    management_ip: "192.168.1.10"  # Line 6
    model: Aruba2930F  # Line 7
    credentials:  # Line 8
      username: admin  # Line 9
      password: test  # Line 10
    vlans:  # Line 11
      - id: 1  # Line 12
        name: default  # Line 13
    ports:  # Line 14
      - port_id: "1"  # Line 15
        mode: invalid_mode  # Line 16 - ERROR HERE
        vlan: 1  # Line 17
```

**Execution Steps**:
1. Try to load config
2. Verify error line number is 16

**Expected Results**:
- ✅ Error shows "line 16" (where invalid_mode is)
- ✅ Line number is accurate
- ✅ Column number also shown

**Pass Criteria**: Line number exactly matches file

---

## Test Execution Workflow

### Before Testing
1. Build the application: `cargo build --release`
2. Create all test config files in `tests/manual/configs/`
3. Have access to at least one Aruba switch
4. Document switch details (IP, credentials, model)

### During Testing
1. Execute tests in order (1.1 through 5.6)
2. Document results for each test
3. Take screenshots of error messages for error tests
4. Capture `show running-config` output for verification
5. Note any unexpected behavior

### After Testing
1. Reset switch to clean state if needed
2. Compile test results
3. Report any failures or issues
4. Update TODO.md if issues found

---

## Test Results Template

Create file: `docs/testing/manual-test-results.md`

```markdown
# Manual Test Results

**Date**: YYYY-MM-DD
**Tester**: [Name]
**Hardware**: [Switch Model and Firmware]
**Build**: [Git commit hash]

## Task 1: Port Mirroring
- [ ] Test 1.1: Four Source Ports - PASS/FAIL - Notes:
- [ ] Test 1.2: Idempotency - PASS/FAIL - Notes:
- [ ] Test 1.3: Direction Change - PASS/FAIL - Notes:
- [ ] Test 1.4: Removal - PASS/FAIL - Notes:

## Task 2: Port Name Cleanup
- [ ] Test 2.1: Name Removed - PASS/FAIL - Notes:
- [ ] Test 2.2: Name Changed - PASS/FAIL - Notes:
- [ ] Test 2.3: Name Unchanged - PASS/FAIL - Notes:
- [ ] Test 2.4: Mixed Operations - PASS/FAIL - Notes:
- [ ] Test 2.5: Reset Command - PASS/FAIL - Notes:

## Task 3: Multi-Config
- [ ] Test 3.1: Credentials from Main - PASS/FAIL - Notes:
- [ ] Test 3.2: VLANs from Folder - PASS/FAIL - Notes:
- [ ] Test 3.3: Missing VLANs Fails - PASS/FAIL - Notes:

## Task 4: VLAN Validation
- [ ] Test 4.1: Empty VLANs Fails - PASS/FAIL - Notes:
- [ ] Test 4.2: Minimal Valid - PASS/FAIL - Notes:

## Task 5: Error Handling
- [ ] Test 5.1: Missing management_ip - PASS/FAIL - Notes:
- [ ] Test 5.2: Missing credentials - PASS/FAIL - Notes:
- [ ] Test 5.3: Type Mismatch (TODO.md case) - PASS/FAIL - Notes:
- [ ] Test 5.4: Invalid Enum - PASS/FAIL - Notes:
- [ ] Test 5.5: Multi-Config Error - PASS/FAIL - Notes:
- [ ] Test 5.6: Line Numbers - PASS/FAIL - Notes:

## Summary
- **Total Tests**: 20
- **Passed**: X
- **Failed**: Y
- **Overall Result**: PASS/FAIL
```

---

## Critical Success Criteria

For the test suite to PASS overall:
1. **All Task 1 tests pass** - Port mirroring works as documented
2. **All Task 2 tests pass** - Port name cleanup works correctly
3. **Test 3.3 and 4.1 FAIL correctly** - Validation catches errors
4. **Test 5.3 MUST pass** - This is the TODO.md example case
5. **All error messages are helpful** - Not just technical errors

---

## Priority Testing Order

### High Priority (Must Test First)
1. Test 5.3 - Type Mismatch (the TODO.md example)
2. Test 1.1 - Four Source Ports (the original bug report)
3. Test 2.1 - Port Name Cleanup
4. Test 4.1 - VLAN Validation

### Medium Priority
5. Tests 1.2-1.4 - Port mirroring variations
6. Tests 2.2-2.5 - Port name variations
7. Tests 3.1-3.3 - Multi-config merging

### Lower Priority (Error Message Verification)
8. Tests 5.1-5.6 - Error message quality

---

## Notes for Test Execution

### Dry-Run Mode
Before applying to real switches, test with `--dry-run`:
```bash
./switch-configurator --config-file CONFIG.yaml --one-off --dry-run
```

This shows what commands WOULD be executed without actually running them.

### Debug Mode
To see each command before execution:
```bash
./switch-configurator --config-file CONFIG.yaml --one-off --debug
```

You'll be prompted Y/n/q for each command.

### Switch Backup
Before testing, backup switch config:
```bash
# SSH to switch
copy running-config flash backup-before-testing
```

### Switch Reset
After testing, restore if needed:
```bash
copy flash backup-before-testing running-config
reload
```

---

## Expected Test Duration

| Task | Tests | Est. Time |
|------|-------|-----------|
| Task 1: Port Mirroring | 4 | 45 min |
| Task 2: Port Name Cleanup | 5 | 60 min |
| Task 3: Multi-Config | 3 | 30 min |
| Task 4: VLAN Validation | 2 | 20 min |
| Task 5: Error Handling | 6 | 45 min |
| **Total** | **20** | **3-4 hours** |

Add 1-2 hours for setup, documentation, and cleanup = **4-6 hours total**

---

## Success Metrics

- **100% pass rate** on functional tests (Tasks 1-4)
- **Error tests MUST fail correctly** (Tasks 3.3, 4.1)
- **All error messages helpful** (Task 5)
- **No unexpected behavior** or side effects
- **Documentation accurate** - behavior matches docs

---

*This manual test plan verifies that all 5 completed TODO.md tasks work correctly in production scenarios with real hardware.*
