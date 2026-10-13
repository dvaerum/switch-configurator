# SNMP on FortiSwitch: what was verified on hardware

Lab device: FortiSwitch-108F-POE v7.2.8 build0495 (standalone), queried from a
Linux host over a dedicated lab VLAN with `snmpget`/`snmpwalk` (net-snmp).
The production 124F-FPOE is re-checked when the feature is rolled out there;
until then treat anything below as "true for 108F-POE 7.2.8".

## What is needed for a query to be answered

All of these, confirmed one at a time (each toggled off and on, with a few
seconds' settling time between a change and the query):

1. `config system snmp sysinfo` → `set status enable` (default: disabled).
2. A community (`config system snmp community`) with the query status on
   (the default for `query-v1-status` / `query-v2c-status` is enable).
3. **`snmp` in the `allowaccess` of the address that is queried.** With the
   management IP configured as a *secondary* address (`config secondaryip`),
   `snmp` on the primary interface alone gets no answer; it has to be in that
   secondary entry's own `allowaccess`. Changes take a few seconds to apply.
4. If the community has a `config hosts` table, the querying address must be
   in it (below).

## `config hosts` restricts who may query

Tested with `edit 1` / `set ip <addr> <mask>` inside the community
(`set interface` is not needed): the right address gets answers, an unused
address in the table gets none, and putting the right address back restores
answers. The same table also holds trap receivers, so the service writes a
community with `allowed_hosts` with both trap statuses disabled
(`set trap-v1-status disable` / `set trap-v2c-status disable`) and reads
those back as an allow-list; a community with trap status enabled and hosts
is read as trap receivers.

## Other facts the implementation relies on

- `show` prints only non-default settings, so a community added by the
  service shows `set name`, the hosts block and the two disabled trap
  statuses; an enabled sysinfo shows `set status enable`.
- `set ip <addr> <mask>` on the `internal` interface is refused (`-651`) while
  `set mode dhcp` is on; add a `config secondaryip` entry instead.
- At most 3 communities fit (FortiSwitchOS administration guide).
- SNMP is read-only on FortiSwitch; a community's access level can't be
  read back, so the service treats it as equal to the desired one.
- An empty `set description ''` makes `sysDescr` an empty string; `sysName`
  (the serial number by default) is a better liveness OID.
- Community strings travel in clear text in SNMPv2c (seen on the wire in
  the lab): keep the SNMP segment isolated and restrict managers with
  `allowed_hosts`.

## Reading data over SNMP (verified on the same 108F-POE)

Walked with `snmpwalk` and then through the service, compared with the
console for the same moment (`get switch physical-port`,
`diagnose switch mac-address list`):

| Data | OIDs | Result |
|---|---|---|
| Interface names, link | IF-MIB `ifName` 1.3.6.1.2.1.31.1.1.1.1, `ifOperStatus` 1.3.6.1.2.1.2.2.1.8 | answered; `ifIndex` 1-10 = `port1`-`port10`, `internal` is listed twice (ifIndex 11 and 12) |
| Speed | `ifHighSpeed` 1.3.6.1.2.1.31.1.1.1.15 | answered, but reports a nominal 1000 for DOWN ports (the console prints no speed for them) |
| Duplex | EtherLike `dot3StatsDuplexStatus` 1.3.6.1.2.1.10.7.2.1.19 | answered: 3 = full when up, 1 = unknown when down |
| PoE | RFC 3621 `pethPsePortAdminEnable` .105.1.1.1.3 / `pethPsePortDetectionStatus` .105.1.1.1.6, indexed `group.port` (group 1, port N = `portN`) | answered; the console path never exposed PoE state |
| MAC table | BRIDGE-MIB `dot1dTpFdbTable` 1.3.6.1.2.1.17.4.3.1 (address .1, bridge port .2, status .3) | answered; the row index is a counter, the MAC is in the address column; status 3 learned, 4 self |
| MAC VLAN | Q-BRIDGE `dot1qTpFdbTable` 1.3.6.1.2.1.17.7.1.2.2 | **noSuchObject**: no VLAN per MAC over SNMP |

After the mapping rules in `src/snmp/mapping.rs` (speed/duplex only for a port
that is up; the duplicate `internal` collapses to the entry with a speed) the
SNMP and console answers were identical for all 11 ports and all 3 MACs, apart
from the PoE state (SNMP only) and the VLAN (console only).

Latency from the host to the switch (through the service): port status 0.07-0.35s
(median ~0.13s) against ~11.7s over the console; MAC table ~1.1s against ~10.5s.

**Quirk: do not walk the FDB table from several requests at once.** Three
parallel walks of `dot1dTpFdbTable` columns made the agent stop answering
(requests timed out), while the interface tables tolerate parallel walks. The
service walks the whole FDB table once, front to back.

SNMPv3 was not tried (the client is built without the v3 crypto backend).
