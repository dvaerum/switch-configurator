# Switch Configurator - Project Structure

**Last Updated**: 2026-01-20
**Total Files**: 194 (excluding build artifacts)

This document provides a comprehensive overview of the project's file structure, organization, and key components.

---

## Visual Overview

```
switch-configurator/
├── 📦 BUILD & PACKAGE
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── flake.nix
│   ├── flake.lock
│   ├── package.nix
│   └── nixos-module.nix
│
├── 📚 DOCUMENTATION
│   ├── README.md
│   ├── CLAUDE.md
│   ├── LICENSE
│   └── TODO.md
│
├── 📁 src/ (RUST SOURCE CODE)
│   ├── Core: models.rs, config.rs, status.rs
│   ├── diff/ - State comparison
│   ├── api/ - REST API server
│   ├── ssh/ - Connection layer (SSH + Serial)
│   ├── vendors/ - Switch implementations
│   ├── validation/ - Post-config validation
│   └── watcher/ - File watching
│
├── 📁 docs/ - Documentation (30+ files)
│   ├── guides/ - User guides
│   ├── reference/ - API/CLI reference
│   ├── deployment/ - Deployment guides
│   │   └── examples/ - NixOS example configs
│   ├── development/ - Internal docs
│   ├── testing/ - Test documentation
│   └── sessions/ - Session notes
│
├── 📁 examples/ - Example configurations (15 files)
├── 📁 tests/ - Automated tests (60+ fixtures)
└── 📁 extra-config/ - Modular config
```

---

## 📁 Source Code Structure (src/)

### Entry Points
- **main.rs** - Application entry point with CLI parsing
- **lib.rs** - Library root, public API exports

### Core Modules

#### models.rs (1,248 lines)
**Purpose**: All data structures and types

**Key Types**:
- `SwitchModel` - Enum of supported switch models
  - Aruba: 2530-24G, 2530-8G, 2530-48G, 2540-24G, 2540-48G, 2930F
  - Cisco: Catalyst 9300-24P UPoE
  - FortiSwitch: 124F-FPOE
- `SwitchConfig` - Complete switch configuration
- `Port` - Port configuration (mode, VLAN, PoE, MAC notify, speed)
- `Vlan` - VLAN definition with IP config (dhcp/static/none)
- `PortMirror` - SPAN/mirror session configuration
- `SnmpConfig` - SNMP communities, trap receivers, enabled traps
- `Credentials` - Connection credentials with jump host support
- `JumpHost` - SSH jump host/bastion configuration
- `ValidationConfig` - Post-config validation tests

#### config.rs (582 lines)
**Purpose**: Configuration management and multi-config merging

**Key Features**:
- Load/save YAML configuration files
- Multi-config merge system with priority-based merging
- Port range expansion (e.g., "1-10,15,20-24")
- Component replacement strategy (VLANs, ports, mirrors)
- ConfigStore (Arc<RwLock<AppConfig>>) for thread-safe access
- RuntimeConfig for execution modes (debug, dry-run, one-off)

**Architecture**:
- Priority system: 0-10 (main), 11-9999 (folders)
- Identity fields (hostname, model, management_ip, credentials)
- Optional fields in folder configs with validation
- Alphabetical folder file ordering

#### status.rs
**Purpose**: Application status tracking and reporting

### State Management

#### diff/mod.rs
**Purpose**: State comparison and diff computation

**Key Function**: `compute_diff(current: &SwitchState, desired: &SwitchConfig, enforce_port_config: bool) -> StateDiff`

**Compares**:
- VLANs (add, update, remove)
- Ports (configure changed ports)
- Port mirrors (add, update, remove sessions)
- SNMP configuration (communities, trap receivers, enabled traps)

**Returns**: Only the changes needed (idempotent operations)

### REST API (api/)

**Framework**: Axum (async HTTP framework)

#### Endpoints
- `GET /health` - Health check
- `GET /switches` - List configured switches
- `POST /switches/{hostname}/apply` - Apply configuration to switch
- `GET /switches/{hostname}/config` - Get running config from switch
- `POST /config/reload` - Reload configuration from file

**Files**:
- **mod.rs** - Module exports
- **server.rs** - HTTP server setup
- **handlers.rs** - Request handlers
- **tests.rs** - API unit tests

