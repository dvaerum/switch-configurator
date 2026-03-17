# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
