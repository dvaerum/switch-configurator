# NixOS Deployment

Deploy switch-configurator as a systemd service on NixOS with the included module.

## Quick Start

Add to your `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    switch-configurator.url = "github:yourusername/switch-configurator";
  };

  outputs = { self, nixpkgs, switch-configurator }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        # Import the module
        switch-configurator.nixosModules.default

        # Configure the service
        ({ config, pkgs, ... }: {
          services.switch-configurator = {
            enable = true;
            configFile = /etc/switch-configurator/config.yaml;
          };

          # Create the config file
          environment.etc."switch-configurator/config.yaml".text = ''
            # Your configuration here
          '';
        })
      ];
    };
  };
}
```

## Module Options

### Basic Options

```nix
services.switch-configurator = {
  enable = true;                                 # Enable the service
  configFile = /etc/switch-configurator/config.yaml;  # Config file path
  port = 4002;                                   # API server port (default: 4002)
  enableFileWatching = true;                     # Auto-reload on config change
  logLevel = "info";                             # Log level: trace, debug, info, warn, error
};
```

### Advanced Options

```nix
services.switch-configurator = {
  enable = true;
  configFile = /var/lib/switch-configurator/config.yaml;

  # User and group
  user = "switch-configurator";         # Service user (default)
  group = "switch-configurator";        # Service group (default)

  # Extra groups (for serial device access)
  extraGroups = [ "dialout" ];          # Default includes dialout

  # Custom environment variables
  environmentVariables = {
    RUST_LOG = "debug";                 # Rust logging
    RUST_BACKTRACE = "1";               # Enable backtraces
  };

  # Logging
  logLevel = "debug";
};
```

### Multi-Config Support

The service supports merging multiple configuration files using priority-based merging. The `/etc/switch-configurator` directory is automatically created and always included for drop-in configs.

```nix
services.switch-configurator = {
  enable = true;
  configFile = /etc/switch-configurator/main.yaml;

  # Additional folders to scan for YAML configs
  # These are merged with the main config file
  extraConfigFolders = [
    "/var/lib/switch-configs"      # Additional switches
    "/run/dynamic-switches"        # Runtime-generated configs
  ];

  # Permissions for /etc/switch-configurator directory
  configDirectoryMode = "0750";    # Default: owner=rwx, group=r-x, others=none
  configDirectoryGroup = "switch-configurator";  # Default group

  # Apply configuration on service startup
  applyOnStartup = true;           # Default: true
};
```

**Note:** Only `/etc/switch-configurator` is automatically created by the module. Additional folders in `extraConfigFolders` must be created manually or through other configuration mechanisms.

**Drop-in Configuration Example:**

```bash
# /etc/switch-configurator is automatically created with correct permissions
# Add additional switch configs as drop-ins:
sudo vim /etc/switch-configurator/office-switches.yaml
sudo vim /etc/switch-configurator/datacenter-switches.yaml

# These will be automatically merged with main.yaml
```

**Priority-Based Merging:**
- Higher priority configs override lower priority configs
- Priority is set per-config using the `priority` field (default: 100)
- Later files in a directory (alphabetically) override earlier files
- See [Multi-Config Design](../development/multi-config-merge-design.md) for details

### Using the Overlay

```nix
{
  # Apply the overlay to make the package available
  nixpkgs.overlays = [
    switch-configurator.overlays.default
  ];

  # Now you can reference it in other places
  environment.systemPackages = [ pkgs.switch-configurator ];

  # Or override the package used by the service
  services.switch-configurator = {
    enable = true;
    package = pkgs.switch-configurator.overrideAttrs (oldAttrs: {
      # Custom modifications
    });
    configFile = /etc/switch-configurator/config.yaml;
  };
}
```

## Configuration Management

### Option 1: Inline Configuration (Simple)

```nix
{
  services.switch-configurator = {
    enable = true;
    configFile = /etc/switch-configurator/config.yaml;
  };

  environment.etc."switch-configurator/config.yaml" = {
    text = ''
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
    '';
    mode = "0640";
    user = "switch-configurator";
    group = "switch-configurator";
  };
}
```

**Warning:** This embeds passwords in the Nix store (world-readable). Use for testing only.

### Option 2: External File (Recommended)

```nix
{
  services.switch-configurator = {
    enable = true;
    configFile = /var/lib/switch-configurator/config.yaml;
  };

  # Manage the file outside of Nix
  # Set ownership and permissions manually or with deployment tools
}
```

Then create the file manually:
```bash
sudo mkdir -p /var/lib/switch-configurator
sudo vim /var/lib/switch-configurator/config.yaml
sudo chown switch-configurator:switch-configurator /var/lib/switch-configurator/config.yaml
sudo chmod 640 /var/lib/switch-configurator/config.yaml
```

### Option 3: Secrets Management (Production)

#### Using agenix

```nix
{
  # Configure agenix
  age.secrets.switch-config = {
    file = ./secrets/switch-config.yaml.age;
    owner = "switch-configurator";
    group = "switch-configurator";
    mode = "0640";
  };

  services.switch-configurator = {
    enable = true;
    configFile = config.age.secrets.switch-config.path;
  };
}
```

#### Using sops-nix

```nix
{
  sops.secrets.switch-config = {
    sopsFile = ./secrets/switches.yaml;
    owner = "switch-configurator";
    group = "switch-configurator";
    mode = "0640";
  };

  services.switch-configurator = {
    enable = true;
    configFile = config.sops.secrets.switch-config.path;
  };
}
```

## Serial Device Access

For serial connections, ensure proper device access:

