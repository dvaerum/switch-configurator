# AGENTS.md - Agent Coding Guidelines

This file provides guidelines for AI agents (OpenCode, Claude Code, etc.) working on the Switch Configurator codebase.

---

## Workflow Conventions

- **Commit whenever something is fixed or improved.** After each self-contained
  fix, feature, or improvement lands and tests pass, make a commit rather than
  batching unrelated changes together.
- **Bump the version accordingly** with every such commit, following Semantic
  Versioning (`switch-configurator/Cargo.toml`, and `switch-configurator-ui/Cargo.toml`
  when the UI changes):
  - patch (`0.4.8 → 0.4.9`) for bug fixes / small improvements,
  - minor (`0.4.x → 0.5.0`) for backward-compatible features,
  - major for breaking changes.
- Update `CHANGELOG.md` (under `## [Unreleased]` or a new version heading) when
  the change is user-visible.
- **Push after committing.** Once the commit (with its version bump) is made and
  tests pass, run `git push` to publish it.

---

## Project Overview

Switch Configurator is a Rust-based service that automates network switch configuration across multiple vendors (Aruba, Cisco, FortiSwitch). The service watches a YAML configuration file for changes and automatically applies them to switches via SSH or serial connection. It also provides a REST API for programmatic configuration management.

### Supported Switch Models

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

Use `SwitchModel::supports_poe()` to check programmatically.

---

## Build Commands

### Basic Build
```bash
cargo build                      # Debug build (default)
cargo build --release           # Release build (optimized)
cargo build --profile dev-fast  # Fast dev build with minimal optimization
cargo build --profile release-fast  # Fast release build (thin LTO)
cargo check                    # Check without building
```

### Running the Application

#### Service Mode (Default)
Runs continuously with API server and file watching:
```bash
cargo run                                    # Default (config.yaml, port 4002)
cargo run -- --config-file /path/to/config.yaml --port 9000
cargo run -- --watch false                  # Disable file watching
cargo run -- --apply-on-startup             # Apply on startup
cargo run --release                         # Release mode
```

#### One-Off Mode
Apply configuration once and exit (no API server or file watching):
```bash
cargo run -- --one-off                          # Apply to all switches
cargo run -- --one-off --switch aruba-switch-01 # Apply to specific switch
cargo run -- --one-off --dry-run                # Preview without executing
cargo run -- --one-off --debug                  # Interactive prompts
```

### Testing
```bash
cargo test                              # Run all tests
cargo test -- --nocapture              # Run with output
cargo test test_name_here               # Run specific test
cargo test --lib vendors::aruba::tests # Run tests in specific module
cargo test --test multi_config_tests   # Run integration tests
cargo test -vv                          # Run with verbose output
```

### Code Quality
```bash
cargo fmt           # Format code
cargo clippy        # Run linter
cargo clippy --fix  # Auto-fix linter warnings
```

---

## Code Style Guidelines

### General Principles
- Use **Rust 2021 edition** (specified in Cargo.toml)
- Prefer **async/await** for I/O operations (tokio runtime)
- Use **trait objects** for vendor abstraction (`Box<dyn SwitchVendor>`)
- Keep functions focused and small (< 100 lines preferred)

### Imports (group in this order)
```rust
// Standard library
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// External crates (alphabetically)
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// Internal crate modules
use crate::config::RuntimeConfig;
use crate::models::{ConfigResult, SwitchState, Vlan};
```

### Naming Conventions
| Element | Convention | Example |
|---------|------------|---------|
| Structs/Enums | PascalCase | `ArubaSwitch`, `Vendor` |
| Enum variants | PascalCase | `PortMode::Access` |
| Functions/Variables | snake_case | `generate_vlan_commands()` |
| Constants | SCREAMING_SNAKE_CASE | `DEFAULT_TIMEOUT` |
| Traits | PascalCase | `SwitchVendor` |
| Modules | snake_case | `ssh`, `vendors` |

### Types
- Use **explicit return types** on public functions
- Prefer **`&str`** over **`&String`** for function parameters

```rust
// Good
fn generate_vlan_commands(&self, vlans: &[Vlan]) -> Vec<String>
// Avoid
fn generate_vlan_commands(&self, vlans: &Vec<Vlan>) -> Vec<String>
```

