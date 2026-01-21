# Switch Configurator Documentation

Multi-vendor network switch configurator with state-aware, idempotent configuration management.

## Quick Links

- **[Getting Started](guides/getting-started.md)** - Installation and first steps
- **[Configuration Guide](guides/configuration.md)** - How to write configuration files
- **[Examples](../examples/README.md)** - Ready-to-use configuration examples

## Documentation Structure

### 📘 Guides
- **[Getting Started](guides/getting-started.md)** - Quick start, installation, first configuration
- **[Configuration Guide](guides/configuration.md)** - Writing and managing configurations
- **[Troubleshooting](guides/troubleshooting.md)** - Common issues and solutions

### 📖 Reference
- **[CLI Reference](reference/cli.md)** - Command-line options
- **[API Reference](reference/api.md)** - REST API documentation
- **[Examples](../examples/README.md)** - Configuration examples with detailed explanations

### 🚀 Deployment
- **[NixOS Deployment](deployment/nixos.md)** - Deploy as a systemd service on NixOS

### 🔧 Development
- **[Architecture](development/architecture.md)** - System design and architecture
- **[State-Aware Implementation](development/state-aware-implementation.md)** - State parsing and diff computation
- **[Multi-Config System](development/multi-config-merge-design.md)** - Modular configuration merging
- **[CLAUDE.md](../CLAUDE.md)** - Development guidance for AI assistants (comprehensive architecture reference)

### 🧪 Testing
- **[Cisco Testing Documentation](testing/cisco/README.md)** - Hardware and unit test results
- **[Session Summaries](sessions/)** - Development session summaries and implementation plans
- 419 total tests (100% passing)
- Hardware validation on real switches

## Features

- [x] **Multi-Vendor Support** - Aruba, Cisco, FortiSwitch
- [x] **State-Aware** - Only applies necessary changes
- [x] **Idempotent** - Safe to run multiple times
- [x] **Connection Types** - SSH (password/key), Serial console, Jump hosts
- [x] **Legacy SSH Support** - Compatible with older switches using ssh-rsa algorithm
- [x] **Multi-Config Merging** - Modular YAML configuration management
- [x] **REST API** - Programmatic configuration management
- [x] **File Watching** - Automatic config reload on changes
- [x] **Port Mirroring** - SPAN/mirror configuration support
- [x] **VLAN Management** - Full Layer 2 and Layer 3 VLAN support
- [x] **SNMP Configuration** - Communities, traps, and receivers
- [x] **Comprehensive Testing** - 419 tests (100% passing), hardware-validated
- [x] **NixOS Module** - First-class Nix/NixOS integration

## Quick Start

### Installation (NixOS)

```nix
{
  inputs.switch-configurator.url = "github:yourusername/switch-configurator";

  nixpkgs.overlays = [ switch-configurator.overlays.default ];

  services.switch-configurator = {
    enable = true;
    configFile = /etc/switch-configurator/config.yaml;
  };
}
```

### Installation (Standalone)

```bash
# Using Nix flakes
nix run github:yourusername/switch-configurator -- --help

# Or build and install
nix build github:yourusername/switch-configurator
./result/bin/switch-configurator --help
```

### Basic Configuration

```yaml
switches:
  - hostname: my-switch
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

settings:
  ssh_timeout_secs: 30
  max_retries: 3
  dry_run: false
```

### Running

```bash
# One-off mode (apply and exit)
switch-configurator --config-file config.yaml --one-off

# Service mode (with API and file watching)
switch-configurator --config-file config.yaml

# Dry-run (show what would be done)
switch-configurator --config-file config.yaml --one-off --dry-run

# Debug mode (interactive prompts)
switch-configurator --config-file config.yaml --one-off --debug

# Multi-config mode (merge multiple YAML files)
switch-configurator --config-file main.yaml --config-folder /etc/switch-configs/common
```

## Use Cases

### Network Automation
Automate switch configuration across multiple sites with consistent, version-controlled configs.

### Zero-Touch Provisioning
Configure new switches via serial connection before they're added to the network.

### Configuration Drift Detection
State-aware system detects and corrects configuration drift automatically.

### Multi-Vendor Networks
Manage heterogeneous networks with Aruba, Cisco, and FortiSwitch from a single tool.

### GitOps Workflows
Combine with file watching and git hooks for automated configuration deployment.

## Architecture Overview

```
┌─────────────────┐
│  Configuration  │
│   (YAML File)   │
└────────┬────────┘
         │
         ▼
┌─────────────────┐     ┌──────────────┐
│  File Watcher   │────▶│  REST API    │
│   (Optional)    │     │  (Port 4002) │
└────────┬────────┘     └──────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│      Switch Configurator Core       │
│  • Config Validation                │
│  • State Parsing                    │
│  • Diff Computation                 │
│  • Command Generation               │
└────────┬────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│      Vendor Implementations         │
│  • Aruba   • Cisco   • FortiSwitch  │
└────────┬────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│      Connection Handlers            │
│  • SSH Client   • Serial Client     │
└────────┬────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│         Network Switches            │
└─────────────────────────────────────┘
```

## Support

- **Issues**: [GitHub Issues](https://github.com/yourusername/switch-configurator/issues)
- **Examples**: See the [examples/](../examples/) directory
- **Development**: See [development documentation](development/)

## License

See [LICENSE](../LICENSE) file in the repository root.
