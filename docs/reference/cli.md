# CLI Reference

Complete command-line interface reference for switch-configurator.

## Usage

```bash
switch-configurator [OPTIONS]
```

## Options

### Configuration

**`--config-file <FILE>`**
- Path to main YAML configuration file
- Default: `config.yaml`
- Example: `--config-file /etc/switch-configurator/production.yaml`

**`--config-folder <FOLDER>`**
- Additional folder(s) to load config files from
- Can be specified multiple times
- Files are merged with main config by switch `id`
- See [Multi-Config Merging](../guides/configuration.md#multi-config-merging)
- Example: `--config-folder /etc/switch-configurator/common`

**`--show-merged-config`**
- Print the final merged configuration and exit
- Useful for debugging multi-config setups
- Example: `--config-file main.yaml --config-folder ./common --show-merged-config`

**`--show-merge-trace`**
- Show detailed merge trace and exit
- Provides audit trail of merge decisions
- Useful for debugging which config source contributed each setting
- Example: `--config-file main.yaml --config-folder ./common --show-merge-trace`

### Operation Modes

**`--one-off`**
- Apply configuration once and exit
- Disables API server and file watching
- Use for manual runs, CI/CD, cron jobs

**`--debug`**
- Interactive mode with command-by-command prompts
- Prompts: `[Y/n/q]` before each command
- Useful for learning and troubleshooting

**`--dry-run`**
- Show what would be done without executing
- Safe way to preview changes
- Recommended before production runs

### Targeting

**`--switch <HOSTNAME>`**
- Apply configuration to specific switch only
- Must match `hostname` field in config
- Example: `--switch aruba-core-01`

### API Server

**`--port <PORT>`**
- API server listen port
- Default: `4002`
- Only used in service mode (without `--one-off`)

**`--watch <BOOL>`**
- Enable/disable file watching
- Default: `true`
- Values: `true`, `false`
- Only used in service mode

**`--apply-on-startup`**
- Apply configuration to all switches on startup
- Used in service mode before starting the API server
- Useful when you want config applied immediately when the service starts
- Example: `--apply-on-startup`

### Logging

**`--log-level <LEVEL>`**
- Set logging verbosity
- Levels: `trace`, `debug`, `info`, `warn`, `error`
- Default: `info`
- Example: `--log-level debug`

## Examples

### Basic Usage

```bash
# Apply config to all switches
switch-configurator --config-file config.yaml --one-off

# Run as service
switch-configurator --config-file config.yaml
```

### Testing and Debugging

```bash
# Dry-run (no changes)
switch-configurator --config-file config.yaml --one-off --dry-run

# Debug mode (interactive)
switch-configurator --config-file config.yaml --one-off --debug

# Debug logging
switch-configurator --config-file config.yaml --one-off --log-level debug

# Combine modes
switch-configurator --config-file config.yaml --one-off --dry-run --log-level debug
```

### Targeting Specific Switches

```bash
# Apply to one switch
switch-configurator --config-file config.yaml --one-off --switch aruba-01

# Dry-run on specific switch
switch-configurator --config-file config.yaml --one-off --dry-run --switch cisco-core
```

### Multi-Config Mode

```bash
# Load main config + common folder
switch-configurator --config-file main.yaml --config-folder ./common --one-off

# Multiple folders (merged in order)
switch-configurator --config-file main.yaml \
  --config-folder ./common \
  --config-folder ./overrides \
  --one-off

# Preview merged configuration
switch-configurator --config-file main.yaml --config-folder ./common --show-merged-config

# Dry-run with multi-config
switch-configurator --config-file main.yaml --config-folder ./common --one-off --dry-run
```

### Service Mode

```bash
# Default (port 4002, file watching enabled)
switch-configurator --config-file config.yaml

# Custom port
switch-configurator --config-file config.yaml --port 9000

# Disable file watching
switch-configurator --config-file config.yaml --watch false

# Custom port with debug logging
switch-configurator --config-file config.yaml --port 9000 --log-level debug

# Service mode with multi-config
switch-configurator --config-file main.yaml --config-folder ./common --port 4002
```

## Environment Variables

### Rust Logging

```bash
# Detailed Rust-level logging
RUST_LOG=debug switch-configurator --config-file config.yaml --one-off

# Module-specific logging
RUST_LOG=switch_configurator::vendors=debug switch-configurator --config-file config.yaml --one-off
```

### Backtrace

```bash
# Enable backtrace on panics
RUST_BACKTRACE=1 switch-configurator --config-file config.yaml --one-off

# Full backtrace
RUST_BACKTRACE=full switch-configurator --config-file config.yaml --one-off
```

## Exit Codes

- **0**: Success
- **1**: Configuration error
- **2**: Connection error
- **Other**: Unexpected error (check logs)

## Output Formats

### Normal Output

```
INFO Starting switch-configurator v0.1.0
INFO Configuring: aruba-switch-01
INFO ✓ Connected successfully
INFO ✓ Applied 3 configuration change(s)
INFO ✓ Configuration saved
INFO ✓ Disconnected
```

### Debug Output

```
DEBUG Opening serial device: /dev/ttyUSB0 at 9600 baud
DEBUG Serial connection established
DEBUG Current state check: ...
DEBUG Parsing current state from aruba-switch-01
DEBUG Found mirror destination port: 22
DEBUG   VLAN 42: IP config = DHCP
DEBUG Computing configuration differences
DEBUG   Port 23 to configure:
DEBUG     Current: mode=Trunk, vlan=666, allowed_vlans=[42, 666]
DEBUG     Desired: mode=Trunk, vlan=666, allowed_vlans=[666]
```

### Dry-Run Output

```
INFO [DRY-RUN] Would execute: configure terminal
INFO [DRY-RUN] Would execute: interface 23
INFO [DRY-RUN] Would execute: no tagged vlan 42
INFO [DRY-RUN] Would execute: exit
```

## Integration Examples

### Cron Job

```bash
# Apply config daily at 2 AM
0 2 * * * /usr/bin/switch-configurator --config-file /etc/switches/config.yaml --one-off >> /var/log/switch-config.log 2>&1
```

### Systemd Service

See [NixOS Deployment](../deployment/nixos.md) for systemd service configuration.

### CI/CD Pipeline

```yaml
# GitHub Actions example
- name: Configure Switches
  run: |
    nix run .#switch-configurator -- \
      --config-file config/production.yaml \
      --one-off \
      --log-level info
```

### Git Hook

```bash
#!/bin/bash
# .git/hooks/pre-push

# Validate config before push
switch-configurator --config-file config.yaml --one-off --dry-run

if [ $? -ne 0 ]; then
  echo "Configuration validation failed"
  exit 1
fi
```

## See Also

- [Getting Started](../guides/getting-started.md) - Installation and first steps
- [Configuration Guide](../guides/configuration.md) - Configuration file format
- [Examples](../../examples/README.md) - Configuration examples
- [Troubleshooting](../guides/troubleshooting.md) - Common issues
