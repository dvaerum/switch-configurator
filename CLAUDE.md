# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Switch Configurator is a Rust-based service that automates network switch configuration across multiple vendors (Aruba, Cisco, FortiSwitch). The service watches a YAML configuration file for changes and automatically applies them to switches via SSH or serial connection. It also provides a REST API for programmatic configuration management.

## Supported Switch Models

- **Aruba**: 2530-24G PoE+, 2530-8G PoE+, 2530-48G-2SFP+ (J9855A), 2540-24G, 2540-48G-4SFP+ (JL355A), 2930F
- **FortiSwitch**: 124F-FPOE
- **Cisco**: Catalyst 9300-24P UPoE

Corresponding `SwitchModel` enum values: `Aruba2530_24G_POE`, `Aruba2530_8G_POE`, `Aruba2530_48G_2SFP`, `Aruba2540_24G`, `Aruba2540_48G_4SFP`, `Aruba2930F`, `Fortiswitch124F_FPOE`, `CiscoCatalyst9300_24P_UPOE`

### PoE Support by Model

**PoE-Capable Models** (will generate PoE commands):
- `Aruba2530_24G_POE`, `Aruba2530_8G_POE`, `Aruba2930F`
- `Fortiswitch124F_FPOE`
- `CiscoCatalyst9300_24P_UPOE`

**Non-PoE Models** (PoE commands are skipped):
- `Aruba2530_48G_2SFP`, `Aruba2540_24G`, `Aruba2540_48G_4SFP`

Use `SwitchModel::supports_poe()` to check programmatically. See `docs/testing/aruba-serial-parsing-fixes.md` for details.

## Build and Run Commands

### Nix Flake (Recommended)

```bash
nix develop        # Enter dev shell with Rust toolchain, rust-analyzer, cargo tools
nix build          # Build the package
nix run            # Run the application
nix run . -- --config-file /path/to/config.yaml --port 9000
```

### Building with Cargo

```bash
cargo build                          # Debug build
cargo build --release                # Release build (optimized)
cargo build --profile dev-fast       # Fast dev build with some optimization
cargo build --profile release-fast   # Fast release build (thin LTO)
cargo check                          # Check without building
```

**Build Profiles** (defined in Cargo.toml):
- `dev`: Default, fast compile, no optimization
- `dev-fast`: Balanced - opt-level=1, max parallel codegen
- `release-fast`: Faster than full release - opt-level=2, thin LTO
- `release`: Slowest build, fastest binary - opt-level=3, full LTO

### Running with Cargo

#### Service Mode (Default)
Runs continuously with API server and file watching:

```bash
# Run with default settings (config.yaml, port 4002)
cargo run

# Run with custom configuration
cargo run -- --config-file /path/to/config.yaml --port 9000 --log-level debug

# Disable file watching
cargo run -- --watch false

# Apply configuration on startup (before starting API server)
cargo run -- --apply-on-startup

# Run in release mode
cargo run --release
```

#### One-Off Mode
Apply configuration once and exit (no API server or file watching):

```bash
# Apply configuration to all switches once
cargo run -- --one-off

# Apply to specific switch only
cargo run -- --one-off --switch aruba-switch-01

# Combine with other options
cargo run -- --one-off --config-file production.yaml --log-level debug
```

#### Debug Mode
Interactive mode that prompts before executing each SSH command:

```bash
# Debug mode with interactive prompts
cargo run -- --one-off --debug

# Each command will prompt:
#   Execute this command? [Y/n/q]:
#   - Y/yes/Enter: Execute the command
#   - n/no: Skip this command
#   - q/quit: Abort entirely
```

#### Dry-Run Mode
Preview what would be done without actually executing commands:

```bash
# Show commands that would be executed without applying
cargo run -- --one-off --dry-run

# Specific switch only
cargo run -- --one-off --dry-run --switch cisco-core-01

# Combine with debug mode
cargo run -- --one-off --dry-run --debug
```

### Testing
```bash
cargo test                                  # Run all tests (300+ total)
cargo test -- --nocapture                   # Run tests with output
cargo test <test_name>                      # Run specific test
cargo test --lib vendors::aruba::tests      # Run Aruba vendor tests
cargo test --lib vendors::cisco::tests      # Run Cisco vendor tests
cargo test --test multi_config_tests        # Run multi-config integration tests
```

### Code Quality
```bash
cargo fmt          # Format code
cargo clippy       # Run linter
cargo clippy --fix # Auto-fix linter warnings
```

## Project Structure

The project follows a clean, organized structure:

**Root Directory** (essential files only):
- Build files: Cargo.toml, flake.nix, package.nix, nixos-module.nix
- Core docs: README.md, CLAUDE.md, LICENSE, TODO.md
- Production configs: config.yaml, main-config.yaml
- Utility: check-config.sh

