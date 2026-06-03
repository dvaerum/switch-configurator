#!/usr/bin/env bash
# Manual Test Execution Script
# This script helps execute the 20 manual tests systematically
# See docs/testing/MANUAL-TESTING-PLAN.md for full test descriptions

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SWITCH_HOSTNAME="${SWITCH_HOSTNAME:-cisco-c9300-24u-a}"
RESULTS_FILE="tests/manual/test-results.txt"
CONFIG_DIR="tests/manual/configs"

# Initialize results file
echo "Manual Test Execution Results - $(date)" > "$RESULTS_FILE"
echo "Switch: $SWITCH_HOSTNAME" >> "$RESULTS_FILE"
echo "========================================" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"

# Helper function to run a test
run_test() {
    local test_num=$1
    local test_name=$2
    local config_file=$3
    local mode=$4  # "apply" or "dry-run" or "error"

    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${YELLOW}Test $test_num: $test_name${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""

    if [ "$mode" = "error" ]; then
        echo -e "${YELLOW}This test expects an ERROR. Verify error message is helpful.${NC}"
        echo ""
        echo "Command: cargo run -- --config-file $config_file --one-off --switch $SWITCH_HOSTNAME 2>&1"
        echo ""
        read -p "Press ENTER to run test (or 's' to skip): " choice
        if [ "$choice" = "s" ]; then
            echo "SKIPPED" >> "$RESULTS_FILE"
            echo -e "${YELLOW}Skipped${NC}\n"
            return
        fi

        if cargo run -- --config-file "$config_file" --one-off --switch "$SWITCH_HOSTNAME" 2>&1; then
            echo -e "${RED}FAIL: Expected error but succeeded${NC}"
            echo "Test $test_num: FAIL (no error)" >> "$RESULTS_FILE"
        else
            echo -e "${GREEN}Error occurred as expected${NC}"
            read -p "Was the error message helpful? (y/n): " helpful
            if [ "$helpful" = "y" ]; then
                echo "Test $test_num: PASS" >> "$RESULTS_FILE"
                echo -e "${GREEN}PASS${NC}"
            else
                echo "Test $test_num: FAIL (unhelpful error)" >> "$RESULTS_FILE"
                echo -e "${RED}FAIL${NC}"
            fi
        fi
    elif [ "$mode" = "dry-run" ]; then
        echo -e "${YELLOW}DRY-RUN mode: Review generated commands${NC}"
        echo ""
        echo "Command: cargo run -- --config-file $config_file --one-off --dry-run --switch $SWITCH_HOSTNAME"
        echo ""
        read -p "Press ENTER to run test (or 's' to skip): " choice
        if [ "$choice" = "s" ]; then
            echo "SKIPPED" >> "$RESULTS_FILE"
            echo -e "${YELLOW}Skipped${NC}\n"
            return
        fi

        if cargo run -- --config-file "$config_file" --one-off --dry-run --switch "$SWITCH_HOSTNAME"; then
            read -p "Do the commands look correct? (y/n): " correct
            if [ "$correct" = "y" ]; then
                echo "Test $test_num: PASS" >> "$RESULTS_FILE"
                echo -e "${GREEN}PASS${NC}"
            else
                echo "Test $test_num: FAIL" >> "$RESULTS_FILE"
                echo -e "${RED}FAIL${NC}"
            fi
        else
            echo "Test $test_num: ERROR" >> "$RESULTS_FILE"
            echo -e "${RED}ERROR${NC}"
        fi
    else
        # Apply mode
        echo -e "${YELLOW}APPLY mode: Will modify switch configuration${NC}"
        echo ""
        echo "Command: cargo run -- --config-file $config_file --one-off --switch $SWITCH_HOSTNAME"
        echo ""
        read -p "Press ENTER to run test (or 's' to skip): " choice
        if [ "$choice" = "s" ]; then
            echo "SKIPPED" >> "$RESULTS_FILE"
            echo -e "${YELLOW}Skipped${NC}\n"
            return
        fi

        if cargo run -- --config-file "$config_file" --one-off --switch "$SWITCH_HOSTNAME"; then
            read -p "Did the test pass? (y/n): " result
            if [ "$result" = "y" ]; then
                echo "Test $test_num: PASS" >> "$RESULTS_FILE"
                echo -e "${GREEN}PASS${NC}"
            else
                echo "Test $test_num: FAIL" >> "$RESULTS_FILE"
                echo -e "${RED}FAIL${NC}"
            fi
        else
            echo "Test $test_num: ERROR" >> "$RESULTS_FILE"
            echo -e "${RED}ERROR${NC}"
        fi
    fi

    echo ""
}

# Main test execution
echo -e "${GREEN}╔══════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  Manual Test Suite for TODO.md Tasks    ║${NC}"
echo -e "${GREEN}╚══════════════════════════════════════════╝${NC}"
echo ""
echo "Target Switch: $SWITCH_HOSTNAME"
echo "Results File: $RESULTS_FILE"
echo ""
echo -e "${YELLOW}IMPORTANT SETUP:${NC}"
echo "1. Ensure Cisco switch is connected via serial: /dev/serial_cisco_c9300-24u-a"
echo "2. Verify serial device permissions (may need to be in dialout group)"
echo "3. Backup switch configuration before testing"
echo ""
read -p "Press ENTER to begin tests..."
echo ""

# Task 1: Port Mirroring Tests
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN} TASK 1: PORT MIRRORING TESTS${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""

run_test "1.1" "Four Source Ports" \
    "$CONFIG_DIR/task1-mirroring/1.1-four-sources.yaml" "apply"

run_test "1.2" "Four Sources Idempotency" \
    "$CONFIG_DIR/task1-mirroring/1.1-four-sources.yaml" "apply"

run_test "1.3" "Direction Change" \
    "$CONFIG_DIR/task1-mirroring/1.3-direction-change.yaml" "apply"

run_test "1.4" "Mirror Removed" \
    "$CONFIG_DIR/task1-mirroring/1.4-mirror-removed.yaml" "apply"

# Task 2: Port Name/Description Tests
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN} TASK 2: PORT NAME/DESCRIPTION TESTS${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""

run_test "2.1" "Name Removed" \
    "$CONFIG_DIR/task2-port-names/2.1-name-removed.yaml" "apply"

run_test "2.2" "Name Changed" \
    "$CONFIG_DIR/task2-port-names/2.2-name-changed.yaml" "apply"

run_test "2.3" "Name Change Idempotency" \
    "$CONFIG_DIR/task2-port-names/2.2-name-changed.yaml" "apply"

run_test "2.4" "Mixed Operations" \
    "$CONFIG_DIR/task2-port-names/2.4-mixed-operations.yaml" "apply"

run_test "2.5" "Reset Ports Enforcement" \
    "$CONFIG_DIR/task2-port-names/2.5-reset-ports.yaml" "apply"

# Task 3: Multi-Config Tests
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN} TASK 3: MULTI-CONFIG TESTS${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""

run_test "3.1" "Optional Credentials in Folder" \
    "$CONFIG_DIR/task3-multi-config/main.yaml" "apply"

run_test "3.2" "Optional VLANs in Folder" \
    "$CONFIG_DIR/task3-multi-config-vlans/main.yaml" "apply"

run_test "3.3" "Missing Required VLANs Reference" \
    "$CONFIG_DIR/task3-missing-vlans/main.yaml" "error"

# Task 4: VLAN Validation Tests
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN} TASK 4: VLAN VALIDATION TESTS${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""

run_test "4.1" "Empty VLANs After Merge" \
    "$CONFIG_DIR/task4-validation/4.1-empty-vlans.yaml" "error"

run_test "4.2" "Minimal Valid Config" \
    "$CONFIG_DIR/task4-validation/4.2-minimal-valid.yaml" "apply"

# Task 5: Error Handling Tests
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN} TASK 5: ERROR HANDLING TESTS${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""

run_test "5.1" "Missing management_ip" \
    "$CONFIG_DIR/task5-errors/5.1-missing-mgmt-ip.yaml" "error"

run_test "5.2" "Missing credentials" \
    "$CONFIG_DIR/task5-errors/5.2-missing-creds.yaml" "error"

run_test "5.3" "Type Mismatch (source_ports)" \
    "$CONFIG_DIR/task5-errors/5.3-type-mismatch.yaml" "error"

run_test "5.4" "Invalid Enum (port mode)" \
    "$CONFIG_DIR/task5-errors/5.4-invalid-mode.yaml" "error"

run_test "5.5" "Multi-Config Error File Path" \
    "$CONFIG_DIR/multi-error/main.yaml" "error"

run_test "5.6" "Line Number Accuracy" \
    "$CONFIG_DIR/task5-errors/5.6-line-number-test.yaml" "error"

# Summary
echo ""
echo -e "${GREEN}╔══════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║         TEST EXECUTION COMPLETE          ║${NC}"
echo -e "${GREEN}╚══════════════════════════════════════════╝${NC}"
echo ""
echo "Results saved to: $RESULTS_FILE"
echo ""
echo "Summary:"
grep "PASS" "$RESULTS_FILE" | wc -l | xargs -I {} echo -e "${GREEN}Passed: {}${NC}"
grep "FAIL" "$RESULTS_FILE" | wc -l | xargs -I {} echo -e "${RED}Failed: {}${NC}"
grep "SKIPPED" "$RESULTS_FILE" | wc -l | xargs -I {} echo -e "${YELLOW}Skipped: {}${NC}"
echo ""
