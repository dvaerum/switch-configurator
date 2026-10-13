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
| Who answers | **Only the management VLAN, when one is configured.** With `management-vlan 42` set, queries arriving on VLAN 1 got no answer at all (ping still worked); with it removed, SNMP answered on VLAN 1. Not tested: a query arriving *on* the management VLAN itself (the lab host is not on VLAN 42). |
| Manager restriction | `ip authorized-managers <ip> <mask> access operator access-method snmp` works: a listed address gets answers, any other gets none. It is **switch-wide, not per community** (it limits every SNMP community), with one `access-method` per entry. A snmp-only entry did not close the other management ports (22, 23 and 80 still accepted connections; not tested beyond TCP reachability). |
| Removing a manager | `no ip authorized-managers <ip>`. The form with the mask (`... 255.255.255.255`) is rejected: `Invalid input`. |
| IF-MIB / EtherLike | `ifName` lists ports `1`-`50`, then VLAN interfaces (`DEFAULT_VLAN`, `VLAN42`) and loopbacks `lo0`...; `ifOperStatus`, `ifHighSpeed` and `dot3StatsDuplexStatus` answered. |
| PoE | `pethPse*` is "No Such Object" on this non-PoE model; a PoE model was not available. |
| MAC table | BRIDGE-MIB `dot1dTpFdbTable` is indexed by the MAC (the standard way). **Q-BRIDGE `dot1qTpFdbTable` is populated**, indexed `fdbId.mac`, and `dot1qVlanFdbId` maps VLAN to fdbId, so a VLAN per MAC is available (unlike FortiSwitch). It also lists a "self" entry per VLAN (status 4, port 0) that the console's `show mac-address` does not. |
| Console comparison | The same four learned MACs, all on VLAN 1 port 1. |
| Console comparison (interfaces) | `show interfaces brief` against `ifAdminStatus` / `ifOperStatus` / `ifHighSpeed` / `dot3StatsDuplexStatus` for ports 1-50 (tested with `management-vlan` removed): admin and link state match on all 50 ports. `ifHighSpeed` equals the console's Mode speed on all 50 (100 on the two ports set to 100, 0 on the two empty SFP slots shown as `.`). Duplex matches (full) on the connected port and the administratively disabled ports, but reads **half (2) on the ten enabled ports that are down** while the console shows `1000FDx`: as on the other vendors, speed and duplex are only trustworthy while the link is up. |
| SNMPv3 | Not tried. |

## Cisco Catalyst 9300 (IOS-XE 16.9)

| Gate | Result |
|---|---|
| Read-only v2c community | `snmp-server community <name> RO` is accepted and read back unchanged (`snmp-server community <name> RO`). ACL/view variants were not tried. |
| IF-MIB / EtherLike | 78 interfaces, named in short form (`Gi1/0/1`, not `GigabitEthernet1/0/1`), among them `Gi0/0`, `Nu0`, `VLAN-n`, `Vl`, stack ports and `Te`/`Twe`/`Fo` types; `ifIndex` 8 is `Gi1/0/1`. Duplex answered for 41 of them. |
| PoE | RFC 3621 `pethPse*` is **populated** (288 rows). |
| MAC table | Empty ("No Such Object") in the default context for both BRIDGE-MIB and Q-BRIDGE-MIB. **BRIDGE-MIB answers per VLAN through `community@<vlan>`** (VLAN 30: five MACs; VLAN 10: one; the others none), so the table needs one set of walks per VLAN from the VTP VLAN list (`vtpVlanState`, 30 VLANs here), plus `dot1dBasePortIfIndex` in each context to turn bridge ports into interfaces. |
| Restricting managers | **Per community, with a standard ACL.** `ip access-list standard LABSNMP` / `permit <other host>` / `snmp-server community <name> RO LABSNMP`; the running config reads back `snmp-server community <name> RO LABSNMP` plus the ACL section. A query from the lab host (not in the ACL) got **no answer** (timeouts, no error); after `no permit <other host>` / `permit <lab host>` the same query was answered immediately. The ACL and community were removed afterwards (`no snmp-server community <name>`, `no ip access-list standard LABSNMP`). Unlike Aruba this is per community, so an `allowed_hosts` mapping would not affect the other communities. |
| Console comparison (same moment) | **Port status**: for `Gi1/0/1`-`24`, link up/down matches `show interfaces status` once `ifAdminStatus` is read too (`ifOperStatus` is `down` for both `notconnect` and `disabled`; `ifAdminStatus` down = `disabled` on exactly the two ports the console shows so). The one connected port reported 1000 Mb/s full duplex both ways. On the other ports `ifHighSpeed` is a nominal `1000` (not the configured `100`) and duplex is "unknown" (1): same quirk as the FortiSwitch, so speed and duplex are only meaningful while the link is up. **MAC table**: per-VLAN walks gave the same entries as `show mac address-table` per VLAN (VLAN 10: 1, VLAN 30: 5, the four learned ones on bridge port 1 = `Gi1/0/1`); SNMP lacks the 21 "All"/CPU static entries and reports the VLAN interface's own MAC on bridge port 0 (no interface). **PoE**: `pethPsePortAdminEnable` equals the console's Admin column for all 24 ports (`auto` = true, `off` = false); `pethPsePortDetectionStatus` was `searching` for the `auto` ports and `disabled` for the `off` ones, consistent with Oper `off`; no device was drawing power, so power values were not compared. |
| SNMPv3 | Not run. |

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