### Connection Layer (ssh/)

**Architecture**: Unified interface for SSH and Serial connections

#### ssh/client.rs (624 lines)
**SSH Client with PTY Support**

**Key Features**:
- Interactive shell (PTY) with prompt detection
- Jump host integration (single or multi-hop)
- Legacy KEX algorithm support (DH_G14_SHA1)
- ANSI escape sequence cleaning
- "-- MORE --" pagination handling
- Authentication via password or SSH key

**Methods**:
- `connect_with_credentials()` - Connect directly or via jump chain
- `open_shell()` - Request PTY and shell
- `execute_command()` - Execute with prompt detection
- `wait_for_prompt()` - Regex-based prompt detection
- `clear_buffer()` - Clean ANSI escapes

#### ssh/serial.rs (453 lines)
**Serial Console Client**

**Key Features**:
- Direct serial port access (tokio-serial)
- Automatic login detection
- Prompt detection (shared regex with SSH)
- Supports debug and dry-run modes

**Methods**:
- `connect()` - Open serial port connection
- `login()` - Detect state and authenticate if needed
- `execute_command()` - Execute with prompt wait
- `wait_for_prompt()` - Same pattern as SSH client

#### ssh/connection.rs
**ConnectionClient Enum**

Unified interface abstracting SSH vs Serial:
```rust
pub enum ConnectionClient {
    Ssh(SshClient),
    Serial(SerialClient),
}
```

All vendors work with this enum transparently.

#### ssh/jump_host.rs (320 lines) ✨ NEW
**Jump Host Session Management**

**Key Features**:
- TCP port forwarding (localhost → jump host → target)
- Authentication fallback (SSH key → password)
- Bidirectional data proxy (tokio::select!)
- Arc<Mutex<Handle>> for async task sharing

**Methods**:
- `connect()` - Establish jump host connection
- `create_port_forward()` - Set up TCP proxy on random port
- `authenticate()` - Try SSH key first, fallback to password

#### ssh/jump_host_parser.rs (196 lines) ✨ NEW
**Jump Host Configuration Parsing**

**Parses**: `user@host:port` format

**Username Precedence**:
1. Explicit `username` field
2. Embedded in `host` field (user@host)
3. Current system user ($USER)
4. Target switch username

**Methods**:
- `parse_host_string()` - Extract user, host, port
- `resolve_jump_host()` - Apply precedence rules
- `validate_jump_host_chain()` - Validate chain consistency

#### ssh/jump_chain.rs (167 lines) ✨ NEW
**Multi-Hop Jump Chain Management**

**Purpose**: Manage chains like `local → jump1 → jump2 → switch`

**Methods**:
- `establish()` - Connect all hops sequentially
- `get_final_endpoint()` - Return localhost:port for target connection
- `disconnect()` - Close all sessions in reverse order

#### ssh/jump_host_tests.rs
**Unit tests for jump host functionality**

### Vendor Implementations (vendors/)

**Architecture**: Trait-based vendor abstraction

#### vendors/traits.rs
**SwitchVendor Trait**

All vendors must implement:
```rust
pub trait SwitchVendor {
    // Connection
    async fn connect(&mut self) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;

    // State parsing
    async fn parse_current_state(&mut self) -> Result<SwitchState>;

    // Configuration application
    async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>>;
    async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>>;

    // Specific configuration
    async fn configure_vlans(&mut self, vlans: &[Vlan]) -> Result<()>;
    async fn configure_ports(&mut self, ports: &[Port]) -> Result<()>;
    async fn configure_port_mirrors(&mut self, mirrors: &[PortMirror]) -> Result<()>;

    // Utilities
    async fn save_configuration(&mut self) -> Result<()>;
    async fn get_running_config(&mut self) -> Result<String>;
    async fn validate_configuration(&mut self) -> Result<bool>;
}
```

#### vendors/aruba.rs (1,684 lines) ✨ FIXED
**Aruba Switch Implementation**

**Supported Models**: 2530-24G, 2530-8G, 2530-48G, 2540-24G, 2540-48G, 2930F

**Command Generation**:
- `generate_vlan_commands()` - VLAN configuration
- `generate_port_commands()` - Port configuration
  - **INCLUDES**: monitor commands for port mirroring (FIX!)
- `generate_mirror_commands()` - Only global mirror destination
- `generate_snmp_commands()` - SNMP configuration

