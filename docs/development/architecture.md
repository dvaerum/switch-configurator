# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Switch Configurator is a Rust-based service that automates network switch configuration across multiple vendors (Aruba, Cisco, FortiSwitch). The service watches a YAML configuration file for changes and automatically applies them to switches via SSH or serial connection. It also provides a REST API for programmatic configuration management.

## Supported Switch Models

- **Aruba**: 2530-24G PoE+, 2530-8G PoE+, 2540-24G, 2930F
- **FortiSwitch**: 124F-FPOE
- **Cisco**: Catalyst 9300-24P UPoE

Corresponding `SwitchModel` enum values: `Aruba2530_24G_POE`, `Aruba2530_8G_POE`, `Aruba2540_24G`, `Aruba2930F`, `Fortiswitch124F_FPOE`, `CiscoCatalyst9300_24P_UPOE`

## Build and Run Commands

### Nix Flake (Recommended)

This project uses Nix flakes for reproducible builds and development environments.

```bash
# Enter development shell (includes Rust toolchain, rust-analyzer, cargo tools)
nix develop

# Build the package
nix build

# Run the application
nix run

# Run with arguments
nix run . -- --config-file /path/to/config.yaml --port 9000

# Check flake validity
nix flake check

# Update flake inputs
nix flake update
```

The development shell (`nix develop`) provides:
- Rust stable toolchain with rust-analyzer and rust-src extensions
- Cargo tools: cargo-watch, cargo-edit, cargo-audit
- All required build dependencies (OpenSSL, pkg-config, etc.)
- Additional utilities (jq, yq)

### Building with Cargo

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check
```

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
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test <test_name>
```

### Code Quality
```bash
# Format code
cargo fmt

# Run linter
cargo clippy

# Auto-fix linter warnings
cargo clippy --fix
```

## Architecture

### Core Design Pattern

The project uses a **trait-based vendor abstraction pattern** to support multiple switch vendors. Each vendor (Aruba, Cisco, FortiSwitch) implements the `SwitchVendor` trait defined in `src/vendors/traits.rs`.

### Module Structure

