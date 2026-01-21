#!/bin/bash
# Manual API Test Script for desired-config endpoints
# Tests PUT, PATCH, GET, DELETE /switches/{id}/desired-config
#
# Usage:
#   ./tests/scripts/test-api-desired-config.sh [port]
#
# Prerequisites:
#   - Server must be running: cargo run -- --config-file config.yaml
#   - curl and jq must be installed

set -e

PORT=${1:-4002}
BASE_URL="http://localhost:${PORT}"
PASSED=0
FAILED=0
TESTS_RUN=0

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_test() {
    echo -e "\n${YELLOW}=== TEST: $1 ===${NC}"
    ((TESTS_RUN++))
}

log_pass() {
    echo -e "${GREEN}PASS${NC}: $1"
    ((PASSED++))
}

log_fail() {
    echo -e "${RED}FAIL${NC}: $1"
    ((FAILED++))
}

check_status() {
    local expected=$1
    local actual=$2
    local description=$3

    if [ "$actual" -eq "$expected" ]; then
        log_pass "$description (HTTP $actual)"
        return 0
    else
        log_fail "$description (expected HTTP $expected, got HTTP $actual)"
        return 1
    fi
}

# Check if server is running
echo "Checking if server is running on port $PORT..."
if ! curl -s "${BASE_URL}/health" > /dev/null 2>&1; then
    echo -e "${RED}ERROR: Server not running on port $PORT${NC}"
    echo "Start the server first with: cargo run -- --config-file config.yaml"
    exit 1
fi
echo -e "${GREEN}Server is running${NC}\n"

# ============================================================
# Test 1: GET /switches - List existing switches
# ============================================================
log_test "GET /switches - List existing switches"
RESPONSE=$(curl -s -w "\n%{http_code}" "${BASE_URL}/switches")
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 200 "$HTTP_CODE" "List switches"
echo "Response: $BODY" | head -c 200
echo "..."