**State Parsing**:
- `parse_current_state()` - Parse "show running-config" output

**Pagination**: Executes `no page` after connection

**Port Mirroring Fix**:
- Monitor commands now generated within port interface blocks
- Prevents Aruba from clearing settings on re-entry
- Hardware validated on test switch

**Tests**: 45 unit tests covering all functionality

#### vendors/cisco.rs (1,445 lines)
**Cisco Catalyst Implementation**

**Supported Models**: Catalyst 9300-24P UPoE

**Command Syntax**: IOS-XE
- `interface GigabitEthernet1/0/X` format
- `switchport mode access/trunk`
- `spanning-tree portfast`

**Pagination**: `terminal length 0`

**Tests**: 24 unit tests

#### vendors/fortiswitch.rs (1,033 lines)
**FortiSwitch Implementation**

**Supported Models**: 124F-FPOE

**Command Syntax**: FortiOS
- `config switch interface` blocks
- `set allowed-vlans X,Y,Z`

**Pagination**:
```
config system console
  set output standard
end
```

**Tests**: FortiSwitch-specific unit tests

#### vendors/mod.rs
**Vendor Factory**

**Key Function**:
```rust
pub fn create_vendor_with_runtime(
    config: &SwitchConfig,
    runtime_config: RuntimeConfig,
    enforce_port_config: bool,
) -> Result<Box<dyn SwitchVendor>>
```

Returns appropriate vendor implementation based on `SwitchModel`.

#### vendors/tests.rs
**Vendor-specific unit tests**

### Validation System (validation/)

#### validation/mod.rs (521 lines)
**Post-Configuration Validation**

**Test Types**:
- `ping` - ICMP connectivity test
- `http` - HTTP endpoint check
- `tcp_port_check` - Port accessibility test
- Custom test implementations

**Configuration**:
- `timeout` - Total test timeout
- `on_failure` - Action (warn, rollback)
- `rollback_method` - How to rollback (running_config, checkpoint)

**ValidationRunner**:
- Runs all tests sequentially
- Collects results
- Triggers rollback if needed

#### validation/tests.rs
**Unit tests for validation system**

### File Watching (watcher/)

#### watcher/mod.rs
**Configuration File Watcher**

**Uses**: notify crate
**Watches**: Config file for changes
**Action**: Automatically applies changes to all switches

---

## 📁 Documentation (docs/)

### Structure
```
docs/
├── README.md - Documentation index
├── DOCUMENTATION-STATUS.md - Completeness tracking
├── known-issues.md - Known bugs
├── PROJECT-STRUCTURE.md - This file
│
├── guides/ - User-facing documentation
│   ├── getting-started.md
│   ├── configuration.md
│   └── troubleshooting.md
│
├── reference/
│   └── cli.md - CLI reference
│
├── deployment/
│   ├── nixos.md - NixOS deployment
│   └── examples/ - NixOS example configurations
│       ├── nixos-module.nix - Module usage example
│       └── overlay-usage.nix - Overlay usage example
│
├── development/ - Internal architecture docs
│   ├── architecture.md - System design
│   ├── state-aware-implementation.md
│   ├── multi-config-merge-design.md
│   ├── validation-design.md
│   ├── aruba-snmp-trap-behavior.md
│   └── ... (7 more)
│
├── testing/ - Test documentation
│   ├── MANUAL-TESTING-PLAN.md
│   ├── aruba-port-mirroring-investigation.md ✨ NEW
│   └── cisco/
│       ├── README.md
│       ├── hardware-testing-complete.md
│       └── ... (3 more)
│
└── sessions/ - Session notes
    ├── README.md
    └── 2025-11-25-*.md (6 files)
```

### Key Documents

**User Documentation**:
- `guides/getting-started.md` - Installation, first run
- `guides/configuration.md` - YAML config guide
- `guides/troubleshooting.md` - Common issues

**Development**:
- `CLAUDE.md` - AI assistant context (most comprehensive)
- `development/architecture.md` - System design patterns
- `development/state-aware-implementation.md` - State-aware config
- `development/multi-config-merge-design.md` - Multi-config algorithm

**Testing**:
- `testing/aruba-port-mirroring-investigation.md` - Port mirror bug fix
- `testing/cisco/hardware-testing-complete.md` - Hardware validation

