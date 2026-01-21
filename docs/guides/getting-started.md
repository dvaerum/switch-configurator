# Getting Started

This guide will help you install and run switch-configurator for the first time.

## Prerequisites

- **Network Access**: Ability to connect to your switches via SSH or serial
- **Credentials**: Admin username and password for your switches
- **Nix** (recommended): For easiest installation
  - Alternatively: Rust toolchain (1.70+) if building from source

## Installation

### Option 1: Nix Flakes (Recommended)

The easiest way to use switch-configurator is with Nix flakes:

```bash
# Run directly without installing
nix run github:yourusername/switch-configurator -- --help

# Or install to your profile
nix profile install github:yourusername/switch-configurator

# Then run
switch-configurator --help
```

### Option 2: NixOS System

Add to your NixOS configuration:

```nix
{
  inputs.switch-configurator.url = "github:yourusername/switch-configurator";

  imports = [ switch-configurator.nixosModules.default ];

  services.switch-configurator = {
    enable = true;
    configFile = /etc/switch-configurator/config.yaml;
  };
}
```

See [NixOS Deployment](../deployment/nixos.md) for complete details.

### Option 3: Build from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/switch-configurator
cd switch-configurator

# Using Nix
nix build

# Or using Cargo
cargo build --release
./target/release/switch-configurator --help
```

## First Configuration

### 1. Create a Configuration File

Create a file named `config.yaml`:

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
        poe_enabled: false

settings:
  ssh_timeout_secs: 30
  max_retries: 3
  dry_run: false
```

### 2. Test with Dry-Run

Always test first with `--dry-run` to see what would be changed:

```bash
switch-configurator --config-file config.yaml --one-off --dry-run
```

This will:
- ✅ Connect to the switch
- ✅ Parse current configuration
- ✅ Show what commands would be executed
- ❌ NOT actually change anything

### 3. Apply Configuration

If the dry-run looks good, apply the configuration:

```bash
switch-configurator --config-file config.yaml --one-off
```

You should see output like:
```
INFO Starting switch-configurator v0.1.0
INFO Configuring: my-switch
INFO ✓ Connected successfully
INFO ✓ Applied 3 configuration change(s)
INFO ✓ Configuration saved
INFO ✓ Disconnected
```

### 4. Verify Idempotency

Run the same command again to verify idempotency:

```bash
switch-configurator --config-file config.yaml --one-off
```

You should see:
```
INFO ✓ No changes needed - switch already in desired state
```

## Connection Types

### SSH Connection

Most common for switches already on the network:

```yaml
credentials:
  username: admin
  password: admin123
  connection_type: ssh
  port: 22  # Optional, defaults to 22
```

For better security, use SSH key authentication:

```yaml
credentials:
  username: admin
  ssh_key_path: /etc/switch-configurator/ssh_key
  connection_type: ssh
```

### Serial Connection

Useful for initial setup or out-of-band management:

```yaml
credentials:
  username: admin
  password: admin
  connection_type: serial
  serial_device: /dev/ttyUSB0
  baud_rate: 9600
```

**Finding your serial device:**
```bash
ls -l /dev/serial/by-id/
```

See [examples/aruba-serial.yaml](../../examples/aruba-serial.yaml) for a complete serial connection example.

## Operation Modes

### One-Off Mode

Apply configuration once and exit (no API server):

```bash
switch-configurator --config-file config.yaml --one-off
```

**Use cases:**
- Manual configuration changes
- CI/CD pipelines
- Scheduled cron jobs

### Service Mode

Run continuously with API server and file watching:

```bash
switch-configurator --config-file config.yaml
```

**Features:**
- REST API on port 4002 (configurable)
- Automatic config reload on file changes
- Best for production deployments

### Debug Mode

Interactive prompts before each command:

```bash
switch-configurator --config-file config.yaml --one-off --debug
```

**Prompts:**
```
Execute this command? [Y/n/q]:
  Y/yes/Enter: Execute the command
  n/no: Skip this command
  q/quit: Abort entirely
```

### Dry-Run Mode

Show what would be done without executing:

```bash
switch-configurator --config-file config.yaml --one-off --dry-run
```

**Combines well with:**
- `--log-level debug` for more details
- Specific switch targeting with `--switch hostname`

## Common Commands

```bash
# Apply config to all switches
switch-configurator --config-file config.yaml --one-off

# Apply to specific switch only
switch-configurator --config-file config.yaml --one-off --switch my-switch

# Dry-run with debug logging
switch-configurator --config-file config.yaml --one-off --dry-run --log-level debug

# Run as service
switch-configurator --config-file config.yaml

# Run as service on custom port
switch-configurator --config-file config.yaml --port 9000

# Disable file watching
switch-configurator --config-file config.yaml --watch false
```

## Next Steps

- **[Configuration Guide](configuration.md)** - Learn about all configuration options
- **[Examples](../../examples/README.md)** - See complete configuration examples
- **[CLI Reference](../reference/cli.md)** - Command-line options
- **[Troubleshooting](troubleshooting.md)** - Common issues and solutions

## Quick Tips

💡 **Always use dry-run first** before applying to production switches

💡 **Version control your configs** - Keep them in git for change tracking

💡 **Start simple** - Begin with basic VLAN and port configs, add complexity gradually

💡 **Check logs** - Use `--log-level debug` for detailed troubleshooting

💡 **Test idempotency** - Run twice to ensure no unwanted changes

💡 **Use examples** - The `examples/` directory has many ready-to-use configs