### Error Handling
Use **`anyhow::Result`** for application-level errors and **`thiserror`** for library-level errors:

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum VendorError {
    #[error("SSH connection error: {0}")]
    SshError(String),
    #[error("Command execution error: {0}")]
    CommandError(String),
}

// Use context and bail!
async fn connect(&mut self) -> Result<()> {
    let session = russh::client::connect(...)
        .await
        .context("Failed to establish SSH connection")?;
    if !self.is_valid() {
        anyhow::bail!("Invalid configuration");
    }
    Ok(())
}
```

### Async Patterns
Use **`async_trait`** for async trait methods and `?` for Result propagation:

```rust
#[async_trait]
pub trait SwitchVendor: Send + Sync {
    async fn connect(&mut self) -> Result<(), VendorError>;
    async fn disconnect(&mut self) -> Result<(), VendorError>;
}
```

### Struct Patterns
```rust
// Prefer named fields for structs with multiple fields
pub struct ArubaSwitch {
    config: SwitchConfig,
    runtime_config: RuntimeConfig,
    client: Option<ConnectionClient>,
    enforce_port_config: bool,
}

// Always derive Debug for internal structs
#[derive(Debug, Clone)]
struct PortVlanInfo { port_id: String, untagged_vlan: Option<u16> }

// Use tuple structs only for single-field wrappers
struct Wrapper(Something);
```

### Documentation
Add doc comments (`///`) for public APIs with usage examples:

```rust
/// Connect to the switch via SSH.
///
/// # Errors
/// Returns `VendorError::SshError` if connection fails
pub async fn connect(&mut self) -> Result<(), VendorError> { ... }
```

### Testing
- Place unit tests in `#[cfg(test)]` module in same file
- Use `mockall` for mocking trait objects
- Integration tests go in `tests/` directory

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_vlan_commands() {
        let commands = generate_vlan_commands(&[Vlan { id: 10, name: "test".into() }]);
        assert!(commands.contains(&"vlan 10".into()));
    }
}
```

### Logging
Use **tracing** for structured logging:

```rust
debug!("Processing port {}", port_id);   // Detailed debugging
info!("Connected to switch {}", hostname); // Operational events
warn!("Port {} not found, skipping", port_id);   // Handled issues
error!("Failed to apply config: {}", e);         // Failures
```

---

## Project Structure

- `src/` - Rust source code (28 modules, ~15,000 lines)
- `docs/` - Documentation (30+ files)
  - `docs/guides/` - User guides
  - `docs/development/` - Internal architecture docs
  - `docs/testing/` - Test documentation
- `tests/` - Automated tests (300+ tests, 60+ fixtures)
- `examples/` - Example YAML configurations (15 complete examples)

Vendor implementations follow `SwitchVendor` trait in `src/vendors/traits.rs`. Each vendor (Aruba, Cisco, FortiSwitch) implements vendor-specific command generation and state parsing.

---

## Architecture

### Core Design Pattern

The project uses a **trait-based vendor abstraction pattern** to support multiple switch vendors. Each vendor implements the `SwitchVendor` trait.

### Key Components

1. **Vendor Factory** (`src/vendors/mod.rs`): `create_vendor_with_runtime()` returns a boxed trait object based on switch model.

2. **State-Aware Configuration**: 
   - Connect → Parse current state → Compute diff → Apply only changes → Save config
   - Idempotent operations (running twice has no effect)
   - `enforce_port_config` controls whether unconfigured ports are reset

3. **Connection Abstraction** (`src/ssh/connection.rs`): Unified interface for SSH and Serial connections.

4. **Configuration Store** (`src/config.rs`): Thread-safe using `Arc<RwLock<AppConfig>>`.

5. **File Watcher** (`src/watcher/mod.rs`): Uses notify crate to detect config changes.

6. **Validation System** (`src/validation/mod.rs`): Pre-deployment testing framework.

### Module Structure

- **main.rs**: Entry point and CLI argument handling
- **lib.rs**: Library entry point
- **models.rs**: Core data structures (SwitchConfig, Port, Vlan, etc.)
- **config.rs**: Configuration loading from YAML
- **diff/**: State comparison logic
- **vendors/**: Vendor implementations (traits, aruba, cisco, fortiswitch)
- **ssh/**: SSH/Serial clients, jump host support
- **api/**: REST API server (Axum)
- **watcher/**: File system watcher
- **validation/**: Configuration validation

---

## Configuration File Format

Key sections in `config.yaml`:

- `switches[]`: Array of switch configurations
- `switches[].model`: Must match `SwitchModel` enum (e.g., `Aruba2930F`)
- `switches[].credentials`: SSH or Serial credentials
  - SSH: `username`, `password` or `ssh_key_path`, `port`
  - Serial: `connection_type: serial`, `serial_device`, `baud_rate`
  - Jump hosts: `jump_hosts` array for bastion servers
- `switches[].vlans[]`: VLAN definitions (id, name, ip_config)
 - `switches[].ports[]`: Port configurations (mode, vlan, poe_enabled, etc.)
   - Port range syntax: `"1-5"`, `"1,3,5"`, `"1-5,7,10-12"`
   - VLAN references (`vlan`, `tagged_vlans`) accept a numeric ID or a VLAN **name**.
     Type-strict: bare int = ID (`vlan: 10`), quoted string = name lookup
     (`vlan: "Users"`; `vlan: "10"` looks up a VLAN *named* "10"). Names are
     case-sensitive, must be unique, and are resolved to IDs at load time
     (see `resolve_vlan_names` in `config.rs`). Unknown untagged name = error;
     unknown tagged name = dropped-with-warning (lenient) / error (strict).
- `switches[].port_mirrors[]`: Port mirroring sessions
- `switches[].snmp`: SNMP configuration (communities, trap_receivers, enabled_traps)
- `switches[].settings`: Per-switch settings (ssh_timeout_secs, enforce_port_config, etc.)

---

## Multi-Config Merge System

Supports loading multiple YAML files for modular configurations:

```bash
cargo run -- --config-file main.yaml --config-folder /etc/switch-configs/common
```

- **Switch Identity**: All switches require unique `id` field for merging
- **Merge Priority**: 0-9999 (lower = higher priority)
  - Main config: 0-10 (default 50)
  - Folder configs: 11-9999 (default 100)
- **Strategy**: Component replacement by id (not field-level merge)

---

## API Endpoints

The REST API runs on port 4002 (configurable):

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check |
| `GET /api/status` | Service status and switch states |
| `GET /switches` | List all configured switches |
| `POST /switches/{id}/apply` | Apply config to one switch (async, returns 202) |
| `GET /switches/{id}/config` | Get running config from switch |
| `POST /config/reload` | Reload YAML and apply to all |
| `PUT/PATCH/DELETE /switches/{id}/desired-config` | Manage in-memory config |

---

## Common Development Tasks

### Debugging Configuration Issues
```bash
# See what changes would be made
cargo run -- --one-off --dry-run --log-level debug

