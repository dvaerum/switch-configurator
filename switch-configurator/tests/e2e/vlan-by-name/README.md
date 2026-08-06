# E2E: VLAN references by name

Proves that referencing a VLAN by **name** on a port's `vlan` / `tagged_vlans`
produces the exact same switch commands as referencing it by numeric **ID**.

## Files

Paired configs that differ *only* in numeric-vs-name VLAN references:

| Numeric baseline | Named equivalent | Switch |
|---|---|---|
| `aruba-2530-8g-numeric.yaml` | `aruba-2530-8g-named.yaml` | Aruba 2530-8G PoE+ |
| `cisco-c9300-numeric.yaml` | `cisco-c9300-named.yaml` | Cisco Catalyst 9300 |

`aruba-unknown-name.yaml` — a config whose port references a non-existent VLAN
name; used to verify the switch is skipped at load time *before* any connection.

Serial configs use the stable `/dev/serial_*` symlinks (they survive reboots).

## Hardware-independent check (CI)

`../../vlan_name_e2e_equivalence.rs` loads each pair, computes the diff against an
empty state, and asserts the generated command preview is identical (port
commands in exact order; full set order-insensitive since VLAN-definition order
is hash-derived). Run anywhere:

```bash
cargo test --test vlan_name_e2e_equivalence
```

This is the most precise proof: it compares the actual generated switch commands
deterministically, with no hardware and no writes.

## Live on-hardware check (IT-03400)

`run.sh` performs the same comparison over a real serial connection using
`--one-off --dry-run` (no writes). Run on the host with the switches attached:

```bash
./switch-configurator/tests/e2e/vlan-by-name/run.sh
```

It connects to each switch, reads current state, prints the commands it *would*
send, and diffs the numeric vs named transcripts. It also confirms the
unknown-name config is rejected before connecting.
