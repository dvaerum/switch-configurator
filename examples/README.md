# Configuration Examples

This directory contains example configurations for the switch-configurator service.

## Quick Start

1. Copy an example file to `config.yaml`
2. Edit the file with your switch details
3. Run: `switch-configurator --config-file config.yaml --one-off`

## Available Examples

### Basic Connection Types

- **[aruba-serial.yaml](aruba-serial.yaml)** - Serial connection to Aruba switch
  - Shows how to use USB-to-serial adapters
  - Demonstrates serial device path configuration
  - Baud rate settings for Aruba switches

- **[aruba-ssh.yaml](aruba-ssh.yaml)** - SSH connection to Aruba switches
  - Password authentication
  - SSH key-based authentication
  - Multiple switch management

### Advanced Configurations

- **[port-ranges.yaml](port-ranges.yaml)** - Port range syntax (NEW!)
  - Configure multiple ports with one line
  - Range syntax: "1-10", list syntax: "1,3,5", mixed: "1-5,7,10-12"
  - Reduces config size by 70%+ for identical ports
  - Works with all vendors (Aruba, Cisco, FortiSwitch)

- **[multi-vendor.yaml](multi-vendor.yaml)** - Multi-vendor environment
  - Aruba, Cisco, and FortiSwitch in one config
  - Different port naming conventions per vendor
  - Coordinated VLAN configuration across vendors

- **[vlan-ip-configs.yaml](vlan-ip-configs.yaml)** - VLAN IP addressing
  - DHCP/BOOTP configuration
  - Static IP addresses
  - Layer 2-only VLANs (no IP)
  - Multiple subnet examples

- **[port-mirroring.yaml](port-mirroring.yaml)** - Port mirroring (SPAN)
  - Single source to destination
  - Multiple sources to destination
  - Traffic direction options (rx/tx/both)
  - Use cases and best practices

- **[aruba-snmp-trap-testing.yaml](aruba-snmp-trap-testing.yaml)** - SNMP trap configuration and testing
  - Complete SNMP setup with communities and trap receivers
  - Per-port MAC notification control (mac_notify: true/false)
  - Link-change and MAC-notify trap configuration
  - Multiple test scenarios: all traps enabled, MAC-only, etc.
  - Demonstrates default trap behavior on Aruba switches

### Deployment

- **[nixos-module.nix](nixos-module.nix)** - NixOS module usage
  - Systemd service deployment
  - Security hardening
  - Serial device access configuration
  - Environment variables

## Configuration Structure

All configuration files follow this basic structure:

```yaml
switches:
  - id: switch-id              # Required: unique identifier for multi-config merging
    hostname: switch-name
    model: SwitchModel
    management_ip: "IP_ADDRESS"
    credentials:
      username: user
      password: pass
      connection_type: ssh|serial
      # Serial-specific options
      serial_device: /dev/ttyUSB0
      baud_rate: 9600
      # SSH-specific options
      port: 22
      ssh_key_path: /path/to/key
    vlans:
      - id: VLAN_ID
        name: vlan-name
        description: Optional description
        ip_config: dhcp|none|{address, netmask}
    ports:
      - port_id: "PORT_ID"        # Supports ranges: "1-5", "1,3,5", "1-5,7,10-12"
        mode: access|trunk
        vlan: NATIVE_VLAN_ID
        allowed_vlans: [VLAN_IDs]  # For trunk ports
        description: Optional description
        enabled: true|false
        poe_enabled: true|false
    port_mirrors:
      - session_id: "SESSION_ID"
        source_ports: ["PORT_IDs"]  # Also supports ranges: ["1-5", "10"]
        destination_port: "PORT_ID"  # Must be single port
        direction: both|rx|tx
    # Per-switch settings (optional)
    settings:
      ssh_timeout_secs: 30
      max_retries: 3
      dry_run: false
      enforce_port_config: false
```

### Port Range Syntax

Configure multiple ports with identical settings using range syntax:

