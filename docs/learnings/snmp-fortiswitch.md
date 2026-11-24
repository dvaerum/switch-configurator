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
   in it (below). **On the 124E (FortiSwitch 7.2.5) a community with no hosts
   table is answered by nobody** (toggled on a bench 124E: no hosts entry = every
   query times out; adding `10.10.132.0/24` made the next query answer within
   a few seconds; removing it silenced it again). So an
   `unrestricted` community needs a hosts entry there; since 0.26.4 the tool
   writes `0.0.0.0 0.0.0.0` for a community with no `allowed_hosts` (`show`
   prints that entry as a bare `edit 1`, because it is the default).

**The management address may be a DHCP lease.** On the IT-02876 switch (124F-FPOE) the management IP is leased by the `vlan42` interface (`set mode dhcp`, `set allowaccess ping`); `show system interface` prints no `set ip` for it, but `get system interface` lists it (`name: vlan42  mode: dhcp  ip: <addr> <mask>`). `snmp` has to be added to that interface's `allowaccess`.

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

## The production model: 124F-FPOE, FortiSwitch 7.2.5

Enabled through the service itself (`snmp.reads`, credential-backed community,
`allowed_hosts` = the host's address towards the switch) on a bench 124F-FPOE
whose management address is a DHCP lease on `vlan42`.

- The apply wrote what the 108F needed: `sysinfo` enabled, `snmp` appended to the
  leased interface's `allowaccess` (`ping` → `ping snmp`), the community with a
  `config hosts` entry for the host; the next read-back converged and the diff is
  empty. The community is shown by `show system snmp community` with only
  non-default fields (`status`/`query-*` enabled are omitted).
- Before the 0.23.6 fix the service generated `set name ""` (the credential was
  not read when configs load from several files); FortiOS rejects that with a
  multi-line error and the community was never created. Typing the same
  sequence by hand worked, which is how the cause was narrowed down.
- Tables answered on this model: `ifName`, `ifOperStatus`, `ifHighSpeed` (31 rows
  each), `dot3StatsDuplexStatus` (30), RFC 3621 `pethPsePortDetectionStatus` (29),
  `dot1dTpFdbPort` (~174 at the time of the first walk), `dot1qVlanStaticName` (8).
- Through the deployed service, same moment: link, speed and duplex agree with
  the console on all 24 ports; PoE state exists only over SNMP (as on the
  108F); the MAC table is identical (179 entries each). Latency: port status
  0.39s against 12.4s over the console; MAC table 0.12s against 3.5s.
  `x-data-source: snmp` for `source=snmp` and `auto`.
- One difference: SNMP also lists the `vlan42` VLAN interface (30 rows against
  the console's 29, which has only the physical ports and `internal`). Facts
  from walking `ifTable`/`ifXTable` on the bench (31 `ifIndex` rows):

  | ifIndex | ifDescr | ifName | ifType |
  |---|---|---|---|
  | 1-28 | `portN(primary)` | `portN` | `ethernetCsmacd(6)` |
  | 29 | `internal(primary)` | `internal` | `ethernetCsmacd(6)` |
  | 30 | `internal` | `internal` | `ethernetCsmacd(6)` |
  | 31 | `Management VLAN` | `vlan42` | `l2vlan(135)` |

  The `internal` rows are the inter-processor link, not a front port; they have
  the same type as real ports, so `ifType` cannot tell them apart (the mapping
  already merges the pair into one row). `vlan42` is the only `l2vlan` row.
  Recommendation (not implemented): if virtual interfaces should stay out of
  `port-status`, filter on `ifType == l2vlan(135)` (a standard, vendor-neutral
  signal, unlike matching `vlan*` names, which depend on how an admin named the
  interface); drop `internal` by name only if it should also match the console,
  since the console lists it.
- Not exercised on this unit: the console fallback with SNMP switched off
  (covered by unit tests only) and SNMPv3.

## SNMPv3 build feasibility

`async-snmp` 0.18.1 has the features `crypto-rustcrypto` (pure Rust, its
default) and `crypto-fips` (`aws-lc-rs`); the project builds it with
`default-features = false`, so no v3 authentication/privacy exists. Enabling
`features = ["crypto-rustcrypto"]` on the dependency in
`switch-configurator/Cargo.toml` makes `cargo check` pass in a scratch worktree
(it pulls in `aes`, `cbc`, `cfb-mode`, `des`, `digest`, `hmac`, `md-5`,
`sha1`, `sha2`; all registry crates, so `package.nix`'s `cargoLock` needs no
extra hashes). Not enabled: it only compiles the crypto, and using it still
needs config fields for user/auth/priv (secrets via credentials) and a lab test
against a real agent.

## Concurrent walks make the agent return a short table, silently

Measured on a bench FortiSwitch 124E (7.2.5) with ~600 learned MAC addresses,
walking `dot1dTpFdbPort` (`snmpbulkwalk -v2c -Cr25`, no retries, nothing else
querying the switch):

| readers at once | entries returned |
|---|---|
| 1 (3 runs) | 600, 600, 600 |
| 6 (18 walks) | 147 to 229, never 600 |

No error, no timeout, no error PDU: the agent ends the walk with an ordinary
end of table, so a client cannot tell a short table from a complete one. The
same showed through the service (`GET .../mac-table?source=snmp`: 592-595
entries for one reader, 45-140 for six, all HTTP 200) and on the IT-02876
124F-FPOE (70, 125 and 15 entries against ~200, worse with more readers).
`async-snmp` treats an exception value or an out-of-subtree OID as the normal
end of a walk, which is correct for the library but means no walk-level check
can catch this.

So since 0.27.2 `snmp::gate` allows one lookup at a time per switch, lets a
caller that arrives mid-walk share that walk (no cache: the result is dropped
when the walk completes), and walks again (at most 3 times, keeping the
largest result: a cut-short walk only loses rows) when a table comes back
below 70% of the previous one. Anything else querying the same agent (a
monitoring system) can still disturb it; the retry covers that case.

Reproduce it (and check a fix) with `examples/snmp_probe.rs`, which runs the same
concurrent load through the raw walker (`raw`) or the gated lookup (`gated`).
On the bench 124E (~582 entries) six readers x four rounds gave: `raw`: 106,
199-201 (23 of 24 reads short, one timeout), 40 s; `gated`: 581-583 on all 24
reads, 9 s.

## PoE over SNMP and the console (FortiSwitch 108F-POE, 7.2.8)
Measured on the lab 108F, no PoE device attached.
- **`pethPsePortTable` (RFC 3621) is answered in full** by one walk of `1.3.6.1.2.1.105.1.1.1`: admin setting, detection status, class (`1`..`5` = class0..class4), and the counters MPSAbsent, InvalidSignature, PowerDenied, OverLoad, Short, one row per PoE port. The service walks it once and splits by column.
- **No per-port watts over SNMP.** `1.3.6.1.4.1.12356.106` holds system info only (serial, version, CPU, memory). The main PSE group `1.3.6.1.2.1.105.1.3` gives the unit total (65 W budget, consumption, threshold). Per-port watts, voltage and current exist only on the console: `get switch poe inline` (all ports, with class) and `diagnose switch poe status <port>`.
- **The SNMP agent lags the switch by up to ~30 s for PoE.** After `set poe-status disable` the console showed `Disabled` within a few seconds, but `pethPsePortAdminEnable` changed only ~30 s later. Never confirm a PoE change by SNMP read-back.
- **The switch applies a PoE change a few seconds after the console accepts it.** A `get switch poe inline` right after the command still showed the old setting; the next one (~2 s later) the new one. A read-back has to retry briefly.
- **`ifLastChange` is 0 for every interface**, including ports that just came up, so "time since the last link change" cannot be read from this agent.
- Console timing: about 6 s per port for a PoE command list on the serial console; `execute poe-reset <port>` returns silently after about 1 s (what it does to the port was not measured).

