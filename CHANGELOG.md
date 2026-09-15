# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **FortiSwitch 124F-FPOE port count was wrong** (`switch-configurator` 0.8.1): registered as 26 total ports (24× PoE+ copper + 2× SFP+ uplinks); real hardware has 28 ports — the same 24 PoE+ copper ports plus 4 SFP+ uplinks (25-28), not 2. `total_ports()` and `port_capabilities()` now reflect this.
- **VLAN dropdowns on the Ports edit page truncated long names** (`switch-configurator-ui` 0.5.1): the untagged-VLAN and tagged-VLANs `<select>` elements had a hardcoded `width: 130px`/`150px`, too narrow for names like `philips-apc-z1 (120)` to render in full. Both now use `width: auto` with a `min-width`/`max-width` range so the box sizes to its content instead of clipping it.
- **Config Sources tab listed every YAML file in a config folder, not just ones for that switch** (`switch-configurator` 0.9.1): a folder holding overlays for multiple switches would leak unrelated files into `GET /switches/:id/config-sources` — harmless before, but exactly the exposure a per-file Delete action (added below) must not have. Now filtered to files that actually declare the switch's id, the same ownership check `find_switch_overlay_file` already used for View/Delete.
- **FortiSwitch connect sequence never disabled CLI pagination** (`switch-configurator` 0.10.1): found immediately on real hardware once full state parsing (above) started issuing multi-block `show` commands — `show switch interface` on a switch with many ports is long enough to trigger FortiOS's `--More--` pager, which the client never advances past, hanging until the read times out. The connect sequence now sends `config system console` / `set output standard` / `end` right after login (best-effort, like the existing "exit config mode" `end` calls) — a fix the code already had a comment anticipating, just never needed until now.
- **FortiSwitch parser trace logging** (`switch-configurator` 0.10.2): `parse_current_state` now logs each raw `show` block at `trace` level, to make discrepancies between the parser's assumptions and a given firmware's actual output visible without guessing blind — needed while tracking down a real-hardware convergence mismatch found live on `IT-02876-sw1`.

### Added
- **Full FortiSwitch state parsing** (`switch-configurator` 0.10.0): `parse_current_state` only ever extracted `management_vlan` — everything else (VLANs, ports, mirrors, SNMP) was left empty, which meant the "parsed state is completely empty but desired config is not" safety check fired on every single reconcile for every FortiSwitch, forever, blocking any real apply. Root-caused on `IT-02876-sw1` (a FortiSwitch 124F-FPOE that had never once converged). Now parses:
  - VLANs from `show switch vlan` (the VLAN database — the only place a VLAN with no Layer-3 interface still shows up) plus `show system interface` for ip_config/description on VLANs that have one. FortiSwitch never persists a VLAN's *name* to the device at all (`generate_vlan_commands` never writes one), so a VLAN's name always comes from the desired config, not from hardware.
  - Ports from `show switch interface` (VLAN membership, description) merged with `show switch physical-port` (link status, PoE, speed).
  - Port mirrors from `show switch mirror`, SNMP communities/trap receivers/enabled traps from `show system snmp community`.
  - All four block parsers follow the same nested `config .../edit/set/next/end` walk `parse_management_vlan` already used for `show system interface` — FortiOS's `show` output for a config path mirrors the syntax used to write it.
- **Overlay and main-config View pages show structured tables, not just raw YAML** (`switch-configurator-ui` 0.7.0): `View` on any config source dumped its raw text with no other option — useful to double-check exact file contents, but a poor match for actually reading what a file declares once it has more than a couple of VLANs or ports. Both pages now parse the file and render the same VLANs/Ports/Port Mirrors/SNMP tables the Edit flow already uses (main config renders one such block per switch it declares), with the raw YAML still available in a collapsed `<details>` section for exactly the cases it's genuinely needed. Falls back to raw-only if the file doesn't parse.
- **VLAN-by-name in the web UI** (`switch-configurator-ui` 0.2.0): the port editor's untagged VLAN field is now a dropdown of the switch's own VLAN names, and tagged VLANs is a proper multi-select, replacing the old numeric spinbutton and comma-separated text field. Saving now persists VLANs by name (`vlan: "users"`) rather than by numeric id whenever the switch has a name for that VLAN, reusing the existing `VlanRef` name-or-id serialization on the save-overlay path (`switch-configurator` 0.6.0).
- **Resolve broken overlay configs from the web UI, not just view or delete them** (`switch-configurator` 0.7.0, `switch-configurator-ui` 0.3.0):
  - The ambiguous-VLAN-name validation error now names which file each colliding id came from (e.g. `VLAN 10 (from switch-config.yaml) and VLAN 99 (from overlay.yaml)`), instead of just the two ids — `merge_single_switch` already tracked this during merge and previously discarded it.
  - The main config is now viewable (read-only) from the dashboard, so the file the error names can actually be inspected — it remains impossible to edit or delete from the UI.
  - A `PUT /switches/:id/overlay/:filename` endpoint (and matching edit form) let a broken overlay's raw YAML be edited and re-validated directly in the browser. Superseded in 0.8.0/0.4.0 below by a structured editor — see that entry.