# ============================================================
# Test 2: PUT - Create new switch (valid)
# ============================================================
log_test "PUT /switches/{id}/desired-config - Create new switch"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "api-test-switch-1",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.1",
        "credentials": {
            "username": "admin",
            "password": "testpass123"
        },
        "vlans": [
            {"id": 10, "name": "management"},
            {"id": 20, "name": "users"}
        ],
        "ports": [
            {"port_id": "1", "mode": "access", "vlan": 10, "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 201 "$HTTP_CODE" "Create new switch"
echo "Response: $BODY"

# ============================================================
# Test 3: GET - Verify switch was created
# ============================================================
log_test "GET /switches/{id}/desired-config - Verify switch created"
RESPONSE=$(curl -s -w "\n%{http_code}" "${BASE_URL}/switches/api-test-sw-01/desired-config")
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 200 "$HTTP_CODE" "Get created switch"
# Verify hostname
if echo "$BODY" | grep -q "api-test-switch-1"; then
    log_pass "Hostname matches"
else
    log_fail "Hostname mismatch"
fi

# ============================================================
# Test 4: PUT - Create with port range expansion
# ============================================================
log_test "PUT - Port range expansion (1-3 should become 3 ports)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-sw-02/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "api-test-switch-2",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.2",
        "credentials": {"username": "admin", "password": "testpass123"},
        "vlans": [{"id": 100, "name": "test-vlan"}],
        "ports": [
            {"port_id": "1-3", "mode": "access", "vlan": 100, "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 201 "$HTTP_CODE" "Create switch with port range"

# Verify port expansion
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-sw-02/desired-config")
PORT_COUNT=$(echo "$RESPONSE" | grep -o '"port_id"' | wc -l)
if [ "$PORT_COUNT" -eq 3 ]; then
    log_pass "Port range expanded to 3 ports"
else
    log_fail "Port range expansion failed (expected 3 ports, got $PORT_COUNT)"
fi

# ============================================================
# Test 5: PUT - Invalid VLAN reference (should fail)
# ============================================================
log_test "PUT - Invalid VLAN reference (should return 400)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-invalid/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "invalid-switch",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.99",
        "credentials": {"username": "admin", "password": "testpass123"},
        "vlans": [{"id": 100, "name": "existing-vlan"}],
        "ports": [
            {"port_id": "1", "mode": "access", "vlan": 999, "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 400 "$HTTP_CODE" "Reject invalid VLAN reference"
echo "Error response: $BODY"

# ============================================================
# Test 6: PUT - Invalid speed/duplex for model (should fail)
# ============================================================
log_test "PUT - Invalid speed/duplex (10G on non-10G switch, should return 400)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-invalid-speed/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "invalid-speed-switch",
        "model": "Aruba2530_24G_POE",
        "management_ip": "192.168.99.98",
        "credentials": {"username": "admin", "password": "testpass123"},
        "vlans": [{"id": 100, "name": "test-vlan"}],
        "ports": [
            {"port_id": "1", "mode": "access", "vlan": 100, "enabled": true, "speed_duplex": "10g-full"}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 400 "$HTTP_CODE" "Reject invalid speed/duplex"
echo "Error response: $BODY"

# ============================================================
# Test 7: PUT - Missing required fields (should fail)
# ============================================================
log_test "PUT - Missing required fields (should return 400)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-incomplete/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "vlans": [{"id": 100, "name": "test-vlan"}]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 400 "$HTTP_CODE" "Reject missing required fields"
if echo "$BODY" | grep -q "hostname"; then
    log_pass "Error mentions missing hostname"
else
    log_fail "Error should mention missing hostname"
fi

# ============================================================
# Test 8: PUT - ID mismatch (should fail)
# ============================================================
log_test "PUT - ID mismatch between URL and body (should return 400)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "id": "different-id",
        "hostname": "test",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.1",
        "credentials": {"username": "admin", "password": "test"}
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

check_status 400 "$HTTP_CODE" "Reject ID mismatch"
if echo "$BODY" | grep -q "mismatch"; then
    log_pass "Error mentions ID mismatch"
else
    log_fail "Error should mention ID mismatch"
fi

# ============================================================
# Test 9: PATCH - Update hostname
# ============================================================
log_test "PATCH - Update hostname"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "api-test-switch-1-updated"
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 200 "$HTTP_CODE" "Update hostname via PATCH"

# Verify hostname changed
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-sw-01/desired-config")
if echo "$RESPONSE" | grep -q "api-test-switch-1-updated"; then
    log_pass "Hostname was updated"
else
    log_fail "Hostname was not updated"
fi

# ============================================================
# Test 10: PATCH - Add new VLAN (merge behavior)
# ============================================================
log_test "PATCH - Add new VLAN (should merge with existing)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "vlans": [{"id": 30, "name": "new-vlan-30"}]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 200 "$HTTP_CODE" "Add VLAN via PATCH"

# Verify VLANs - should have original 10, 20 plus new 30
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-sw-01/desired-config")
VLAN_COUNT=$(echo "$RESPONSE" | grep -o '"id":' | wc -l)
# Note: id appears multiple times (switch id, vlan ids, port ids)
if echo "$RESPONSE" | grep -q '"name":"new-vlan-30"'; then
    log_pass "New VLAN 30 was added"
else
    log_fail "New VLAN 30 was not added"
fi

# ============================================================
# Test 11: PATCH - Add port with range expansion
# ============================================================
log_test "PATCH - Add ports with range expansion"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "ports": [
            {"port_id": "5-7", "mode": "access", "vlan": 10, "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 200 "$HTTP_CODE" "Add ports with range via PATCH"

# Verify ports were expanded
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-sw-01/desired-config")
if echo "$RESPONSE" | grep -q '"port_id":"5"' && echo "$RESPONSE" | grep -q '"port_id":"6"' && echo "$RESPONSE" | grep -q '"port_id":"7"'; then
    log_pass "Port range 5-7 was expanded"
else
    log_fail "Port range expansion failed in PATCH"
fi

# ============================================================
# Test 12: PATCH - Invalid VLAN reference (should fail and rollback)
# ============================================================
log_test "PATCH - Invalid VLAN reference (should return 400 and rollback)"

# First, get current hostname to verify rollback
ORIGINAL=$(curl -s "${BASE_URL}/switches/api-test-sw-01/desired-config" | grep -o '"hostname":"[^"]*"' | head -1)

RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "${BASE_URL}/switches/api-test-sw-01/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "should-not-persist",
        "ports": [
            {"port_id": "99", "mode": "access", "vlan": 9999, "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 400 "$HTTP_CODE" "Reject PATCH with invalid VLAN"

# Verify rollback - hostname should NOT have changed
CURRENT=$(curl -s "${BASE_URL}/switches/api-test-sw-01/desired-config" | grep -o '"hostname":"[^"]*"' | head -1)
if [ "$ORIGINAL" = "$CURRENT" ]; then
    log_pass "Rollback worked - hostname unchanged"
else
    log_fail "Rollback failed - hostname changed from $ORIGINAL to $CURRENT"
fi

# ============================================================
# Test 13: PATCH - Non-existent switch (should fail)
# ============================================================
log_test "PATCH - Non-existent switch (should return 404)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "${BASE_URL}/switches/non-existent-switch/desired-config" \
    -H "Content-Type: application/json" \
    -d '{"hostname": "test"}')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 404 "$HTTP_CODE" "Reject PATCH on non-existent switch"

# ============================================================
# Test 14: DELETE - Remove switch
# ============================================================
log_test "DELETE /switches/{id}/desired-config - Remove switch"
RESPONSE=$(curl -s -w "\n%{http_code}" -X DELETE "${BASE_URL}/switches/api-test-sw-02/desired-config")
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 200 "$HTTP_CODE" "Delete switch"

# Verify switch is gone
RESPONSE=$(curl -s -w "\n%{http_code}" "${BASE_URL}/switches/api-test-sw-02/desired-config")
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 404 "$HTTP_CODE" "Verify switch was deleted"

# ============================================================
# Test 15: DELETE - Non-existent switch (should return 404)
# ============================================================
log_test "DELETE - Non-existent switch (should return 404)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X DELETE "${BASE_URL}/switches/non-existent-switch/desired-config")
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 404 "$HTTP_CODE" "Reject DELETE on non-existent switch"

# ============================================================
# Test 16: PUT - Trunk VLAN filtering
# ============================================================
log_test "PUT - Trunk port with invalid allowed_vlans (should filter, not reject)"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-trunk/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "trunk-test-switch",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.50",
        "credentials": {"username": "admin", "password": "testpass123"},
        "vlans": [{"id": 100, "name": "valid-vlan"}],
        "ports": [
            {"port_id": "1", "mode": "trunk", "vlan": 100, "allowed_vlans": [100, 200, 300], "enabled": true}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 201 "$HTTP_CODE" "Create switch with trunk (invalid VLANs filtered)"

# Verify only valid VLAN remains in allowed_vlans
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-trunk/desired-config")
if echo "$RESPONSE" | grep -q '"allowed_vlans":\[100\]'; then
    log_pass "Invalid VLANs (200, 300) were filtered out"
else
    log_fail "Invalid VLAN filtering failed"
    echo "Response: $RESPONSE"
fi

# ============================================================
# Test 17: PUT - Mirror with source port range
# ============================================================
log_test "PUT - Mirror with source port range expansion"
RESPONSE=$(curl -s -w "\n%{http_code}" -X PUT "${BASE_URL}/switches/api-test-mirror/desired-config" \
    -H "Content-Type: application/json" \
    -d '{
        "hostname": "mirror-test-switch",
        "model": "Aruba2930F",
        "management_ip": "192.168.99.60",
        "credentials": {"username": "admin", "password": "testpass123"},
        "vlans": [{"id": 100, "name": "test-vlan"}],
        "ports": [
            {"port_id": "1-5", "mode": "access", "vlan": 100, "enabled": true},
            {"port_id": "10", "mode": "access", "vlan": 100, "enabled": true}
        ],
        "port_mirrors": [
            {"session_id": "1", "source_ports": ["1-3"], "destination_port": "10", "direction": "both"}
        ]
    }')
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
check_status 201 "$HTTP_CODE" "Create switch with mirror port range"

# Verify source ports were expanded
RESPONSE=$(curl -s "${BASE_URL}/switches/api-test-mirror/desired-config")
if echo "$RESPONSE" | grep -q '"source_ports":\["1","2","3"\]'; then
    log_pass "Mirror source ports expanded from 1-3 to [1,2,3]"
else
    log_fail "Mirror source port expansion failed"
    echo "Response: $RESPONSE"
fi

# ============================================================
# Cleanup - Remove test switches
# ============================================================
echo -e "\n${YELLOW}=== CLEANUP ===${NC}"
curl -s -X DELETE "${BASE_URL}/switches/api-test-sw-01/desired-config" > /dev/null
curl -s -X DELETE "${BASE_URL}/switches/api-test-trunk/desired-config" > /dev/null
curl -s -X DELETE "${BASE_URL}/switches/api-test-mirror/desired-config" > /dev/null
echo "Cleaned up test switches"

# ============================================================
# Summary
# ============================================================
echo -e "\n${YELLOW}========================================${NC}"
echo -e "${YELLOW}       MANUAL API TEST SUMMARY${NC}"
echo -e "${YELLOW}========================================${NC}"
echo -e "Tests run: $TESTS_RUN"
echo -e "${GREEN}Passed: $PASSED${NC}"
echo -e "${RED}Failed: $FAILED${NC}"

if [ $FAILED -eq 0 ]; then
    echo -e "\n${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "\n${RED}Some tests failed!${NC}"
    exit 1
fi
