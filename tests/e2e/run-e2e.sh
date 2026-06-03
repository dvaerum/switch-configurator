#!/usr/bin/env bash
# E2E browser tests for switch-configurator-ui
# Starts backend + UI, runs Playwright tests against Firefox headless, cleans up
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SOCKET_PATH="/tmp/e2e-test-$(date +%s).sock"
BACKEND_PORT=4098
UI_PORT=8099
CLEANUP_PIDS=()

cleanup() {
    echo "Cleaning up..."
    for pid in "${CLEANUP_PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    rm -f "$SOCKET_PATH"
}
trap cleanup EXIT

# Create test config
TMPCONFIG=$(mktemp /tmp/e2e-config-XXXXXX.yaml)
cat > "$TMPCONFIG" << 'EOF'
switches:
  - id: demo-sw-01
    hostname: demo-switch-1
    model: Aruba2930F
    management_ip: "192.168.1.1"
    credentials:
      username: admin
      password: admin
      connection_type: ssh
    vlans:
      - id: 1
        name: default
      - id: 10
        name: management
        ip_config: dhcp
      - id: 20
        name: users
      - id: 30
        name: servers
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
        description: "Uplink"
        enabled: true
      - port_id: "2"
        mode: access
        vlan: 20
        description: "User ports"
        enabled: true
      - port_id: "3"
        mode: access
        vlan: 20
        description: "User ports"
        enabled: true
      - port_id: "24"
        mode: trunk
        vlan: 1
        allowed_vlans: [10, 20, 30]
        description: "Trunk"
        enabled: true
    snmp:
      communities:
        - name: public
          access: operator
      trap_receivers:
        - host: "192.168.1.100"
          community: public
          version: "2c"
      enabled_traps:
        - mac-notify
        - link-change
EOF

echo "=== Starting backend ==="
"$ROOT_DIR/target/debug/switch-configurator" \
    --config-file "$TMPCONFIG" \
    --port $BACKEND_PORT \
    --socket "$SOCKET_PATH" \
    --log-level warn &
CLEANUP_PIDS+=($!)
sleep 2

echo "=== Starting UI ==="
"$ROOT_DIR/target/debug/switch-configurator-ui" \
    --backend-socket "$SOCKET_PATH" \
    --listen "127.0.0.1:$UI_PORT" \
    --log-level warn &
CLEANUP_PIDS+=($!)
sleep 1

# Verify services are up
echo "=== Verifying services ==="
curl -sf "http://127.0.0.1:$UI_PORT/health" > /dev/null || { echo "UI not responding"; exit 1; }
echo "UI is up on port $UI_PORT"

echo "=== Running Playwright E2E tests (Firefox headless) ==="
cd "$SCRIPT_DIR"
export PLAYWRIGHT_BROWSERS_PATH=$(nix build nixpkgs#playwright-driver.browsers --no-link --print-out-paths 2>/dev/null)
export UI_URL="http://127.0.0.1:$UI_PORT"
npx @playwright/test@latest test --config=playwright.config.mjs
EXIT_CODE=$?

rm -f "$TMPCONFIG"
exit $EXIT_CODE