```yaml
# Range: expands to ports 1,2,3,4,5
- port_id: "1-5"
  mode: access
  vlan: 20

# List: expands to ports 7,9,11
- port_id: "7,9,11"
  mode: access
  vlan: 20

# Mixed: expands to ports 1,2,3,4,5,7,10,11,12
- port_id: "1-5,7,10-12"
  mode: trunk
  vlan: 10
  allowed_vlans: [10, 20]
```

**Benefits:**
- Write 1 config line instead of 20 for identical ports
- Reduce configuration errors
- Easier maintenance

**Vendor-specific examples:**
```yaml
# Aruba
port_id: "1-24"

# Cisco
port_id: "GigabitEthernet1/0/1-48"

# FortiSwitch
port_id: "port1-24"
```

## Supported Switch Models

### Aruba
- `Aruba2530_24G_POE` - Aruba 2530 24G PoE+ Switch
- `Aruba2530_8G_POE` - Aruba 2530 8G PoE+ Switch
- `Aruba2530_48G_2SFP` - Aruba 2530-48G-2SFP+ (J9855A) 48-port Gigabit Switch with 2x SFP+ 10G uplinks
- `Aruba2540_24G` - Aruba 2540 24G Switch
- `Aruba2540_48G_4SFP` - Aruba 2540-48G-4SFP+ (JL355A) 48-port Gigabit Switch with 4x SFP+ 10G uplinks
- `Aruba2930F` - Aruba 2930F Switch Series

### Cisco
- `CiscoCatalyst9300_24P_UPOE` - Cisco Catalyst 9300 with UPoE

### FortiSwitch
- `Fortiswitch124F_FPOE` - FortiSwitch 124F with PoE

## Port ID Formats

Different vendors use different port naming conventions:

- **Aruba**: Simple numbers: `"1"`, `"23"`, `"24"`
- **Cisco**: Interface names: `"GigabitEthernet1/0/1"`, `"TenGigabitEthernet1/1/1"`
- **FortiSwitch**: Port names: `"port1"`, `"port24"`

## Operation Modes

### Service Mode (Default)
Runs continuously with API server and file watching:
```bash
switch-configurator --config-file config.yaml
```

### One-Off Mode
Apply configuration once and exit:
```bash
switch-configurator --config-file config.yaml --one-off
```

### Debug Mode
Interactive prompts before each command:
```bash
switch-configurator --config-file config.yaml --one-off --debug
```

### Dry-Run Mode
Show what would be done without executing:
```bash
switch-configurator --config-file config.yaml --one-off --dry-run
```

## Best Practices

1. **Start with dry-run**: Always test with `--dry-run` first
2. **Use serial for initial setup**: Serial connections are more reliable for first-time configuration
3. **Version control**: Keep your configs in git
4. **Separate credentials**: Use environment variables or separate credential files
5. **Test incrementally**: Start with simple configs and add complexity
6. **Backup before changes**: Save switch configs before modifications

## Security Considerations

- Store passwords securely (consider using SSH keys instead)
- Set appropriate file permissions: `chmod 600 config.yaml`
- Use dedicated service accounts with minimal privileges
- Enable audit logging on switches
- Keep configuration files out of public repositories

## Troubleshooting

### Serial Connection Issues
```bash
# Find serial devices
ls -l /dev/serial/by-id/

# Check user has dialout group membership
groups

# Add user to dialout group (requires logout/login)
sudo usermod -a -G dialout $USER
```

### SSH Connection Issues
```bash
# Test SSH connection manually
ssh admin@192.168.1.10

# Check SSH key permissions (must be 600)
chmod 600 /path/to/ssh_key
```

### Permission Issues
```bash
# Check file permissions
ls -l config.yaml

# Set correct permissions
chmod 640 config.yaml
```

## Getting Help

- Check logs: `journalctl -u switch-configurator -f` (NixOS)
- Enable debug logging: `--log-level debug`
- Review vendor documentation for command syntax
- See [AGENTS.md](../AGENTS.md) for development details

## Related Files

- [config.example.yaml](config.example.yaml) - Basic configuration example
- [config.example.vlan-ip.yaml](config.example.vlan-ip.yaml) - VLAN IP configuration examples
- [AGENTS.md](../AGENTS.md) - Development and architecture documentation
