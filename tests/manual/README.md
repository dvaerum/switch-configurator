# Manual Test Suite

This directory contains manual test configurations for verifying the TODO.md tasks on real hardware.

## Overview

**Total Tests**: 20 tests across 5 task categories
**Time Estimate**: 4-6 hours for complete execution
**Target Hardware**: Cisco Catalyst 9300-24P UPoE (via serial connection)

## Directory Structure

```
tests/manual/
├── README.md                    # This file
├── run-tests.sh                 # Test execution script
├── test-results.txt             # Test results (generated)
└── configs/                     # Test configuration files
    ├── task1-mirroring/         # Task 1: Port Mirroring (3 configs)
    ├── task2-port-names/        # Task 2: Port Name Cleanup (4 configs)
    ├── task3-multi-config/      # Task 3: Multi-Config (3 folders)
    ├── task4-validation/        # Task 4: VLAN Validation (2 configs)
    ├── task5-errors/            # Task 5: Error Handling (4 configs)
    └── multi-error/             # Task 5: Multi-config error test (1 folder)
```

## Test Categories

### Task 1: Port Mirroring (4 tests)
Verifies Cisco port mirroring (SPAN) with multiple source ports.

- **Test 1.1**: Four source ports - THE TODO.md example
- **Test 1.2**: Four sources idempotency (run twice, no changes)
- **Test 1.3**: Direction change (rx → both)
- **Test 1.4**: Mirror removed (clean deletion)

**Files**: `configs/task1-mirroring/*.yaml`

### Task 2: Port Name/Description Cleanup (5 tests)
Verifies state-aware port name/description handling.

- **Test 2.1**: Name removed (clear existing name)
- **Test 2.2**: Name changed (update existing)
- **Test 2.3**: Name change idempotency (run twice)
- **Test 2.4**: Mixed operations (some with names, some without)
- **Test 2.5**: Reset ports with `enforce_port_config: true`

**Files**: `configs/task2-port-names/*.yaml`

### Task 3: Multi-Config System (3 tests)
Verifies optional fields during parse, required after merge.

- **Test 3.1**: Optional credentials in folder config
- **Test 3.2**: Optional VLANs in folder config
- **Test 3.3**: Missing required VLANs reference (should fail validation)

**Files**: `configs/task3-multi-config/`, `configs/task3-multi-config-vlans/`, `configs/task3-missing-vlans/`

### Task 4: VLAN Validation (2 tests)
Verifies post-merge validation catches missing required fields.

- **Test 4.1**: Empty VLANs after merge (should fail)
- **Test 4.2**: Minimal valid config (should pass)

**Files**: `configs/task4-validation/*.yaml`

### Task 5: Enhanced Error Handling (6 tests)
Verifies helpful error messages with field paths, line numbers, and tips.

- **Test 5.1**: Missing `management_ip` error
- **Test 5.2**: Missing `credentials` error
- **Test 5.3**: Type mismatch - `source_ports` string vs array (THE TODO.md example!)
- **Test 5.4**: Invalid enum - `wrong_mode` vs `access`/`trunk`
- **Test 5.5**: Multi-config error shows file path
- **Test 5.6**: Line number accuracy test

**Files**: `configs/task5-errors/*.yaml`, `configs/multi-error/`

## Quick Start

### 1. Setup

All test configs are pre-configured for:
- **Serial Device**: `/dev/serial_cisco_c9300-24u-a`
- **Switch Model**: Cisco Catalyst 9300-24P UPoE
- **Credentials**: admin/admin (modify if different)

**IMPORTANT PREREQUISITES**:
1. Connect Cisco switch via serial cable to `/dev/serial_cisco_c9300-24u-a`
2. Ensure you have serial port permissions (add user to `dialout` group if needed)
3. Verify serial device exists: `ls -l /dev/serial_cisco_c9300-24u-a`
4. Backup your switch configuration before testing!

### 2. Run All Tests

```bash
# Set your switch hostname (optional, defaults to cisco-c9300-24u-a)
export SWITCH_HOSTNAME=cisco-c9300-24u-a

# Run the test suite
./tests/manual/run-tests.sh
```

The script will:
- Prompt before each test
- Allow skipping tests (press 's')
- Record results to `test-results.txt`
- Show pass/fail summary at the end

### 3. Run Individual Tests

