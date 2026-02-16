# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **SSH/Serial Connection Retry Logic**: Added automatic retry capability for connection failures. Switches will now retry connecting up to `max_retries` times (default: 3) with a 5-second delay between attempts. This addresses issues where temporary network issues cause configuration to fail. The retry logic applies to both SSH and Serial connections.
- **Configuration Summary Logging**: Added consistent summary logging to the file watcher flow. When configuration is applied (either on startup or via file watcher), the logs now include a summary showing success/failure counts, matching the format used in one-off mode.

### Changed
- **Documentation References**: Updated all references from `CLAUDE.md` to `AGENTS.md` to reflect the correct AI assistant guidance file for OpenCode.

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