- **Structured, cross-file-aware editing for broken overlay configs, replacing raw-text editing** (`switch-configurator` 0.8.0, `switch-configurator-ui` 0.4.0): the previous release's raw-textarea edit endpoint let you break YAML syntax, typo an id, or pick a new id that collides with the *other* file without ever seeing it — no better than SSH-editing blind. Now:
  - The merge itself doesn't fail on an ambiguous VLAN name — both colliding rows survive as ordinary entries; only the later uniqueness check rejects them. `SwitchValidationFailure` now carries this merged-but-unvalidated preview, plus which source file each VLAN/port id came from, instead of discarding it after building the error string.
  - New `GET /switches/:id/merge-preview` endpoint exposes this (falling back to the normal valid-switch view when there's no failure), so a broken switch can reach the same drafting flow a healthy one uses.
  - The VLAN/port editor now renders a broken switch's rows tagged by origin: a row from the main config is read-only (enforced server-side, not just hidden in the UI) with a "also used by id X (file)" note on both sides of a collision; the disputed overlay's row stays fully editable through the existing rename/renumber/remove controls.
  - Saving now excludes main-config-sourced rows from what gets written to the overlay (they were never the overlay's to declare), and defaults the save dialog's filename to the disputed overlay's own file, so "Save" fixes the actual broken file instead of creating an inert duplicate.
  - The raw-text `PUT /switches/:id/overlay/:filename` endpoint, its edit form, and the line-highlighting built for it are removed — the structured editor replaces them; `View` (read-only) and `Delete` are unchanged.
- **Config Sources tab gains View/Edit/Delete per source file** (`switch-configurator-ui` 0.6.0): previously it only listed each contributing file's priority and type, with no way to act on one — the same View/Edit/Delete already available from the dashboard's broken-switch banner, now available for any switch's healthy source list too. The main config row stays view-only (via the existing `/main-config/view`), consistent with everywhere else it's never editable or deletable from the UI.

### Fixed
- **Saving an overlay silently stripped `model`/`management_ip`/`credentials`** (`switch-configurator-ui` 0.4.1): `save_overlay`'s payload never included identity fields at all — only `hostname`, VLANs, ports, mirrors and SNMP. Harmless when a switch's identity lives in the main config (main always wins that part of the merge regardless), but for a switch whose identity lives entirely in the overlay being edited — a supported config shape — every save quietly turned it into a "missing required fields" failure. Found via live verification of the structured editor above, on a switch with no main-config counterpart. Identity fields are now always included in the save payload.
- **Main config file no longer offered for deletion on the dashboard** (`switch-configurator-ui`): the validation-failure banner read the main config's path from the wrong `/api/status` JSON key, so the filter meant to exclude it from the "View/Delete overlay" list silently never matched — every validation failure offered to delete the main config alongside genuine overlays. The main config now only appears informationally under "Config files."
- **Overlay `View`/`Delete` always used the first configured folder, and ignored the switch id** (`switch-configurator`): `get_first_config_folder` picked whichever config folder was configured first regardless of where the requested file actually lived, and the switch id in the URL was unused — a same-named overlay for a different switch could be served or deleted by mistake. The lookup now searches every configured folder and confirms the file actually declares the requested switch's id.
- **Removing a colliding VLAN row in the structured editor could silently orphan the ports that used it** (`switch-configurator` 0.9.0, `switch-configurator-ui` 0.5.0): a real production overlay declares no VLANs of its own for most ports, relying entirely on the main config's — a supported shape. `validate_overlay_config` skipped its port-VLAN reference check *entirely* whenever the submitted overlay had zero VLANs, trusting "must come from elsewhere" without checking anywhere. Resolving a real VLAN-name collision by removing the disputed duplicate rows (the obvious, correct way to fix it) emptied the overlay's VLAN list and let the save through with 13 ports pointing at ids that existed nowhere at all — a worse state than the original collision. Two changes close this:
  - `known_external_vlan_ids()` re-reads every *other* config source for a switch (main config, other overlays) at save time, so `validate_overlay_config` can now run its port-reference check unconditionally instead of skipping it — a port is valid if its VLAN is declared by this overlay, by another known source, or implicit on the model.
  - The VLAN editor's "Remove" action now refuses to remove a VLAN still referenced by a port in the draft, naming which port(s) and linking straight to the Ports tab to reassign them first — the same server-side-enforced boundary already used for main-config rows.

## [0.5.0] - 2026-08-06

### Added
- **VLAN references by name**: A port's untagged `vlan` and its `tagged_vlans` may now reference a VLAN by name (string) in addition to numeric id, e.g. `vlan: "Users"` or `tagged_vlans: ["Voice", 40]`. Matching is type-strict (bare int = id, quoted string = name), case-sensitive, and names are resolved to ids at load time. Unknown untagged names are a hard error; unknown tagged names are dropped-with-warning (lenient) or an error (strict); duplicate VLAN names are rejected as ambiguous. See `examples/vlan-by-name.yaml`.

### Changed
- Migrated both workspace crates to the Rust 2024 edition (`rust-version = "1.85"`).


## [0.3.21] - 2026-03-17

### Added
- **Hardware Model Verification**: The Aruba parser now extracts the hardware product number from the running config header (e.g., `; J9779A Configuration Editor;`) and compares it against known product numbers for the configured model. A warning is emitted on mismatch and surfaced via the `/api/status` and `/switches/{id}/config` REST API endpoints.
- **`product_numbers()` method on `SwitchModel`**: Returns known hardware product numbers for each model, used for model verification.
- **`warnings` field on `SwitchState`**: Collects warnings during state parsing (e.g., model mismatch).
- **`warnings` field on `SwitchStatus`**: Persists warnings in the status tracker, visible in the `/api/status` response.

## [0.3.20] - 2026-03-17

### Added
- **SSH/Serial Connection Retry Logic**: Added automatic retry capability for connection failures. Switches will now retry connecting up to `max_retries` times (default: 3) with a 5-second delay between attempts. This addresses issues where temporary network issues cause configuration to fail. The retry logic applies to both SSH and Serial connections.
- **Configuration Summary Logging**: Added consistent summary logging to the file watcher flow. When configuration is applied (either on startup or via file watcher), the logs now include a summary showing success/failure counts, matching the format used in one-off mode.

### Fixed
- **Aruba PoE Parser (Critical)**: Fixed `poe-allocate-by class` incorrectly overriding `no power-over-ethernet` in the running config parser. On Aruba switches, `poe-allocate-by` is an allocation method present on all PoE-capable ports regardless of whether PoE is enabled. The parser now ignores it when determining PoE state, preventing an infinite reconfiguration loop where PoE was disabled and re-enabled every cycle.
- **Aruba 2530 Mirror Command**: Fixed `monitor all both mirror 1` being sent inside interface context on Aruba 2530/2540 models, which returned `Invalid input: all`. These models use legacy `monitor` (no parameters) syntax. The command generator now checks `uses_legacy_mirror_syntax()` to select the correct syntax per model.
- **Serial Output Truncation (Critical)**: Fixed a bug where `show running-config` via serial connection returned only a few lines instead of the full configuration. Root cause was three-fold:
  1. **False-positive prompt detection**: The prompt regex could match lines within config output that resembled switch prompts (e.g., hostname references). Added confirmation wait: after detecting a potential prompt, the client now waits 500ms to verify no more data arrives before accepting the match.
  2. **Incomplete buffer clearing**: `clear_buffer()` only read a single 1024-byte chunk, which could leave stale data from a previous command in the serial buffer. Now drains all pending data in a loop.
  3. **Overly permissive end-of-output prompt regex**: Removed the unanchored `end_prompt_pattern` that could match prompt-like text anywhere in a line. Prompt detection now strictly matches only the last non-empty line.
- **SSH Prompt Detection**: Applied the same confirmation-wait pattern to the SSH client's `wait_for_prompt()`, preventing similar false-positive issues on SSH connections.
- **Serial `connect_with_retry` panic**: Fixed a panic when `max_retries=0` was passed (now enforces minimum of 1 attempt).

### Changed
- **Documentation References**: Updated all references from `CLAUDE.md` to `AGENTS.md` to reflect the correct AI assistant guidance file for OpenCode.
- **Serial Command Timeout**: Increased timeout for `show running-config` from 30s to 60s on serial connections, accommodating large configurations on slower serial links.
- **Prompt Detection Regex**: Tightened the switch prompt regex to require at least 2 word characters (`[\w-]{2,}`) before `#` or `>`, reducing false positives from single-character matches.

## [0.1.0] - 2025-11-25

### Added
- **Multi-Vendor Support**: Complete implementations for Aruba, Cisco, and FortiSwitch vendors
- **State-Aware Configuration**: Parses current switch state and only applies necessary changes
- **Idempotent Operations**: Safe to run multiple times without side effects
- **Connection Types**: SSH (password/key), Serial console, and SSH Jump Hosts support
- **Multi-Config Merging**: Modular YAML configuration with priority-based merging
- **REST API**: Full programmatic configuration management
- **File Watching**: Automatic config reload on file changes
- **Port Mirroring**: SPAN/mirror session configuration
- **VLAN Management**: Layer 2 and Layer 3 VLAN support with IP configuration
- **SNMP Configuration**: Communities, traps, and trap receivers
- **Comprehensive Testing**: 419+ tests including unit tests, integration tests, and hardware validation

### Fixed
- **Port Mirroring**: Fixed command generation for multiple source ports
- **Port Name Cleanup**: Fixed port name/description removal when not in config
- **Error Handling**: Enhanced YAML parsing errors with field paths and line numbers
- **Serial Connection**: Improved prompt detection and login handling

### Documentation
- Complete API reference documentation
- CLI reference documentation
- Configuration guide with examples
- NixOS deployment guide
- Hardware test reports for all supported vendors
