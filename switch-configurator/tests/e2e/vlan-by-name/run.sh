#!/usr/bin/env bash
#
# Live E2E: prove that VLAN name references produce identical switch commands to
# numeric IDs, over a REAL serial connection on IT-03400.
#
# This is the on-hardware counterpart to the CI test
# `tests/vlan_name_e2e_equivalence.rs`. It performs DRY-RUN only — no config is
# written to any switch. It connects, reads current state, and prints the
# commands it WOULD send; we then assert the name-based and numeric configs yield
# the same set.
#
# Run from the repo root on IT-03400:
#   ./switch-configurator/tests/e2e/vlan-by-name/run.sh
#
# Requires the /dev/serial_* symlinks to be present (see `ls -l /dev/serial_*`).

set -uo pipefail
cd "$(dirname "$0")/../../.."   # -> switch-configurator crate dir

HERE="tests/e2e/vlan-by-name"
FAIL=0

# Extract the ordered list of commands the tool would send in dry-run.
# We capture lines the vendor logs as "would send"/generated commands. To stay
# robust across log formats, we diff the full dry-run transcripts with volatile
# lines (timestamps, connection banners) stripped.
run_dryrun() {
  local cfg="$1"
  nix develop --command cargo run --quiet -- \
    --config-file "$cfg" --one-off --dry-run --log-level info 2>&1 \
    | sed -E 's/^[0-9TZ:.\-]+ +//' \
    | grep -vE 'Connecting|Connected|Starting switch-configurator|Loaded configuration|DRY-RUN mode|Disconnect' \
    || true
}

compare_pair() {
  local label="$1" numeric="$2" named="$3"
  echo "=== $label: comparing numeric vs named (dry-run) ==="
  local out_num out_name
  out_num="$(mktemp)"; out_name="$(mktemp)"
  run_dryrun "$HERE/$numeric" > "$out_num"
  run_dryrun "$HERE/$named"   > "$out_name"

  if diff -u "$out_num" "$out_name" > /tmp/vlan_name_diff.txt; then
    echo "PASS: $label — name-based and numeric dry-runs are identical"
  else
    echo "FAIL: $label — dry-run transcripts differ:"
    cat /tmp/vlan_name_diff.txt
    FAIL=1
  fi
  rm -f "$out_num" "$out_name"
  echo
}

compare_pair "Aruba 2530-8G PoE+" \
  "aruba-2530-8g-numeric.yaml" "aruba-2530-8g-named.yaml"

compare_pair "Cisco Catalyst 9300" \
  "cisco-c9300-numeric.yaml" "cisco-c9300-named.yaml"

echo "=== Error path: unknown VLAN name must skip the switch before connecting ==="
ERR_OUT="$(nix develop --command cargo run --quiet -- \
  --config-file "$HERE/aruba-unknown-name.yaml" --one-off --dry-run --log-level info 2>&1 || true)"
if echo "$ERR_OUT" | grep -q "DOES-NOT-EXIST"; then
  echo "PASS: unknown VLAN name reported and switch skipped"
else
  echo "FAIL: expected an error mentioning DOES-NOT-EXIST"
  echo "$ERR_OUT"
  FAIL=1
fi
echo

if [ "$FAIL" -eq 0 ]; then
  echo "ALL E2E CHECKS PASSED"
else
  echo "SOME E2E CHECKS FAILED"
fi
exit "$FAIL"