```nix
{
  services.switch-configurator = {
    enable = true;
    configFile = /etc/switch-configurator/config.yaml;

    # Include dialout group (default)
    extraGroups = [ "dialout" ];
  };

  # Grant access to HP/Aruba serial adapters
  services.udev.extraRules = ''
    # HP/Aruba serial adapters
    SUBSYSTEM=="tty", ATTRS{idVendor}=="03f0", GROUP="dialout", MODE="0660"

    # Generic USB-to-serial adapters
    SUBSYSTEM=="tty", ATTRS{idVendor}=="067b", GROUP="dialout", MODE="0660"

    # FTDI adapters (Cisco, etc.)
    SUBSYSTEM=="tty", ATTRS{idVendor}=="0403", GROUP="dialout", MODE="0660"
  '';
}
```

**Find your device vendor ID:**
```bash
lsusb
# Example output:
# Bus 001 Device 005: ID 03f0:3524 HP, Inc HP Serial Port
```

## Network Configuration

Ensure the host can reach your switches:

```nix
{
  # Static IP on management interface
  networking.interfaces.eth0.ipv4.addresses = [{
    address = "192.168.1.5";
    prefixLength = 24;
  }];

  networking.defaultGateway = "192.168.1.1";
  networking.nameservers = [ "8.8.8.8" "1.1.1.1" ];

  # Open firewall for API access (if needed)
  networking.firewall.allowedTCPPorts = [ 4002 ];
}
```

## Service Management

### Systemd Commands

```bash
# Check service status
systemctl status switch-configurator

# View logs (follow mode)
journalctl -u switch-configurator -f

# View recent logs
journalctl -u switch-configurator -n 100

# Restart service (reload config if file watching disabled)
systemctl restart switch-configurator

# Stop service
systemctl stop switch-configurator

# Start service
systemctl start switch-configurator

# Enable service (start on boot)
systemctl enable switch-configurator

# Disable service
systemctl disable switch-configurator
```

### Checking API

```bash
# Health check
curl http://localhost:4002/health

# Service status (detailed)
curl http://localhost:4002/api/status | jq '.'

# List switches
curl http://localhost:4002/switches

# Get switch config
curl http://localhost:4002/switches/my-switch/config

# Apply config to specific switch
curl -X POST http://localhost:4002/switches/my-switch/apply

# Reload configuration
curl -X POST http://localhost:4002/config/reload
```

The `/api/status` endpoint provides comprehensive service information including:
- Service version and uptime
- Configuration status (loaded files, last reload time)
- Per-switch metrics (apply counts, success/failure rates, last applied timestamp)
- Recent errors (last 50 with timestamps)
- API endpoint list

## Security Considerations

The module includes security hardening by default:

- **NoNewPrivileges**: Prevents privilege escalation
- **PrivateTmp**: Isolated temporary directory
- **ProtectSystem**: Read-only system directories
- **ProtectHome**: No access to user home directories
- **RestrictAddressFamilies**: Limited to IPv4, IPv6, and Unix sockets
- **DeviceAllow**: Only specified serial devices accessible

### Additional Hardening

```nix
{
  services.switch-configurator = {
    enable = true;
    configFile = config.age.secrets.switch-config.path;

    # Use dedicated user
    user = "network-admin";
    group = "network-admin";

    # Limit environment
    environmentVariables = { };  # Don't expose extra variables
  };

  # Create dedicated user
  users.users.network-admin = {
    isSystemUser = true;
    group = "network-admin";
    extraGroups = [ "dialout" ];
  };

  users.groups.network-admin = { };
}
```

## Monitoring

### Prometheus Metrics (Future)

Currently, monitoring is via logs. For metrics, consider:

```nix
{
  services.promtail = {
    enable = true;
    configuration = {
      clients = [{
        url = "http://loki:3100/loki/api/v1/push";
      }];
      scrape_configs = [{
        job_name = "journal";
        journal = {
          json = false;
          max_age = "12h";
          labels = {
            job = "systemd-journal";
            host = config.networking.hostName;
          };
        };
        relabel_configs = [{
          source_labels = [ "__journal__systemd_unit" ];
          target_label = "unit";
        }];
      }];
    };
  };
}
```

### Log Forwarding

```nix
{
  # Forward to syslog server
  services.rsyslog = {
    enable = true;
    extraConfig = ''
      :programname, isequal, "switch-configurator" @@syslog.example.com:514
    '';
  };
}
```

## Troubleshooting

### Service Won't Start

Check the service status:
```bash
systemctl status switch-configurator
journalctl -u switch-configurator -n 50
```

Common issues:
- **Config file not found**: Check `configFile` path exists
- **Permission denied**: Verify file ownership and permissions
- **Serial device access**: Check user is in dialout group
- **Port already in use**: Another service using port 4002

### Serial Connection Issues

```bash
# Check device exists
ls -l /dev/serial/by-id/

# Check permissions
ls -l /dev/ttyUSB0

# Check user groups
groups switch-configurator

# Add to dialout group if missing
usermod -a -G dialout switch-configurator
systemctl restart switch-configurator
```

### Configuration Not Reloading

If file watching is enabled but changes aren't applied:

```bash
# Check file watching is enabled
systemctl cat switch-configurator | grep watch

# Manually reload
curl -X POST http://localhost:4002/config/reload

# Or restart service
systemctl restart switch-configurator
```

## Complete Example

See [examples/nixos-module.nix](../../examples/nixos-module.nix) for a complete, annotated example.

## Next Steps

- **[Getting Started](../guides/getting-started.md)** - Quick start guide
- **[Configuration Guide](../guides/configuration.md)** - Configuration reference
- **[Examples](../../examples/README.md)** - Configuration examples
