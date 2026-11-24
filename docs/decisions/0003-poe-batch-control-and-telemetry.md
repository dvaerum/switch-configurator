# 0003 — PoE control for several ports, PoE telemetry and `/metrics`

## Status
**Accepted as a plan** (not implemented). Written 2026-11-24 after a researched
review and a design interview with the user; nothing below is built yet.

## Context
The provision project (DECT rig on IT-02876, a 124F-FPOE) asked for three
things (conexus `msg_ffb1f28c…`, `msg_b1973ef0…`):

1. Switch PoE for **several ports in one request** (today: one port per
   request, one console turn each) so it can provision a few units at a time.
2. See **per-port PoE state and delivered power** (watts, class, ...) because a
   base station that reboots every ~55 s shows up only as a power sawtooth.
3. An optional **`/metrics`** endpoint (per-port power/state, SNMP walk time and
   retries, SNMP source up).

The real goal behind 2 and 3 is *detecting reboot loops*, not watts as such.

## What is verified, and what is not
Verified in this repo's code (2026-11-24):

- All console work for one switch (apply, poe-on/off/reset, console reads)
  shares **one turn**; PoE endpoints answer **202** and wait up to **600 s**.
- `poe-reset` = disable, a fixed **3 s** wait (three `waiting` events), enable
  (enable retried once). One port per request.
- **Latent defect:** `execute_raw_commands` returns FortiOS's text without
  inspecting it. FortiOS reports a refused command as text (`command parse
  error`, `Command fail. Return code …`), not as an error. `is_command_rejected`
  exists but only the read lookups use it. So today `poe-on/off/reset` report
  `done` **even if the switch refused the command**, and a failed *disable*
  leaves a port in an unknown power state.
- Since 0.27.2 SNMP lookups are serialised per switch (`snmp::gate`).

Researched (sources below). **Verified** = read in the primary source;
**unverified** = a claim I could not confirm.

| Question | Finding | Status |
|---|---|---|
| Does RFC 3621 give per-port watts? | The per-port group has admin, detection status, priority, class, and fault counters. `UNITS "Watts"` appears only on the two **main-PSE** objects (`pethMainPsePower`, `pethMainPseConsumptionPower`). | Verified |
| Per-port watts over SNMP on FortiSwitch? | A security blog names `FORTINET-FORTISWITCH-MIB::fswPortPoePowerConsumption.<port>` (module base `1.3.6.1.4.1.12356.106`). Not confirmed by any primary source; the Observium MIB page would not render. | **Unverified** |
| Per-port watts over the console? | Fortinet's 7.2.5 admin guide documents `diagnose switch poe status <port>`: sample `Port(6) Power:4.20W, Power-Status: Delivering Power`, plus `Voltage: 54.00V` and `Current: 71mA`. | Verified (sample); exact full output not read |
| Native reset command? | `execute poe-reset <port>` is documented (no sample output, so duration/behaviour unknown). | Verified exists; semantics **unverified** |
| `get switch poe inline` | Documented as the power-capacity command. **Read on a lab 108F-POE (FortiSwitchOS 7.2.8): one command prints, for ALL ports, `Status`, `State` (e.g. `Searching`), `Max-Power(W)`, `Power-consumption(W)`, `Priority`, `Class`, `Error`, plus unit budget (65.00W), guard band, unit consumption and temperature.** So per-port watts over the console cost one command, not one per port. | **Verified on hardware (7.2.8; 124F is 7.2.5, same family)** |
| Prometheus unit rule | Use base units. Power: *"Prefer exporting a counter of joules, then `rate(joules[5m])` gives you power in Watts."* We only have instantaneous watts, so a `_watts` gauge is an accepted exception, documented. | Verified |
| Exporter rules | Collect **on scrape**, synchronously, no own timers; avoid "scrape meta" metrics that duplicate Prometheus' own (`up`, scrape duration); report a failed target instead of silently omitting it. | Verified |
| Rust library | `prometheus-client` (prometheus/client_rust) is the OpenMetrics implementation with a `Collector` trait. Release cadence not checked. | Recommended; check crates.io at implementation |

## Critical review of my first proposal
1. **New path shape.** `POST /switches/{id}/poe` + an `action` field breaks the
   API's convention (action in the path). → Use the existing verbs without the
   trailing port (decided with the user): `POST /switches/{id}/poe-on|poe-off|poe-reset`
   with `{"ports":[...]}`.
2. **Per-port `execute` calls skew a reset.** Ports would lose power at
   different moments (N × ~1–2 s apart) and regain it likewise; "one 3 s wait for
   all" is false for 24 ports. → Send **one command list per phase** (all
   disables, wait, all enables).
3. **"Keep going on failure" needs attribution.** One list gives no per-port
   result unless the text output is read. → Record which command indices belong
   to which port and classify every output.
4. **Fail towards power.** A failed or half-applied *disable* must not leave a
   port off. → If any phase errors after a disable may have taken effect,
   attempt a best-effort enable of all affected ports before reporting.