```bash
# Task 1, Test 1: Four source ports (the TODO.md example!)
cargo run -- \
  --config-file tests/manual/configs/task1-mirroring/1.1-four-sources.yaml \
  --one-off \
  --switch cisco-c9300-24u-a

# Task 2, Test 1: Name removed
cargo run -- \
  --config-file tests/manual/configs/task2-port-names/2.1-name-removed.yaml \
  --one-off \
  --switch cisco-c9300-24u-a

# Task 3, Test 1: Multi-config with folder
cargo run -- \
  --config-file tests/manual/configs/task3-multi-config/main.yaml \
  --config-folder tests/manual/configs/task3-multi-config/folder \
  --one-off \
  --switch cisco-c9300-24u-a

# Task 5, Test 3: Type mismatch error (should fail with helpful error)
cargo run -- \
  --config-file tests/manual/configs/task5-errors/5.3-type-mismatch.yaml \
  --one-off \
  --switch cisco-c9300-24u-a
```

### 4. Dry-Run Mode

Preview commands without executing:

```bash
cargo run -- \
  --config-file tests/manual/configs/task1-mirroring/1.1-four-sources.yaml \
  --one-off \
  --dry-run \
  --switch cisco-c9300-24u-a
```

## Test Execution Tips

1. **Serial Connection**: Ensure serial cable is connected and device exists
2. **Start with Dry-Run**: Always test with `--dry-run` first to verify commands
3. **Test Idempotency**: Run same config twice - second run should show "no changes"
4. **Verify State**: Connect to switch console and check running config after each test
5. **Error Tests**: Tests 5.1-5.6 SHOULD FAIL - verify error messages are helpful
6. **Backup First**: Always backup switch config before testing
7. **One Task at a Time**: Focus on one task category before moving to next

## Expected Results

### Success Tests (Tests 1.1-4.2)
- Configuration applies successfully
- Running second time shows "no changes needed" (idempotency)
- Switch running config matches desired config
- No unexpected side effects

### Error Tests (Tests 5.1-5.6)
- Configuration fails to parse
- Error message shows:
  - Field path (e.g., `switches[0].port_mirrors[0].source_ports`)
  - Line number (e.g., `at line 20`)
  - Helpful tip (e.g., "Use: source_ports: [\"33\", \"34\"]")
  - File path for multi-config errors

## Test Results Format

Results are saved to `test-results.txt`:

```
Manual Test Execution Results - 2025-11-25 14:30:00
Switch: cisco-c9300-24u-a
========================================

Test 1.1: PASS
Test 1.2: PASS
Test 1.3: PASS
Test 1.4: PASS
Test 2.1: PASS
...
Test 5.6: PASS

Summary:
Passed: 20
Failed: 0
Skipped: 0
```

## Troubleshooting

### Test Fails to Apply
1. Check serial connection: `ls -l /dev/serial_cisco_c9300-24u-a`
2. Verify serial permissions: `groups` (should include dialout)
3. Check switch console is responsive (try connecting with screen/minicom)
4. Enable debug logging: `--log-level debug`

### Unexpected Commands Generated
1. Run with `--dry-run` to review commands
2. Check state parsing is working correctly
3. Verify current switch state matches assumptions

### Error Tests Not Showing Helpful Errors
1. Verify you're running latest code (with error handling enhancements)
2. Check error message includes field path and line number
3. Report missing tips/suggestions as issues

## Related Documentation

- **Full Test Plan**: `docs/testing/MANUAL-TESTING-PLAN.md` - Detailed test descriptions
- **TODO Tasks**: `TODO.md` - Original task requirements
- **Error Handling**: `docs/sessions/2025-11-25-error-handling-plan.md` - Implementation details
- **Multi-Config**: `AGENTS.md` (Multi-Config Merge System section) - Multi-config documentation

## Contributing Test Results

After running tests, please document results in session notes:

1. Note any failed tests and why they failed
2. Document any unexpected behavior
3. Suggest improvements to test configs or execution
4. Report bugs found during testing

## Safety Notes

⚠️ **IMPORTANT**: These tests modify switch configurations!

- Always backup switch config first
- Test on non-production switches when possible
- Verify credentials are correct before bulk execution
- Monitor switch during test execution
- Have console access available for recovery
- Document baseline state before testing

---

**Last Updated**: 2025-11-25
**Test Count**: 20 tests
**Status**: Ready for execution
