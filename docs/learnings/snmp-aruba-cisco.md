# SNMP on Aruba and Cisco: what the lab showed, and what is not built

Lab devices: HP/Aruba J9855A 2530-48G-2SFP+ (ArubaOS-Switch YA.16.10.0001, a
non-PoE model) and a Cisco Catalyst C9300 (IOS-XE 16.9), both reached over
serial for configuration and over a dedicated lab VLAN for SNMP, queried with
net-snmp from the host. The FortiSwitch results are in
[snmp-fortiswitch.md](snmp-fortiswitch.md).

**Status in the service:** `snmp.reads` and `allowed_hosts` are refused on these
vendors with a validation error (`allowed_hosts_are_refused_...`); the SNMP read
path (`?source=snmp`) is FortiSwitch only. This page is the evidence for that
choice and for the design questions below.

## ArubaOS-Switch

| Gate | Result |
|---|---|
| Read-only v2c community | `snmp-server community "<name>" operator restricted` is accepted; the running config shows `snmp-server community "<name>" operator` (the default `restricted` is not printed). |
| Who answers | **Only the management VLAN, when one is configured.** With `management-vlan 42` set, queries arriving on VLAN 1 got no answer at all (ping still worked); with it removed, SNMP answered on VLAN 1. |
| Manager restriction | `ip authorized-managers <ip> <mask> access operator access-method snmp` works: a listed address gets answers, any other gets none. It is **switch-wide, not per community** (it limits every SNMP community), with one `access-method` per entry. A snmp-only entry did not close the other management ports (22, 23 and 80 still accepted connections; not tested beyond TCP reachability). |
| Removing a manager | `no ip authorized-managers <ip>`. The form with the mask (`... 255.255.255.255`) is rejected: `Invalid input`. |
| IF-MIB / EtherLike | `ifName` lists ports `1`-`50`, then VLAN interfaces (`DEFAULT_VLAN`, `VLAN42`) and loopbacks `lo0`...; `ifOperStatus`, `ifHighSpeed` and `dot3StatsDuplexStatus` answered. |
| PoE | `pethPse*` is "No Such Object" on this non-PoE model; a PoE model was not available. |
| MAC table | BRIDGE-MIB `dot1dTpFdbTable` is indexed by the MAC (the standard way). **Q-BRIDGE `dot1qTpFdbTable` is populated**, indexed `fdbId.mac`, and `dot1qVlanFdbId` maps VLAN to fdbId, so a VLAN per MAC is available (unlike FortiSwitch). It also lists a "self" entry per VLAN (status 4, port 0) that the console's `show mac-address` does not. |
| Console comparison | The same four learned MACs, all on VLAN 1 port 1. |
| SNMPv3 | Not tried. |

## Cisco Catalyst 9300 (IOS-XE 16.9)

| Gate | Result |
|---|---|
| Read-only v2c community | `snmp-server community <name> RO` is accepted and read back unchanged (`snmp-server community <name> RO`). ACL/view variants were not tried. |
| IF-MIB / EtherLike | 78 interfaces, named in short form (`Gi1/0/1`, not `GigabitEthernet1/0/1`), among them `Gi0/0`, `Nu0`, `VLAN-n`, `Vl`, stack ports and `Te`/`Twe`/`Fo` types; `ifIndex` 8 is `Gi1/0/1`. Duplex answered for 41 of them. |
| PoE | RFC 3621 `pethPse*` is **populated** (288 rows). |
| MAC table | Empty ("No Such Object") in the default context for both BRIDGE-MIB and Q-BRIDGE-MIB. **BRIDGE-MIB answers per VLAN through `community@<vlan>`** (VLAN 30: five MACs; VLAN 10: one; the others none), so the table needs one set of walks per VLAN from the VTP VLAN list (`vtpVlanState`, 30 VLANs here), plus `dot1dBasePortIfIndex` in each context to turn bridge ports into interfaces. |
| Restricting managers | Not tested. |
| Comparison with `show` output, SNMPv3 | Not run. |

## What is not built, and the questions behind it

- **Aruba `allowed_hosts`**: the device can only restrict managers switch-wide.
  Mapping a per-community `allowed_hosts` onto it would silently restrict every
  other SNMP community on the switch (a monitoring system could stop working).
  Needs a decision: a switch-wide setting with that name, or "not supported".
- **`snmp.reads` on Aruba/Cisco** without a manager restriction would leave the
  service's read-only community open to anyone who knows its string. Needs a
  decision on whether that is acceptable.
- **Speed**: these switches are managed over SSH, not a 12-second serial
  console, so SNMP's main benefit (no console session) is smaller; a Cisco MAC
  table needs a walk per VLAN. The clear gain is **PoE and port status**, which
  the console path does not provide for them at all.
- **Management VLAN**: on an Aruba with `management-vlan` set, SNMP only works
  from that VLAN; the service's host must be on it.
