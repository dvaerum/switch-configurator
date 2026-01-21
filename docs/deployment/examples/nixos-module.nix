# NixOS Module Configuration Example
# This file demonstrates how to use the switch-configurator NixOS module

{ config, pkgs, ... }:

{
  # Import the flake in your flake.nix inputs:
  # inputs.switch-configurator.url = "github:yourusername/switch-configurator";
  #
  # Then in your configuration:
  # imports = [ inputs.switch-configurator.nixosModules.default ];

  # Basic configuration
  services.switch-configurator = {
    enable = true;

    # Path to your configuration file
    configFile = /etc/switch-configurator/config.yaml;

    # API server port (default: 4002)
    port = 4002;

    # Enable file watching (default: true)
    # When enabled, config changes are automatically applied
    enableFileWatching = true;

    # Logging level (default: "info")
    # Options: "trace", "debug", "info", "warn", "error"
    logLevel = "info";
  };

  # Create the configuration file
  # WARNING: This embeds passwords in the Nix store (world-readable)
  # For production, use a separate file with proper permissions
  environment.etc."switch-configurator/config.yaml" = {
    text = ''
      switches:
        - hostname: aruba-switch-01
          model: Aruba2930F
          management_ip: "192.168.1.10"
          credentials:
            username: admin
            password: admin  # Don't put real passwords here!
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
    '';
    mode = "0640";
    user = "switch-configurator";
    group = "switch-configurator";
  };
}

# Alternative: Use a separate configuration file (recommended for production)
# {
#   services.switch-configurator = {
#     enable = true;
#     configFile = /var/lib/switch-configurator/config.yaml;
#   };
#
#   # Manage the file separately with proper secrets management
#   # For example, using agenix, sops-nix, or other secrets managers
# }

# Advanced configuration with serial device access
# {
#   services.switch-configurator = {
#     enable = true;
#     configFile = /etc/switch-configurator/config.yaml;
#
#     # User and group (default: "switch-configurator")
#     user = "switch-configurator";
#     group = "switch-configurator";
#
#     # Extra groups for the service user
#     # "dialout" is included by default for serial device access
#     extraGroups = [ "dialout" ];
#
#     # Custom environment variables
#     environmentVariables = {
#       RUST_LOG = "debug";
#       RUST_BACKTRACE = "1";
#     };
#
#     # Logging
#     logLevel = "debug";
#   };
#
#   # Grant access to specific serial devices via udev rules
#   services.udev.extraRules = ''
#     # HP/Aruba serial adapters
#     SUBSYSTEM=="tty", ATTRS{idVendor}=="03f0", ATTRS{idProduct}=="*", GROUP="dialout", MODE="0660"
#
#     # Generic USB-to-serial adapters
#     SUBSYSTEM=="tty", ATTRS{idVendor}=="067b", GROUP="dialout", MODE="0660"
#   '';
# }

# Monitoring and management
# {
#   # View service status
#   # systemctl status switch-configurator
#
#   # View logs
#   # journalctl -u switch-configurator -f
#
#   # Restart service (to reload config if file watching is disabled)
#   # systemctl restart switch-configurator
#
#   # Stop service
#   # systemctl stop switch-configurator
# }

# Network configuration considerations
# {
#   # Ensure the management network is available
#   networking.interfaces.eth0.ipv4.addresses = [{
#     address = "192.168.1.5";
#     prefixLength = 24;
#   }];
#
#   networking.defaultGateway = "192.168.1.1";
#   networking.nameservers = [ "8.8.8.8" "1.1.1.1" ];
#
#   # Open firewall for API access (if needed)
#   networking.firewall.allowedTCPPorts = [ 4002 ];
# }

# Complete example with secrets management (using agenix)
# {
#   age.secrets.switch-config = {
#     file = ./secrets/switch-config.yaml.age;
#     owner = "switch-configurator";
#     group = "switch-configurator";
#     mode = "0640";
#   };
#
#   services.switch-configurator = {
#     enable = true;
#     configFile = config.age.secrets.switch-config.path;
#     logLevel = "info";
#   };
# }

# Using a custom package version
# {
#   services.switch-configurator = {
#     enable = true;
#
#     # Override the package
#     package = pkgs.switch-configurator.overrideAttrs (oldAttrs: {
#       # Custom build options or patches
#     });
#
#     configFile = /etc/switch-configurator/config.yaml;
#   };
# }

# Multiple instances (different configs)
# Note: This requires modifying the module to support multiple instances
# For now, use separate machines or containers
# {
#   # Instance for network A
#   systemd.services.switch-configurator-a = {
#     # ... custom service definition ...
#   };
#
#   # Instance for network B
#   systemd.services.switch-configurator-b = {
#     # ... custom service definition ...
#   };
# }