# Interactive mode with confirmation prompts
cargo run -- --one-off --debug --log-level debug

# Test specific switch
cargo run -- --one-off --switch hostname-here --dry-run
```

### Adding New Configuration Fields
1. Add field to struct in `src/models.rs`
2. Add `#[serde(default)]` if optional
3. Update `examples/config.example.yaml`
4. Update `SwitchState` if state tracking needed
5. Update `src/diff/mod.rs` comparison logic
6. Implement command generation in vendor's `apply_diff()`

### Adding a New Vendor
1. Create `src/vendors/yourvendor.rs`
2. Define struct implementing `SwitchVendor` trait
3. Implement all trait methods (connect, disconnect, parse_current_state, apply_diff, etc.)
4. Add vendor to `Vendor` enum in `src/models.rs`
5. Add model to `SwitchModel` enum
6. Update factory in `src/vendors/mod.rs`

---

## Dependencies

- **tokio**: Async runtime
- **axum**: Web framework for REST API
- **russh/russh-keys**: SSH client
- **tokio-serial**: Serial port communication
- **notify**: File system watcher
- **serde/serde_yaml**: Configuration serialization
- **tracing**: Structured logging
- **validator**: Configuration validation
- **anyhow/thiserror**: Error handling
- **regex**: Pattern matching

---

## NixOS Module

The flake includes a NixOS module for deploying as a systemd service. Options under `services.switch-configurator`:
- `enable`, `package`, `configFile`, `port`
- `enableFileWatching`, `applyOnStartup`, `logLevel`
- `user`, `group`, `extraGroups`, `environmentVariables`

---

## Security Notes

- Configuration files may contain passwords - use `chmod 600`
- SSH host key verification is simplified (accepts all) - see `src/ssh/client.rs`
- Use SSH key authentication instead of passwords when possible
- Serial connections require `dialout` or `uucp` group membership