5. **"done" means "commands sent", not "state changed".** → Verify by reading
   back through the gated SNMP lookup when SNMP reads are on; say so explicitly
   when verification is not possible.
6. **Durations I quoted are estimates from code, not measurements.** → Measure
   first (see Hardware plan) and put real numbers in the docs.
7. **Metrics would fight PoE operations.** Per-port watts from the console need
   the switch's single console turn; polling that every 15–30 s starves PoE
   requests. → `/metrics` is **SNMP-only**, never the console.
8. **My metric list had scrape-meta metrics** (`walk_seconds`, `walk_attempts`
   per scrape), which Prometheus advises against. → A per-switch `up` gauge and
   an event **counter** (`..._rewalks_total`) instead.
9. **Watts may be the wrong signal.** For reboot loops, link flaps
   (`ifLastChange`, already in IF-MIB), RFC 3621 counters (MPS-absent, denied,
   overload, short) or detection-status transitions may show the loop without
   any enterprise OID. → Measure which signal actually moves before building
   anything for watts.
10. **Scope.** Batch control, telemetry and metrics are independent; bundling
    them delays the small, valuable fix (1 below).

## Decision (agreed with the user in a design interview, 2026-11-24)
Small, independently releasable steps, each test-first, each with a version bump
and changelog entry. Principle: **publish raw facts, draw no conclusions**
(no `is_rebooting`/`healthy` fields, no thresholds); consumers decide.

### R0 — 0.27.3 (patch): PoE operations detect a refused command
- Apply `is_command_rejected` to the output of every PoE command; a refusal is a
  `failed` event with the switch's text, recorded as an error.
- On a failed or erroring *disable* in `poe-reset`, make a best-effort enable
  before failing (fail towards power).
- Tests: fake console returning FortiOS rejection text; existing single-port
  endpoint tests stay green.
- Why first: a bug today, and the later "completed" must not sit on a check
  that can lie.

### R1 — 0.28.0: read the PoE state of a port
- `GET /switches/{id}/poe` (all ports) and `GET /switches/{id}/poe/{port}`,
  served from the existing background-refreshed cache (ADR 0001 headers), so no
  request waits for the switch.
- Per port, raw values only: link state, seconds since last link change, PoE
  admin setting, PoE power state (delivering / searching / fault …), class and
  the fault counters **where the agent answers** (otherwise absent, never
  guessed), plus `last_operation {action, status, detail, at}` kept in memory.
  `status` is `in-progress` | `completed` | `failed`; queued work shows as
  `in-progress`. There is no operation id: the port is the thing you ask about.
- Walk `pethPsePortTable` once through the gate (not two column walks).
- Freshness: the 60 s lookup refresher is too slow and skips a busy switch, so a
  separate SNMP-only refresher (every 10 s, no console turn) keeps the port-status
  cache warm. Shipped in 0.27.4.
- Watts are **not** in the default read (console-only on FortiSwitch); possible
  later behind an explicit option after hardware evidence (H4).

### R2 — 0.29.0: batch PoE control
- **API:** `POST /switches/{id}/poe-on`, `/poe-off`, `/poe-reset`, body
  `{"ports":["1","2","3"]}`; 400 for empty/blank/more than **64** ports;
  duplicates dropped (order kept); all ports validated **before anything runs**
  (404 unknown switch; 400 no PoE; one 400 naming **all** offending ports);
  202 echoing the normalised list.
- **Status:** the per-port `poe-set` / `poe-reset` SSE events keep their `stage`
  names unchanged and gain an additive `status` (`in-progress` | `failed` |
  `completed`), mirrored in `last_operation`.
  - `completed` means the **admin setting reads back as requested** (SNMP,
    through the gate, ≤ ~10 s). Power state is reported next to it and never
    turns the status into `failed` (a device that is off or unplugged is
    normal). For `poe-reset`: the setting is back to on.
  - Refusal, or setting not reached → `failed` with the reason.
  - SNMP reads unavailable → `completed` right after acceptance with the note
    "not confirmed".
  - The legacy `done` stage is still sent at acceptance.
- **Core:** one module (`api/poe.rs`): plan → execute → confirm. The single-port
  endpoints become the one-port case of the same core. One turn, one pooled
  session, one command list per phase. Reset: **all ports off together → one
  wait (3 s) → all on together** (the switch's own power budget denies what does
  not fit; that is reported per port; verify on the lab 108F). No stagger option
  until someone needs it. Transport error ⇒ session discarded, remaining ports
  failed with the error, never silently skipped; enable keeps its single retry.
- **Vendors:** FortiSwitch gets rejection detection; Aruba keeps today's
  behaviour (no evidence of its refusal markers; documented limitation).
- **Tests:** mock vendor recording command lists; FortiOS rejection text;
  per-port partial failure; reset phase order; fail-towards-power; one queue
  entry per batch; start-paused time for the wait. Early failing tests are
  parked in the session scratchpad (`parked-batch-poe-tests.patch`).