**Key Directories**:
- `src/` - Rust source code (28 modules, ~15,000 lines)
- `docs/` - Documentation (30+ files)
  - `docs/guides/` - User guides
  - `docs/development/` - Internal architecture docs
  - `docs/testing/` - Test documentation
  - `docs/PROJECT-STRUCTURE.md` - Complete file organization reference
- `tests/` - Automated tests (300+ tests, 60+ fixtures)
  - `tests/configs/` - Hardware test configurations
  - `tests/scripts/` - Test helper scripts
  - `tests/fixtures/` - Test fixtures for integration tests
  - `tests/manual/` - Manual test scenarios
- `examples/` - Example configurations (15 complete examples)
- `extra-config/` - Modular production configuration

**Test Organization**: Test files, configs, and scripts are organized in `tests/` subdirectories. Hardware test configurations are in `tests/configs/`, test scripts in `tests/scripts/`.

See `docs/PROJECT-STRUCTURE.md` for complete file inventory and organization details.

## Architecture

**Note on Line References**: Line number references (e.g., `src/file.rs:123`) are approximate and may shift as code evolves. Search by function/method names if exact lines have changed.

### Core Design Pattern

The project uses a **trait-based vendor abstraction pattern** to support multiple switch vendors. Each vendor (Aruba, Cisco, FortiSwitch) implements the `SwitchVendor` trait defined in `src/vendors/traits.rs`.

### Module Structure

