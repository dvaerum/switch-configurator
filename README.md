# Switch Configurator

A Rust-based multi-vendor network switch configurator with state-aware, idempotent configuration management.

[![Built with Nix](https://img.shields.io/badge/built%20with-nix-blue.svg)](https://nixos.org/)

## Features

- [x] **Multi-Vendor Support** - Aruba, Cisco, FortiSwitch
- [x] **State-Aware** - Only applies necessary changes
- [x] **Idempotent** - Safe to run multiple times
- [x] **Connection Types** - SSH (password/key), Serial console, and SSH Jump Hosts
- [x] **Jump Host Support** - Multi-hop bastion server chains with authentication fallback
- [x] **Multi-Config Merging** - Modular YAML configuration management
- [x] **REST API** - Programmatic configuration management
- [x] **File Watching** - Automatic config reload on changes
- [x] **Port Mirroring** - SPAN/mirror configuration support
- [x] **VLAN Management** - Full Layer 2 and Layer 3 VLAN support
- [x] **SNMP Configuration** - Communities, traps, and receivers
- [x] **Comprehensive Testing** - 419 tests (100% passing), hardware-validated
- [x] **NixOS Module** - First-class Nix/NixOS integration

## Quick Start

### Installation

```bash
# Run directly with Nix
nix run github:yourusername/switch-configurator -- --help

# Or on NixOS
services.switch-configurator = {
  enable = true;
  configFile = /etc/switch-configurator/config.yaml;
};
```

### Basic Configuration

```yaml
switches:
  - id: my-switch-01              # Unique ID for multi-config merging
    hostname: my-switch
    model: Aruba2930F
    management_ip: "192.168.1.10"
    credentials:
      username: admin
      password: admin
      connection_type: ssh
    vlans:
      - id: 10
        name: management
        ip_config: dhcp
    ports:
      - port_id: "1"
        mode: access
        vlan: 10
        enabled: true
    settings:                     # Per-switch settings
      ssh_timeout_secs: 30
      max_retries: 3
      dry_run: false
```

### Running

```bash
# Test with dry-run
switch-configurator --config-file config.yaml --one-off --dry-run

# Apply configuration
switch-configurator --config-file config.yaml --one-off

# Run as service with API
switch-configurator --config-file config.yaml

# Multi-config mode (merge multiple YAML files)
switch-configurator --config-file main.yaml --config-folder /etc/switch-configs/common
```

## Documentation

📘 **[Complete Documentation](docs/README.md)** - Start here

### Guides
- **[Getting Started](docs/guides/getting-started.md)** - Installation and first steps
- **[Configuration Guide](docs/guides/configuration.md)** - Writing configurations
- **[Troubleshooting](docs/guides/troubleshooting.md)** - Common issues and solutions

### Reference
- **[CLI Reference](docs/reference/cli.md)** - Command-line options
- **[Configuration Schema](examples/README.md)** - Complete YAML reference
- **[CLAUDE.md](CLAUDE.md)** - Comprehensive architecture and development guide

### Deployment
- **[NixOS Deployment](docs/deployment/nixos.md)** - Deploy as systemd service

### Development
- **[Architecture](docs/development/architecture.md)** - System design
- **[CLAUDE.md](CLAUDE.md)** - Development guidance and adding vendors

## Examples

See the [examples directory](examples/) for complete, working configurations:

- **[aruba-serial.yaml](examples/aruba-serial.yaml)** - Serial connection example
- **[aruba-ssh.yaml](examples/aruba-ssh.yaml)** - SSH connection examples
- **[ssh-with-jump-hosts.yaml](examples/ssh-with-jump-hosts.yaml)** - Jump host/bastion configuration
- **[multi-vendor.yaml](examples/multi-vendor.yaml)** - Mixed vendor environment
- **[vlan-ip-configs.yaml](examples/vlan-ip-configs.yaml)** - All IP config modes
- **[port-mirroring.yaml](examples/port-mirroring.yaml)** - SPAN configuration

## Supported Switches

**Aruba:**
- 2530-24G PoE+, 2530-8G PoE+, 2530-48G-2SFP+
- 2540-24G, 2540-48G-4SFP+
- 2930F

**Cisco:**
- Catalyst 9300-24P UPoE

**FortiSwitch:**
- 124F-FPOE

## Building

### Using Nix (Recommended)

```bash
# Enter development environment
nix develop

# Build
nix build

# Run
nix run
```

### Using Cargo

```bash
# Build
cargo build --release

# Run
./target/release/switch-configurator --help
```

## Operation Modes

- **Service Mode**: Continuous operation with API and file watching
- **One-Off Mode**: Apply configuration once and exit
- **Debug Mode**: Interactive prompts before each command
- **Dry-Run Mode**: Preview changes without applying

See [Getting Started](docs/guides/getting-started.md) for detailed usage.

## API Endpoints

- `GET /health` - Health check
- `GET /api/status` - Service status and recent operations
- `GET /switches` - List all configured switches
- `POST /switches/{id}/apply` - Apply configuration (async, returns 202)
- `GET /switches/{id}/config` - Get running configuration
- `POST /config/reload` - Reload and apply to all switches (async, returns 202)
- `GET /switches/{id}/desired-config` - Get in-memory desired config
- `PUT /switches/{id}/desired-config` - Create/replace switch config
- `PATCH /switches/{id}/desired-config` - Partial update to switch config
- `DELETE /switches/{id}/desired-config` - Remove switch from config

**Note:** Apply and reload endpoints are asynchronous - they return `202 Accepted` immediately and process in the background. Poll `/api/status` to monitor progress.

See [docs/reference/api.md](docs/reference/api.md) for complete API documentation with examples, request/response formats, and usage patterns.

## NixOS Module

Deploy as a systemd service with the included NixOS module:

```nix
{
  inputs.switch-configurator.url = "github:yourusername/switch-configurator";

  services.switch-configurator = {
    enable = true;
    configFile = /etc/switch-configurator/config.yaml;
    port = 4002;
    enableFileWatching = true;
    logLevel = "info";
    extraGroups = [ "dialout" ];  # For serial device access
  };
}
```

See [NixOS Deployment](docs/deployment/nixos.md) for complete guide.

## Development

### Project Structure

```
src/
├── main.rs              # Application entry point
├── models.rs            # Data models and types
├── config.rs            # Configuration management
├── api/                 # REST API implementation
├── ssh/                 # SSH and serial client implementation
├── vendors/             # Vendor-specific implementations
│   ├── traits.rs        # SwitchVendor trait
│   ├── aruba.rs         # Aruba implementation
│   ├── cisco.rs         # Cisco implementation
│   └── fortiswitch.rs   # FortiSwitch implementation
├── diff/                # State comparison logic
└── watcher/             # File watcher for config changes
```

### Adding New Vendors

See [CLAUDE.md](CLAUDE.md) section "Adding a New Vendor" for detailed instructions.

### Testing

The project has comprehensive test coverage:

```bash
# Run all tests
cargo test

# Run specific vendor tests
cargo test --lib vendors::cisco::tests
cargo test --lib vendors::aruba::tests

# Run with output
cargo test -- --nocapture

# Code quality
cargo clippy
cargo fmt
```

**Test Coverage:**
- 419 total tests (100% passing)
- Comprehensive vendor tests (Aruba, Cisco, FortiSwitch)
- Multi-config integration tests
- API endpoint tests
- Hardware validation on real switches

See [Cisco Testing Documentation](docs/testing/cisco/README.md) for detailed test results.

## Contributing

Contributions are welcome! See [CLAUDE.md](CLAUDE.md) and [Architecture](docs/development/architecture.md) for development guidance.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Support

- **Documentation**: [docs/](docs/)
- **Examples**: [examples/](examples/)
- **Issues**: [GitHub Issues](https://github.com/yourusername/switch-configurator/issues)