### R3 — 0.30.0: `/metrics` (Prometheus text, optional)
- Served from the same cache the PoE read uses (no delay, no switch traffic at
  scrape time); numbers are at most one refresh interval old.
- `switch_snmp_up{switch}`, `switch_snmp_rewalks_total{switch}`, per-port PoE
  admin/state/class, fault counters `..._total`, link state and
  `switch_port_last_change_seconds`. Base units, `switch_` prefix. Raw data
  only; no watts, no verdicts.
- No auth (closed lab network); binds wherever the API binds.

### Prerequisite for R4 — shared library crate (other repo, needs its owner's OK)
Extract the trap listener and the known-trap decoding from
`snmp-trap-event-handler` into a small library crate used by both programs
(git pin), instead of copying it. That repo's bulk (hooks, Kea, store) is not
needed here.

### R4 — 0.31.0: SNMP traps inside the service
- Embedded listener (address and port configurable, so it can sit beside the
  router's handler that owns 162); each switch is told where to send traps.
- A link or PoE trap refreshes that switch's cache at once (≈ 1 s instead of the
  poll interval) and is published on our own `/events` SSE stream with its
  origin (`poll` | `trap`). Polling stays as the fallback, so a missed trap only
  costs freshness.
- FortiSwitch traps used: standard `linkUp`/`linkDown` and the RFC 3621 PoE
  on/off notification; verify on the lab 108F before relying on them.

### Release order and later work
R0 → R1 → R2 → R3 → (crate step) → R4. Watts and a native `execute poe-reset`
stay open until hardware evidence exists (H4/H5).

## Hardware plan (needed before R1/R4; nothing is guessed)
Decision (user, 2026-11-24): **no measurements on the IT-02876 switch.** They
are done on the lab **FortiSwitch 108F-POE** attached to this machine
(FortiSwitchOS 7.2.8, SNMP now enabled, lab address 192.168.250.13). Caveat to
state in the docs: production is a 124F-FPOE on 7.2.5; anything that is
firmware-specific is marked as measured on 7.2.8.

| # | What | Needs |
|---|---|---|
| H1 | Time poe-on/off/reset from the SSE events (real numbers for the docs) | cable (lab 108F into VLAN 3250) |
| H2 | One gated walk of the whole `pethPsePortTable`: do class and counters answer? | cable |
| H3 | Walk `1.3.6.1.4.1.12356.106` for PoE objects; confirm or drop `fswPortPoePowerConsumption` | cable |
| H4 | Output of `diagnose switch poe status <port>` and `execute poe-reset <port>` (duration, text on success and on refusal); `get switch poe inline` already read | none (console only) |
| H5 | What the raw link/PoE data and counters look like when a device cycles; watts need a real PoE load | a PoE device on a lab port |
| H6 | Does a batch reset of many ports exceed the power budget, and how is a denied port reported? | cable + PoE loads |
| H7 | Which traps the 108F sends for link and PoE changes | cable, a trap receiver |

## Risks
| Risk | Mitigation |
|---|---|
| New `status` field breaks a strict SSE consumer | Additive and announced to provision before release |
| Confirmation lengthens the turn | Bounded (~10 s); skipped without SNMP reads |
| Best-effort enable after a failed disable hides the cause | The failure is still reported with the switch's text |
| Cache up to one interval old | Served with ADR 0001 headers; traps (R4) shorten it |
| Many ports powered at once | Switch budget protection; per-port result; H6 |
| Enterprise OID does not exist | Watts stay out until verified; nothing shipped on a guess |
| Trap dependency on a second repo | Library crate behind a git pin; polling remains the fallback |
| Aruba rejection markers unknown | Out of scope until evidence; documented |

## Non-goals
No verdicts or thresholds in the API, no external poller (it would break the
agent's walks), no change to the queue semantics, no auth, no watts in
`/metrics`, no stagger option.

## Open items (defaults chosen; say if you disagree)
1. Batch limit 64 ports.
2. Is a PoE load (a DECT base station or any PoE device) available for the lab
   108F for H5/H6?
3. Cabling the lab 108F (untagged VLAN 3250) is needed before H1–H3, H6, H7.

## Sources
- Fortinet, [FortiSwitchOS 7.2.5 — Configuring power over Ethernet on a port](https://docs.fortinet.com/document/fortiswitch/7.2.5/administration-guide/861226)
- IETF, [RFC 3621 — Power Ethernet MIB](https://www.rfc-editor.org/rfc/rfc3621.html)
- Prometheus, [Writing exporters](https://prometheus.io/docs/instrumenting/writing_exporters/) and [Metric and label naming](https://prometheus.io/docs/practices/naming/)
- [prometheus-client (client_rust)](https://github.com/prometheus/client_rust)
- Observium, [FORTINET-FORTISWITCH-MIB](https://mibs.observium.org/mib/FORTINET-FORTISWITCH-MIB/) (page did not render objects)