---

## 📁 Examples (examples/)

### Complete Example Configurations

**Connection Types**:
- `aruba-ssh.yaml` - SSH connection examples
- `aruba-serial.yaml` - Serial console configuration
- `ssh-with-jump-hosts.yaml` ✨ NEW - Jump host examples (153 lines)

**Feature Examples**:
- `vlan-ip-configs.yaml` - All IP modes (dhcp, static, none)
- `port-mirroring.yaml` - SPAN/mirror configuration
- `port-ranges.yaml` - Port range syntax (e.g., "1-10,15")
- `snmp-trap-example.yaml` - SNMP configuration
- `per-port-mac-notify-control.yaml` - Per-port MAC notify
- `validation-example.yaml` - Validation tests

**Multi-Config**:
- `config.example.yaml` - Complete reference
- `multi-vendor.yaml` - Mixed vendor environment

**NixOS Deployment Examples** (moved to docs/deployment/examples/):
- See `docs/deployment/examples/` for NixOS module and overlay usage examples

---

## 📁 Tests (tests/)

### Integration Tests

#### multi_config_tests.rs (2,131 lines)
**25 comprehensive multi-config test scenarios**

**Test Categories**:
1. Basic merge functionality
2. Priority-based overrides
3. Identity field conflicts
4. Credential merging
5. Port range expansion
6. SNMP sub-component merging
7. Validation config merging
8. Settings object replacement

### Test Fixtures (60+ files)

**Organization**:
```
tests/fixtures/
├── invalid-configs/ (4 files)
│   ├── missing-credentials.yaml
│   ├── missing-management-ip.yaml
│   ├── invalid-port-mode.yaml
│   └── source-ports-type-mismatch.yaml
│
└── multi-config/ (57 YAML files)
    ├── basic/ - Basic merge
    ├── conflicts/ - Conflict detection
    ├── credentials/ - Credential merging
    ├── mirrors/ - Port mirror merging
    ├── priority/ - Priority system
    ├── snmp-merge/ - SNMP merging
    └── ... (14 more scenarios)
```

### Manual Tests

**Structure**:
```
tests/manual/
├── README.md - Test documentation
├── run-tests.sh - Test runner
└── configs/
    ├── task1-mirroring/ - Port mirroring tests
    ├── task2-port-names/ - Port name changes
    ├── task3-multi-config/ - Multi-config tests
    ├── task4-validation/ - Validation tests
    └── task5-errors/ - Error handling tests
```

**20+ manual test scenarios** for exploratory testing.

---

## 📁 Production Configuration

### extra-config/
**Modular production configuration folder**

- `ports.yaml` - Port configurations
  - Defines 27 ports (expanded from 9 port ranges)
  - Includes mirror session: ports 33-36 → port 42
  - Uses port range syntax: "33,34,35,36" → 4 individual ports

**Usage**:
```bash
cargo run -- --config-file main-config.yaml --config-folder extra-config
```

**Merge Priority**:
- main-config.yaml: Priority 50
- extra-config/ports.yaml: Priority 100 (default)

---

## 📊 Project Metrics

### Code Statistics
- **Total Files**: 194
- **Source Files**: 26 Rust modules
- **Total Lines of Rust**: ~8,000+ lines

**Largest Modules**:
| Module | Lines | Purpose |
|--------|-------|---------|
| vendors/aruba.rs | 1,684 | Aruba implementation |
| vendors/cisco.rs | 1,445 | Cisco implementation |
| models.rs | 1,248 | Data structures |
| vendors/fortiswitch.rs | 1,033 | FortiSwitch implementation |
| ssh/client.rs | 624 | SSH client with PTY |
| config.rs | 582 | Config management |
| validation/mod.rs | 521 | Validation system |

### Test Coverage
- **Total Automated Tests**: 419 (100% passing)
  - 394 unit tests (vendors, API, validation, etc.)
  - 25 integration tests (multi-config)
- **Manual Test Scenarios**: 20+

### Documentation
- **User Guides**: 3
- **Development Docs**: 11
- **Testing Docs**: 6
- **Session Notes**: 6
- **Examples**: 15 complete configs
- **Test Fixtures**: 60+ YAML files

---

## ✨ New Features (This Session)