- **models.rs**: Core data structures (SwitchConfig, Port, Vlan, PortMirror, SwitchState, StateDiff, ConnectionType)
- **config.rs**: Configuration loading/saving from YAML files, RuntimeConfig for execution modes
- **diff/**: State comparison logic that computes differences between current and desired state
- **vendors/**: Vendor-specific implementations
  - **traits.rs**: `SwitchVendor` trait that all vendors must implement
  - **aruba.rs**, **cisco.rs**, **fortiswitch.rs**: Vendor implementations
  - **mod.rs**: Factory functions `create_vendor()` and `create_vendor_with_runtime()`
- **ssh/**: Connection client implementations for accessing switches
  - **client.rs**: SSH client using russh for network-based switch access
  - **serial.rs**: Serial client using tokio-serial for direct console access
  - **connection.rs**: Unified `ConnectionClient` enum that abstracts SSH vs Serial
  - **mod.rs**: Module exports for connection clients
- **api/**: REST API server and handlers using Axum framework
- **watcher/**: File system watcher using `notify` crate to detect config file changes

### Key Architectural Components

1. **Vendor Factory Pattern** (`src/vendors/mod.rs:17`): The `create_vendor_with_runtime()` function returns a boxed trait object based on the switch model, enabling runtime polymorphism. Accepts `RuntimeConfig` for debug/dry-run modes.

2. **State-Aware Configuration System**: The service reads the current switch state before applying changes, computes a diff, and only applies necessary changes (idempotent operations).
   - **Flow**: Connect → Parse current state (`parse_current_state()`) → Compute diff (`src/diff/mod.rs:6`) → Apply only changes (`apply_diff()`) → Save config to switch's startup-config (executes `write memory` on switch, does NOT modify YAML files)
   - **Benefits**: Efficiency (minimal commands), safety (no unnecessary changes), idempotency (running twice has no effect)
   - See `STATE_AWARE_IMPLEMENTATION.md` for implementation details

3. **Runtime Configuration** (`src/config.rs:85`): `RuntimeConfig` struct controls execution modes (debug, dry-run, one-off, target switch) passed to vendor implementations.

4. **Configuration Store** (`src/config.rs:79`): Thread-safe configuration using `Arc<RwLock<AppConfig>>` for concurrent access from API handlers and file watcher.

5. **Connection Abstraction** (`src/ssh/connection.rs:5`): The `ConnectionClient` enum provides a unified interface for both SSH and Serial connections, allowing vendor implementations to work with either connection type transparently.
   - **SSH Mode** (`src/ssh/client.rs`): Network-based access using russh library with async/await
   - **Serial Mode** (`src/ssh/serial.rs`): Direct console access via serial port using tokio-serial
   - Both clients support debug mode (interactive prompts) and dry-run mode (preview commands)
   - Serial client includes intelligent prompt detection and automatic login handling

6. **Interactive Shell Usage**: All vendor implementations use interactive shells (PTY sessions) for both SSH and serial connections, not exec mode. This design choice ensures:
   - **Consistent Behavior**: Same command execution flow across SSH and serial connections
   - **Universal Compatibility**: Works with switches that don't support exec mode (e.g., Aruba switches)
   - **Pagination Handling**: All vendors disable pagination immediately after connection to prevent `--More--` or paging prompts
   - **Implementation Details**:
     - **Aruba**: Executes `no page` after connection (SSH and serial)
     - **Cisco**: Executes `terminal length 0` after connection (SSH and serial)
     - **FortiSwitch**: Executes `config system console` → `set output standard` → `end` after connection (SSH and serial)
   - Pagination disabling is performed at connection time in the `connect()` method, ensuring all subsequent commands receive unpaginated output
   - Prompt detection uses regex patterns to identify command completion (see `src/ssh/client.rs:233` and `src/ssh/serial.rs`)

7. **File Watcher Integration** (`src/watcher/mod.rs`): Uses notify crate to watch config file and automatically applies changes to all switches.

### Adding a New Vendor

To add support for a new switch vendor:

1. Create new file: `src/vendors/yourvendor.rs`
2. Define struct: `pub struct YourVendorSwitch { config: SwitchConfig, runtime_config: RuntimeConfig, client: Option<ConnectionClient>, enforce_port_config: bool }`
3. Implement all `SwitchVendor` trait methods:
   - **Connection**: `connect()`, `disconnect()`
     - **IMPORTANT**: Must disable pagination at connection time for both SSH and serial modes
     - Execute vendor-specific pagination disable command immediately after establishing connection
     - Examples: `no page` (Aruba), `terminal length 0` (Cisco), `config system console` → `set output standard` → `end` (FortiSwitch)
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

**Connection Setup** (implements trait method):
1. `connect()`: Establish connection and configure session
   - Create SSH or Serial client based on `ConnectionType`
   - **SSH Mode**:
     - Call `ssh_client.connect_with_credentials()` (handles jump hosts automatically)
     - **Immediately** execute pagination disable command(s)
   - **Serial Mode**:
     - Call `serial_client.connect()` and `serial_client.login()`
     - **Immediately** execute pagination disable command(s)
   - Store client in `self.client` as `ConnectionClient` enum
   - Example pagination commands: `no page` (Aruba), `terminal length 0` (Cisco), `config system console` + `set output standard` + `end` (FortiSwitch)

**Command Generation** (vendor-specific helpers):
1. `generate_vlan_commands()`: Builds vendor-specific VLAN configuration commands
2. `generate_port_commands()`: Builds port configuration commands (access/trunk mode, PoE, etc.)
3. `generate_mirror_commands()`: Builds port mirroring/SPAN commands
4. `normalize_port_id()`: Converts generic port identifiers to vendor-specific format

**State Parsing** (implements trait method):
1. `parse_current_state()`: Parses "show running-config" output into `SwitchState { vlans, ports, port_mirrors }`
   - Use simple line-by-line regex/string matching
   - Extract VLANs, port configurations, and mirror sessions
   - Return structured state representation

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

2. `apply_diff()`: Apply only the changes specified in `StateDiff`
   - Add/update/remove VLANs as specified
   - Configure changed ports only
   - Add/update/remove port mirrors as specified

Example port identifiers: Aruba uses "interface 1", Cisco uses "GigabitEthernet1/0/1", FortiSwitch uses "port1"

## Configuration File Format

The service uses YAML configuration (`config.yaml` or custom path). See `../../examples/config.example.yaml` for complete structure.

Key sections:
- `switches[]`: Array of switch configurations
- `switches[].model`: Must match `SwitchModel` enum variants (e.g., `Aruba2530_24G_POE`, `Aruba2930F`, `CiscoCatalyst9300_24P_UPOE`)
- `switches[].credentials`: Connection credentials supporting both SSH and Serial modes
  - **SSH Mode** (default): Requires `username`, `password` or `ssh_key_path`, and `port` (default: 22)
  - **Serial Mode**: Requires `connection_type: serial`, `serial_device` (e.g., `/dev/ttyUSB0`), `baud_rate` (default: 9600), `username`, and `password`
  - Example SSH: `{ username: admin, password: secret, port: 22 }`
  - Example Serial: `{ connection_type: serial, serial_device: /dev/ttyUSB0, baud_rate: 9600, username: admin, password: secret }`
- `switches[].vlans[]`: VLAN definitions (id, name, description, ip_config)
  - **IP Configuration**: Each VLAN can have an IP address configuration:
    - `ip_config: dhcp` - Get IP address from DHCP server
    - `ip_config: none` - No IP address (default)
    - `ip_config: { static: { address: "192.168.1.1", netmask: "255.255.255.0" } }` - Static IP
  - Example: `{ id: 10, name: management, ip_config: dhcp }`
  - See `../../examples/config.example.vlan-ip.yaml` for detailed examples
- `switches[].ports[]`: Port configurations (mode, vlan, poe_enabled, etc.)
- `switches[].port_mirrors[]`: Port mirroring sessions
- `settings`: Global settings (ssh_timeout, max_retries, dry_run)

## API Endpoints

The REST API runs on port 4002 (configurable):

- `GET /health` - Health check
- `GET /switches` - List all configured switches
- `POST /switches/{hostname}/apply` - Apply configuration to specific switch
- `GET /switches/{hostname}/config` - Get running configuration from switch
- `POST /config/reload` - Reload configuration from file

Handler implementations are in `src/api/handlers.rs`.

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
3. Update `../../examples/config.example.yaml` with new field
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
- ANSI escape sequence filtering for clean output parsing
- Supports debug and dry-run modes like SSH connections
- Uses carriage return (`\r`) line endings as expected by switches

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

### Module Structure

Located in `flake.nix` (not a separate file), the module provides:
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
- `logLevel` (enum): trace, debug, info, warn, error (default: info)
- `user` (str): Service user (default: switch-configurator)
- `group` (str): Service group (default: switch-configurator)

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
