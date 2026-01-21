#!/usr/bin/env bash
# Quick script to check if mirror monitor commands are in running-config

cd "$(dirname "$0")/../.."

echo "=== Checking mirror configuration on test switch ==="
echo ""

nix develop --command cargo run -- --config-file tests/configs/test-mirror-check.yaml --one-off 2>&1 | \
  awk '/show running-config/{flag=1} /show snmp-server traps/{flag=0} flag' | \
  grep -A 3 "^interface 33\|^interface 34\|^interface 35\|^interface 36\|^mirror-port"

echo ""
echo "=== Expected: All 4 ports should have 'monitor' command ==="