### 1. SSH Jump Host Support (683 lines)
**Files Added**:
- `src/ssh/jump_host.rs` (320 lines)
- `src/ssh/jump_host_parser.rs` (196 lines)
- `src/ssh/jump_chain.rs` (167 lines)
- `examples/ssh-with-jump-hosts.yaml` (153 lines)

**Features**:
- Single and multi-hop chains
- user@host:port format parsing
- Authentication fallback (SSH key → password)
- Username precedence rules
- TCP port forwarding

### 2. Interactive Shell (PTY) Support
**Modified**:
- `src/ssh/client.rs` - Added PTY support
- `src/vendors/aruba.rs` - Pagination at connection
- `src/vendors/cisco.rs` - Pagination at connection
- `src/vendors/fortiswitch.rs` - Pagination at connection

**Features**:
- Interactive shell (PTY) for all vendors
- Unified prompt detection (reused from serial)
- ANSI escape sequence cleaning
- "-- MORE --" pagination handling

### 3. Port Mirroring Fix
**Modified**:
- `src/vendors/aruba.rs` - Include monitor commands in port blocks

**Documentation**:
- `docs/testing/aruba-port-mirroring-investigation.md` - Complete bug report

**Testing**:
- `tests/scripts/test-check-mirrors.sh` - Hardware verification script
- `tests/scripts/test_port_range_cli.sh` - Port range testing script
- `tests/configs/test-mirror-check.yaml` - Test configuration

**Result**: Hardware validated on test switch, all 4 source ports retain monitor commands

---

## 🔧 Build & Deployment

### Nix Flake
**Files**:
- `flake.nix` - Flake definition
- `flake.lock` - Locked inputs
- `package.nix` - Package build definition
- `nixos-module.nix` - Systemd service module

**Usage**:
```bash
nix develop        # Enter dev shell
nix build          # Build package
nix run            # Run application
```

### Cargo/Rust
**Files**:
- `Cargo.toml` - Project manifest
- `Cargo.lock` - Locked dependencies

**Usage**:
```bash
cargo build --release
cargo test
cargo run -- --config-file config.yaml
```

### NixOS Module
Deploy as systemd service with automatic restarts, security hardening, and configurable options.

See: `docs/deployment/nixos.md`

---

## 🎯 Architectural Patterns

### 1. Trait-Based Vendor Abstraction
All vendors implement `SwitchVendor` trait → Easy to add new vendors

### 2. State-Aware Configuration
Read current state → Compute diff → Apply only changes (idempotent)

### 3. Multi-Config Merge System
Priority-based merging with component replacement strategy

### 4. Connection Abstraction
Unified `ConnectionClient` enum for SSH and Serial

### 5. Interactive Shell Usage
All vendors use PTY with prompt detection (not exec mode)

### 6. Validation Framework
Optional post-config validation with rollback support

---

## 📝 Version Control

### Recent Commits
```
dbc16a9 - Document port mirroring bug fix and add hardware validation tools
1fb8ce6 - Implement SSH jump host support and fix interactive shell handling
5db17a4 - Fix CLI argument inconsistency: replace --config with --config-file
5eece5b - Implement optional identity fields in multi-config merge system
```

### Git Structure
- **Master branch**: Main development branch
- **.gitignore**: Excludes target/, .direnv/, logs

---

## 📚 Learning Resources

**For New Contributors**:
1. Start with `README.md`
2. Read `CLAUDE.md` for comprehensive context
3. Review `docs/development/architecture.md`
4. Explore `examples/` for configuration patterns
5. Run tests: `cargo test`

**For Users**:
1. `docs/guides/getting-started.md`
2. `examples/` directory for templates
3. `docs/guides/troubleshooting.md` for issues

**For Maintainers**:
1. `CLAUDE.md` - Complete system context
2. `docs/development/` - Architecture and design
3. `docs/testing/` - Test documentation
4. `docs/sessions/` - Development history

---

## 🔗 Related Files

- **Main Documentation**: `README.md`
- **AI Context**: `CLAUDE.md`
- **Architecture**: `docs/development/architecture.md`
- **Test Coverage**: `TEST_COVERAGE_ANALYSIS.md`
- **Todo List**: `TODO.md`

---

**Maintained By**: Claude Code
**Repository**: github.com/yourusername/switch-configurator
**License**: MIT