- **main.rs**: Application entry point and CLI argument handling
- **lib.rs**: Library entry point, re-exports public modules
- **models.rs**: Core data structures (SwitchConfig, Port, Vlan, PortMirror, SnmpConfig, SwitchState, StateDiff, ConnectionType)
- **config.rs**: Configuration loading/saving from YAML files, RuntimeConfig for execution modes
- **config/errors.rs**: Enhanced error messages with field paths and helpful suggestions for common mistakes
- **status.rs**: Service-wide status tracking (StatusTracker, SwitchStatus, ConfigMetadata)
- **diff/**: State comparison logic that computes differences between current and desired state
- **vendors/**: Vendor-specific implementations
  - **traits.rs**: `SwitchVendor` trait that all vendors must implement
  - **aruba.rs**, **cisco.rs**, **fortiswitch.rs**: Vendor implementations
  - **tests.rs**: Vendor integration tests
  - **mod.rs**: Factory functions `create_vendor()` and `create_vendor_with_runtime()`
- **ssh/**: Connection client implementations for accessing switches
  - **client.rs**: SSH client using russh for network-based switch access with PTY support
  - **serial.rs**: Serial client using tokio-serial for direct console access
  - **connection.rs**: Unified `ConnectionClient` enum that abstracts SSH vs Serial
  - **jump_host.rs**: Jump host/bastion session management with TCP port forwarding
  - **jump_host_parser.rs**: Jump host configuration parsing (user@host:port format)
  - **jump_chain.rs**: Multi-hop jump host chain management
  - **jump_host_tests.rs**: Jump host unit tests
  - **mod.rs**: Module exports for connection clients
- **api/**: REST API server and handlers using Axum framework
  - **server.rs**: Axum server setup with CORS and routing
  - **handlers.rs**: Request handlers for all API endpoints
  - **tests.rs**: API endpoint unit tests
  - **mod.rs**: Module exports
- **watcher/**: File system watcher using `notify` crate to detect config file changes
- **validation/**: Configuration validation system for pre-deployment testing
  - **mod.rs**: ValidationConfig, FailureAction, RollbackMethod definitions
  - **tests.rs**: Validation test implementations (ping, HTTP, port checks)

### Key Architectural Components

1. **Vendor Factory Pattern** (`src/vendors/mod.rs:21`): The `create_vendor_with_runtime()` function returns a boxed trait object based on the switch model, enabling runtime polymorphism. Accepts `RuntimeConfig` for debug/dry-run modes and `enforce_port_config` parameter to control whether unconfigured ports should be reset to defaults.

2. **State-Aware Configuration System**: The service reads the current switch state before applying changes, computes a diff, and only applies necessary changes (idempotent operations).
   - **Flow**: Connect → Parse current state (`parse_current_state()`) → Compute diff (`src/diff/mod.rs:7`) → Apply only changes (`apply_diff()`) → Save config to switch's startup-config (executes `write memory` on switch, does NOT modify YAML files)
   - **Benefits**: Efficiency (minimal commands), safety (no unnecessary changes), idempotency (running twice has no effect)
   - **Port Enforcement**: When `enforce_port_config: true` in settings, ports not defined in config are reset to defaults (disabled, VLAN 1). When false (default), only configured ports are modified.
   - See `docs/development/state-aware-implementation.md` for implementation details

3. **Runtime Configuration**: `RuntimeConfig` struct controls execution modes (debug, dry-run, one-off, target switch) passed to vendor implementations.

4. **Configuration Store** (`src/config.rs:96`): Thread-safe configuration using `Arc<RwLock<AppConfig>>` for concurrent access from API handlers and file watcher.

5. **Connection Abstraction** (`src/ssh/connection.rs:5`): The `ConnectionClient` enum provides a unified interface for both SSH and Serial connections, allowing vendor implementations to work with either connection type transparently.
   - **SSH Mode** (`src/ssh/client.rs`): Network-based access using russh library with async/await
   - **Serial Mode** (`src/ssh/serial.rs`): Direct console access via serial port using tokio-serial
   - Both clients support debug mode (interactive prompts) and dry-run mode (preview commands)
   - Serial client includes intelligent prompt detection and automatic login handling

6. **File Watcher Integration** (`src/watcher/mod.rs`): Uses notify crate to watch config file and automatically applies changes to all switches.

7. **Validation System** (`src/validation/mod.rs`): Optional pre-deployment testing framework that runs connectivity and functional tests after configuration changes. Supports configurable failure actions (warn, rollback) and rollback methods (running-config, checkpoint).

### Adding a New Vendor

To add support for a new switch vendor:

1. Create new file: `src/vendors/yourvendor.rs`
2. Define struct: `pub struct YourVendorSwitch { config: SwitchConfig, runtime_config: RuntimeConfig, enforce_port_config: bool, client: Option<ConnectionClient> }`
3. Implement all `SwitchVendor` trait methods:
   - **Connection**: `connect()`, `disconnect()`
   - **State parsing**: `parse_current_state()` - parse running config into `SwitchState`
   - **Diff application**: `apply_diff()` - apply only changes from `StateDiff`
   - **Configuration**: `configure_vlans()`, `configure_ports()`, `configure_port_mirrors()`
   - **Orchestration**: `apply_configuration()` - should use `parse_current_state()` → `compute_diff()` → `apply_diff()` pattern
   - **Utilities**: `save_configuration()`, `get_running_config()`, `validate_configuration()`
4. Add vendor to `Vendor` enum in `src/models.rs:8`
5. Add model to `SwitchModel` enum in `src/models.rs:16`
6. Update factory in `src/vendors/mod.rs:17` to handle new vendor
7. Add module declaration in `src/vendors/mod.rs`

### Vendor Implementation Pattern

Each vendor implementation follows this pattern:

**Command Generation** (vendor-specific helpers):
1. `generate_vlan_commands()`: Builds vendor-specific VLAN configuration commands
2. `generate_port_commands()`: Builds port configuration commands (access/trunk mode, PoE, MAC notify, etc.)
3. `generate_mirror_commands()`: Builds port mirroring/SPAN commands
4. `generate_snmp_commands()`: Builds SNMP configuration commands (communities, trap receivers, trap types)
5. `normalize_port_id()`: Converts generic port identifiers to vendor-specific format

**State Parsing** (implements trait method):
1. `parse_current_state()`: Parses "show running-config" output into `SwitchState { vlans, ports, port_mirrors, snmp }`
   - Use simple line-by-line regex/string matching
   - Extract VLANs, port configurations, mirror sessions, and SNMP settings
   - Return structured state representation
   - **Aruba Mirror Parsing**: Handles two syntax variants:
     - Legacy (2530/2540): `mirror-port <dest>` global command
     - Newer (2930F+): `mirror <id> port <dest>` global command
     - Both syntaxes use per-interface `monitor` commands for source ports

**Configuration Application** (implements trait method):
1. `apply_configuration()`:
   ```rust
   async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError> {
       let current = self.parse_current_state().await?;
       let diff = crate::diff::compute_diff(&current, &self.config, self.enforce_port_config);
       if !diff.has_changes() {
           return Ok(vec![]);  // No changes needed
       }
       self.apply_diff(&diff).await
   }
   ```
   Note: The `enforce_port_config` parameter determines whether unconfigured ports should be reset to defaults.

2. `apply_diff()`: Apply only the changes specified in `StateDiff`
   - Add/update/remove VLANs as specified
   - Configure changed ports only
   - Add/update/remove port mirrors as specified
   - Update SNMP configuration if changed

Example port identifiers: Aruba uses "interface 1", Cisco uses "GigabitEthernet1/0/1", FortiSwitch uses "port1"

## Configuration File Format

The service uses YAML configuration (`config.yaml` or custom path). See `examples/config.example.yaml` for complete structure.

Key sections:
- `switches[]`: Array of switch configurations
- `switches[].model`: Must match `SwitchModel` enum variants (e.g., `Aruba2530_24G_POE`, `Aruba2930F`, `CiscoCatalyst9300_24P_UPOE`)
- `switches[].credentials`: Connection credentials supporting both SSH and Serial modes
  - **SSH Mode** (default): Requires `username`, `password` or `ssh_key_path`, and `port` (default: 22)
  - **Serial Mode**: Requires `connection_type: serial`, `serial_device` (e.g., `/dev/ttyUSB0`), `baud_rate` (default: 9600), `username`, and `password`
  - **Jump Hosts** (SSH Bastion Servers): Optional `jump_hosts` array for accessing switches through intermediate servers
    - Supports single or multi-hop chains: `local -> jump1 -> jump2 -> ... -> switch`
    - Each jump host supports `user@host:port` format with automatic parsing
    - Authentication priority: SSH key (tried first) → Password (fallback)
    - Username resolution: explicit `username` field → embedded in `host` → target username
    - Port resolution: explicit `port` field → embedded in `host` → default 22
    - Example single hop: `jump_hosts: [{ host: "bastion-user@bastion.company.com", ssh_key_path: "/path/to/key" }]`
    - Example multi-hop: `jump_hosts: [{ host: "jump1.com", ssh_key_path: "/key1" }, { host: "user@jump2.com:2222", password: "pass2" }]`
    - See `examples/ssh-with-jump-hosts.yaml` for detailed examples
  - Example SSH: `{ username: admin, password: secret, port: 22 }`
  - Example Serial: `{ connection_type: serial, serial_device: /dev/ttyUSB0, baud_rate: 9600, username: admin, password: secret }`
  - Example Jump Host: `{ username: admin, password: secret, jump_hosts: [{ host: "bastion.company.com", ssh_key_path: "/path/to/key" }] }`
- `switches[].vlans[]`: VLAN definitions (id, name, description, ip_config)
  - **IP Configuration**: Each VLAN can have an IP address configuration:
    - `ip_config: dhcp` - Get IP address from DHCP server
    - `ip_config: none` - No IP address (default)
    - `ip_config: { static: { address: "192.168.1.1", netmask: "255.255.255.0" } }` - Static IP
  - Example: `{ id: 10, name: management, ip_config: dhcp }`
  - See `examples/vlan-ip-configs.yaml` for detailed examples
- `switches[].ports[]`: Port configurations (mode, vlan, poe_enabled, mac_notify, speed_duplex, etc.)
  - **Port Range Syntax**: Supports concise configuration of multiple ports:
    - `"1-5"` - Range: expands to ports 1,2,3,4,5
    - `"1,3,5"` - List: expands to ports 1,3,5
    - `"1-5,7,10-12"` - Mixed: expands to ports 1,2,3,4,5,7,10,11,12
    - Vendor-specific: `"GigabitEthernet1/0/1-48"` (Cisco), `"port1-24"` (FortiSwitch)
    - Example: `port_id: "1-10"` configures ports 1 through 10 with identical settings
    - Port ranges are automatically expanded at config load time (`src/config.rs:115`)
  - **MAC Notify**: Set `mac_notify: true` to enable SNMP traps for MAC address changes on port (requires `snmp.enabled_traps: ["mac-notify"]` to be configured globally)
  - **Speed/Duplex**: Configure port speed and duplex mode
    - `auto` - Auto-negotiation (default)
    - `10-half`, `10-full` - 10 Mbps half/full duplex
    - `100-half`, `100-full` - 100 Mbps half/full duplex
    - `1000-full` - 1 Gbps full duplex
    - `10g-full` - 10 Gbps full duplex (only on switches with 10G uplinks)
    - Validated against switch model capabilities at config load time
    - Example: `speed_duplex: "100-full"` forces 100Mbps full-duplex
  - See `examples/port-ranges.yaml` and `examples/aruba-ssh.yaml` for detailed examples
- `switches[].port_mirrors[]`: Port mirroring sessions
  - `source_ports` also supports port range syntax
  - `destination_port` must be a single port (range not allowed)
  - **Aruba Port Mirror Syntax Variations**: The parser handles two different Aruba command syntaxes:
    - **Legacy syntax (2530/2540 series)**: `mirror-port <destination>` with per-interface `monitor` commands for sources
    - **Newer syntax (2930F and newer)**: `mirror <session-id> port <destination>` with per-interface `monitor` commands for sources
    - Both syntaxes are automatically detected during state parsing
    - When generating commands, the service always uses the newer `mirror 1 port <dest>` syntax
- `switches[].snmp`: SNMP configuration (optional)
  - **communities**: SNMP community strings with access levels (unrestricted, manager, operator)
  - **trap_receivers**: Hosts to receive SNMP traps (host, community, optional version)
  - **enabled_traps**: Types of traps to enable (mac-notify, link-change, authentication, stp, all)
  - **Per-Port MAC Notification**: When `mac-notify` trap is enabled globally, individual ports can control whether they send MAC change traps using `ports[].mac_notify: true/false`. Both global and per-port settings must be `true` for traps to be sent.
  - See `examples/snmp-trap-example.yaml` and `examples/per-port-mac-notify-control.yaml` for detailed examples
- `switches[].validation`: Validation tests to run after configuration (optional)
  - **enabled**: Enable/disable validation
  - **timeout**: Total timeout for all tests (e.g., "60s", "2m")
  - **on_failure**: Action when validation fails (warn, rollback)
  - **rollback_method**: How to rollback (running_config, checkpoint)
  - **tests**: List of validation tests (ping, http, port_check, etc.)
  - See `examples/validation-example.yaml` for detailed examples
- `switches[].settings`: Per-switch settings (optional, defaults applied if not specified)
  - **ssh_timeout_secs**: Timeout for SSH connections (default: 30)
  - **max_retries**: Maximum retry attempts (default: 3)
  - **enforce_port_config**: Reset unconfigured ports to defaults (default: false)

For complete configuration examples, see the `examples/` directory:
- `examples/aruba-serial.yaml` - Serial connection configuration
- `examples/aruba-ssh.yaml` - SSH connection examples
- `examples/ssh-with-jump-hosts.yaml` - Jump host/bastion server configuration
- `examples/multi-vendor.yaml` - Mixed vendor environments
- `examples/vlan-ip-configs.yaml` - All IP configuration modes
- `examples/port-mirroring.yaml` - SPAN/mirror configuration
- `examples/port-ranges.yaml` - Port range syntax examples
- `examples/snmp-trap-example.yaml` - SNMP communities and trap configuration
- `examples/validation-example.yaml` - Validation test configuration

## Multi-Config Merge System

The service supports loading and merging multiple YAML configuration files, enabling modular and reusable configurations. This is useful for:
- Separating common/baseline configs from environment-specific overrides
- Managing per-team or per-application switch configurations
- Creating reusable configuration snippets for standard setups

### Basic Usage

```bash
# Single file mode (legacy, still supported)
cargo run -- --config-file main.yaml

# Multi-config mode: main config + folder(s)
cargo run -- --config-file main.yaml --config-folder /etc/switch-configs/common
cargo run -- --config-file main.yaml --config-folder /path/to/folder1 --config-folder /path/to/folder2

# Debug merged result
cargo run -- --config-file main.yaml --config-folder ./configs --show-merged-config
```

### Switch Identity and the `id` Field

**BREAKING CHANGE**: All switches now require an `id` field for multi-config merging.

```yaml
switches:
  - id: core-sw-01              # Unique identifier for merging
    hostname: aruba-core-01
    management_ip: 192.168.1.10
    model: Aruba2930F
    # ... rest of config
```

The `id` field is the unique key used to merge configurations from multiple sources. Switches with the same `id` across different config files will be merged according to priority rules.

### Merge Priority System

- **Priority Range**: 0-9999 (lower number = higher priority)
- **Main Config**: Priority 0-10 (reserved for main file only)
  - Default priority: 50 if not specified
- **Folder Configs**: Priority 11-9999 (folder files cannot use 0-10)
  - Default priority: 100 if not specified
- **Priority Specification**: Add `merge_priority` at the top level of any YAML file

```yaml
# main.yaml - can use 0-10
merge_priority: 5

switches:
  - id: sw-01
    hostname: switch-01
    # ...
```

```yaml
# folder config - must use 11+ (or omit for default of 100)
merge_priority: 150

switches:
  - id: sw-01
    vlans:
      - id: 100
        name: app-vlan
```

### Merge Strategy (Component Replacement)

The merge system uses **component replacement**, not field-level merging:

1. **VLANs**: Entire VLAN replaced by `id`
   - If VLAN 10 defined in multiple configs, highest priority wins
   - All fields (name, description, ip_config) come from that source

2. **Ports**: Entire port replaced by `port_id`
   - Port ranges expanded during load, then merged
   - Port "5" from priority-50 config replaces port "5" from priority-100

3. **Port Mirrors**: Entire mirror session replaced by `session_id`

4. **SNMP**: Sub-component list replacement
   - `communities` list replaced as a whole
   - `trap_receivers` list replaced as a whole
   - `enabled_traps` list replaced as a whole
   - Highest priority non-empty list wins for each sub-component

5. **Validation, Settings, Credentials**: Replace entire object
   - Highest priority config providing the object wins

### Identity Field Validation

Identity fields are **optional** in folder configs and only need to be present in **one** config file (typically the main config):
- `hostname`
- `management_ip`
- `model`
- `credentials`

**Validation Rules**:
- These fields only need to exist in ONE config file for a switch
- If a field appears in multiple configs, values **must match exactly**
- Mismatch results in a detailed conflict error listing all conflicting sources
- After merging, all four fields must be present (validated post-merge)

**List Fields Default to Empty**:
- `vlans: []` - Defaults to empty list if not specified
- `ports: []` - Defaults to empty list if not specified
- Lists from all configs are merged together (no replacement)

### Folder Scanning

- Folders are scanned for `*.yaml` and `*.yml` files
- Files loaded in **alphabetical order** by filename
- Use numeric prefixes for explicit ordering: `00-base.yaml`, `10-network.yaml`, `20-ports.yaml`

### Debug and Troubleshooting

```bash
# Show final merged configuration as YAML
./switch-configurator --config-file main.yaml --config-folder ./configs --show-merged-config

# Show merge process (placeholder, use --log-level debug for now)
./switch-configurator --config-file main.yaml --config-folder ./configs --show-merge-trace

# Detailed merge logging
./switch-configurator --config-file main.yaml --config-folder ./configs --log-level debug
```

### Example Multi-Config Setup

```
switch-config/
├── main.yaml                 # Priority 5: Core config with credentials
├── common/
│   ├── 00-base-vlans.yaml    # Priority 100: Standard VLANs
│   ├── 10-port-defaults.yaml # Priority 100: Default port configs
│   └── 20-snmp.yaml          # Priority 100: SNMP settings
└── overrides/
    └── production.yaml       # Priority 50: Production-specific overrides
```

**main.yaml** (priority 5):
```yaml
merge_priority: 5

switches:
  - id: sw-office-01
    hostname: aruba-office-01
    management_ip: 192.168.1.10
    model: Aruba2930F
    credentials:
      username: admin
      password: secret123
```

**common/00-base-vlans.yaml** (priority 100):
```yaml
# Uses default priority 100
# Note: Only 'id' is required - identity fields come from main.yaml

switches:
  - id: sw-office-01
    # No hostname, model, management_ip, or credentials needed
    # They're inherited from main.yaml
    vlans:
      - id: 10
        name: management
        ip_config: dhcp
      - id: 100
        name: users
```

**overrides/production.yaml** (priority 50):
```yaml
merge_priority: 50

switches:
  - id: sw-office-01
    # Only specify what you want to override
    # Identity fields optional unless you want to validate they match
    vlans:
      - id: 10
        name: mgmt-prod          # Replaces VLAN 10 from base (higher priority)
        ip_config:
          static:
            address: 192.168.10.1
            netmask: 255.255.255.0
    # VLAN 100 from base-vlans.yaml remains unchanged
```

**Alternative - With Identity Field Validation**:
If you want to ensure identity fields match across configs, you can include them:
```yaml
switches:
  - id: sw-office-01
    hostname: aruba-office-01     # Will validate this matches main.yaml
    management_ip: 192.168.1.10   # Will validate this matches main.yaml
    model: Aruba2930F             # Will validate this matches main.yaml
    vlans:
      - id: 10
        name: mgmt-prod
```
This provides an extra safety check but is not required.

**Result**: Switch `sw-office-01` will have:
- Credentials from main.yaml (priority 5)
- VLAN 10 from production.yaml (priority 50, overrides base)
- VLAN 100 from base-vlans.yaml (priority 100, not overridden)

See inline examples in this section for multi-config merge patterns.

### Migration Notes

**Breaking Changes**:
1. **`id` field required**: Add unique `id` to all switches
2. **Settings moved**: `settings` moved from root level to per-switch `switches[].settings`
3. **CLI change**: `--config` renamed to `--config-file` (no backward compatibility)

Migration script example:
```bash
# Add id field to all switches (manual step required)
# Move settings from root to each switch
# Update scripts using --config to use --config-file
```

## API Endpoints

The REST API runs on port 4002 (configurable):

- `GET /health` - Health check
- `GET /api/status` - Service status, config errors, and switch states
- `GET /switches` - List all configured switches
- `POST /switches/{id}/apply` - Apply in-memory config to one switch (async, see below)
- `GET /switches/{id}/config` - Get running configuration from switch via SSH
- `POST /config/reload` - Reload YAML from disk and apply to all switches (see below)
- `GET /switches/{id}/desired-config` - Get in-memory desired config for a switch
- `PUT /switches/{id}/desired-config` - Create or replace switch config in memory
- `PATCH /switches/{id}/desired-config` - Partial update to switch config in memory
- `DELETE /switches/{id}/desired-config` - Remove switch from in-memory config

Handler implementations are in `src/api/handlers.rs`.

### Async Apply Endpoint

The `POST /switches/{id}/apply` endpoint is **always asynchronous**:
- Returns `202 Accepted` immediately with a hint to poll `/api/status`
- The actual configuration is applied in a background task
- Returns `409 Conflict` only if the **same switch** is already being configured (per-switch conflict detection)
- Multiple different switches can be configured in parallel
- Poll `/api/status` to check progress:
  - `currently_configuring` is an array of switch IDs currently being configured (empty when idle)
  - `switches[].last_result` shows the outcome after completion

Example:
```bash
# Start async apply (can run multiple in parallel for different switches)
curl -X POST http://localhost:4002/switches/switch-01/apply
curl -X POST http://localhost:4002/switches/switch-02/apply  # OK - different switch

# This would return 409 Conflict (same switch already in progress)
curl -X POST http://localhost:4002/switches/switch-01/apply

# Poll for completion
curl http://localhost:4002/api/status
# Check "currently_configuring" array and "switches[].last_result"
```

### Understanding `apply` vs `reload`

**Config flow:**
```
YAML files (disk) --reload--> Memory (AppConfig) --apply--> Switch (hardware)
                                    ^
                                    |
                    PUT/PATCH /switches/{id}/desired-config
```

| Endpoint | Reads YAML? | Applies to | Use case |
|----------|-------------|------------|----------|
| `POST /config/reload` | Yes | All switches | YAML file changed, reload and push to all |
| `POST /switches/{id}/apply` | No | One switch | Re-push config to one switch (e.g., after switch reboot) |
| `PUT /switches/{id}/desired-config` | No | Memory only | Create/replace switch config via API |
| `PATCH /switches/{id}/desired-config` | No | Memory only | Update specific fields via API |

**Note:** The file watcher automatically performs `reload + apply all` when YAML files change. Config can also be defined via API using PUT/PATCH endpoints.

**Examples:**
```bash
# Reload YAML and apply to all switches
curl -X POST http://localhost:4002/config/reload

# Re-apply current config to one specific switch (returns 202, runs async)
curl -X POST http://localhost:4002/switches/aruba-switch-01/apply

# Create a new switch via API
curl -X PUT http://localhost:4002/switches/new-switch/desired-config \
  -H "Content-Type: application/json" \
  -d '{"id": "new-switch", "hostname": "sw1", "model": "Aruba2930F", "management_ip": "192.168.1.10", "credentials": {"username": "admin", "password": "secret"}}'

# Add a VLAN to existing switch
curl -X PATCH http://localhost:4002/switches/aruba-switch-01/desired-config \
  -H "Content-Type: application/json" \
  -d '{"id": "aruba-switch-01", "vlans": [{"id": 100, "name": "new-vlan"}]}'
```

**Comprehensive API Documentation:** See `docs/reference/api.md` for complete API reference including:
- Request/response formats and schemas
- Error handling and status codes
- Example usage with curl
- Common use cases and patterns
- Security considerations

## Error Handling

- Vendor operations return `Result<T, VendorError>` where `VendorError` is defined in `src/vendors/traits.rs:6`
- SSH errors are wrapped in `VendorError::SshError`
- Configuration validation errors use `VendorError::ValidationError`
- All functions use `anyhow::Result` for application-level errors

## Testing Strategy

- Unit tests for individual vendor command generation
- Integration tests should mock SSH connections using the trait abstraction
- Use `mockall` crate (already in dev-dependencies) for mocking `SwitchVendor` trait

## Common Development Tasks

### Modifying Vendor Commands

To change how commands are generated for a vendor:
1. Locate the vendor file: `src/vendors/{aruba|cisco|fortiswitch}.rs`
2. Modify the appropriate `generate_*_commands()` method or `apply_diff()` logic
3. Test with dry-run mode: `cargo run -- --one-off --dry-run` to see generated commands without executing

### Adding New Configuration Fields

To add new configuration options:
1. Add field to appropriate struct in `src/models.rs` (e.g., `Port`, `Vlan`, `PortMirror`)
2. Add `#[serde(default)]` if field is optional
3. Update `examples/config.example.yaml` with new field
4. Update `SwitchState` if the field needs state tracking
5. Update `src/diff/mod.rs` comparison logic if needed
6. Implement command generation in each vendor's `apply_diff()` or `configure_*()` methods
7. Update `SwitchVendor` trait if new operation type is needed

### Debugging Configuration Issues

Use combination of modes for debugging:

```bash
# See what changes would be made without applying
cargo run -- --one-off --dry-run --log-level debug

# Apply changes interactively with confirmation prompts
cargo run -- --one-off --debug --log-level debug

# Test on specific switch only
cargo run -- --one-off --switch hostname-here --dry-run
```

Enable debug logging to see state parsing and diff computation:
```bash
cargo run -- --log-level debug
```

**Using Example Configurations**:
The `examples/` directory contains complete, working example configurations for testing:
```bash
# Test with serial connection example
cargo run -- --config-file examples/aruba-serial.yaml --one-off --dry-run

# Test port range expansion
cargo run -- --config-file examples/port-ranges.yaml --one-off --dry-run

# Test multi-vendor configuration
cargo run -- --config-file examples/multi-vendor.yaml --one-off --dry-run
```

SSH client logs connection attempts, command execution, and output in `src/ssh/client.rs`.

### Testing State Parsing

To test parsing of switch running configurations:
1. Enable debug logging to see parsed state
2. Run with dry-run to verify diff computation
3. Check that `parse_current_state()` correctly extracts VLANs, ports, and mirrors
4. Verify `compute_diff()` identifies only necessary changes

### Working with Serial Connections

Serial connections provide direct console access to switches, useful for initial setup or when network access is unavailable:

**Configuration Setup**:
Set `connection_type: serial` in switch credentials along with `serial_device` (e.g., `/dev/ttyUSB0` or `/dev/serial/by-id/...`) and `baud_rate` (typically 9600 for most switches).

**Serial Device Identification**:
- List available serial devices: `ls -l /dev/ttyUSB* /dev/serial/by-id/*`
- Use by-id paths for stability: `/dev/serial/by-id/usb-FTDI_...` (survives reboots/reconnects)
- Common baud rates: 9600 (most common), 19200, 38400, 115200

**Testing Serial Connections**:
```bash
# Test serial connection with dry-run to see commands without executing
cargo run -- --one-off --dry-run --switch serial-switch-01 --log-level debug

# Interactive mode to step through commands on serial connection
cargo run -- --one-off --debug --switch serial-switch-01
```

**Serial Connection Features**:
- Automatic login handling (detects login prompts vs existing sessions)
- Intelligent prompt detection using regex patterns (matches `#`, `>`, `switch#`, `switch(config)#`, etc.)
- ANSI escape sequence filtering for clean output parsing (both in serial client and parser)
- Supports debug and dry-run modes like SSH connections
- Uses carriage return (`\r`) line endings as expected by switches

**ANSI Escape Code Handling**:
Serial connections often inject ANSI escape codes (cursor positioning, colors) into switch output. The Aruba parser automatically strips these before parsing configuration to ensure accurate state detection. See `docs/testing/aruba-serial-parsing-fixes.md` for details.

**Aruba Serial Connection Note**:
- If the previous user logged out, you may need to press ENTER twice to get a prompt
- The serial client handles this automatically in the login detection phase
- If connection issues occur, the client will retry with additional ENTER keystrokes

**Implementation Details** (`src/ssh/serial.rs`):
- `SerialClient` struct manages connection state and communication
- `login()` method detects current state and authenticates if needed
- `wait_for_prompt()` uses regex to identify when command execution completes
- `execute_command()` handles command execution with timeout and error detection

## Dependencies

Key dependencies and their purposes:
- **tokio**: Async runtime for concurrent operations
- **axum**: Web framework for REST API
- **russh/russh-keys**: SSH client implementation for network-based switch access
- **tokio-serial**: Serial port communication for direct console access
- **notify**: File system watcher for config changes
- **serde/serde_yaml**: Configuration serialization
- **tracing**: Structured logging
- **validator**: Configuration validation
- **anyhow/thiserror**: Error handling
- **regex**: Pattern matching for serial prompt detection

## NixOS Module

The flake includes a NixOS module (`nixosModules.default`) for deploying as a systemd service.

**Note**: The package build in `package.nix` uses source filtering to only include Rust-related files (Cargo.toml, Cargo.lock, src/). This means changes to `nixos-module.nix`, documentation, or example files will **not** trigger a Rust rebuild, only a module update.

### Module Structure

Located in `nixos-module.nix` (imported by `flake.nix`), the module provides:
- Systemd service configuration with automatic restarts
- User/group creation for service isolation
- Security hardening (NoNewPrivileges, ProtectSystem, ReadWritePaths)
- Configurable options for port, config file, log level, file watching

### Module Options

All options under `services.switch-configurator`:
- `enable` (bool): Enable the service
- `package` (package): Which package to use (auto-detected by default)
- `configFile` (path): Path to YAML configuration file
- `port` (port): API server port (default: 4002)
- `enableFileWatching` (bool): Watch config file for changes (default: true)
- `applyOnStartup` (bool): Apply configuration to all switches on service startup (default: true)
- `logLevel` (enum): trace, debug, info, warn, error (default: info)
- `user` (str): Service user (default: switch-configurator)
- `group` (str): Service group (default: switch-configurator)
- `extraGroups` (list): Supplementary groups for the systemd service (default: ["dialout"] for serial access)
  - Note: These groups are only active within the service context, not on the user account
- `environmentVariables` (attrs): Environment variables for the service (e.g., RUST_LOG, RUST_BACKTRACE)

### Usage Example

Import the flake in your NixOS configuration and enable the service. The module automatically:
1. Creates a systemd service with proper dependencies
2. Sets up user and group for service isolation
3. Configures file permissions on the config file (640, owned by service user)
4. Applies security hardening directives
5. Enables network access for SSH to switches

## Security Notes

- Configuration file may contain passwords in plaintext - ensure proper file permissions (chmod 600)
- SSH host key verification is currently simplified (accepts all) - see `src/ssh/client.rs:20`
- Consider implementing proper host key verification for production use
- Use SSH key authentication instead of passwords when possible
- Serial connections require physical access and appropriate device permissions (user must be in `dialout` or `uucp` group on Linux)
- NixOS module includes security hardening via systemd service options
