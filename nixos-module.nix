{ self }:

{
  config,
  lib,
  pkgs,
  ...
}:

with lib;

let
  cfg = config.services.switch-configurator;
in
{
  options.services.switch-configurator = {
    enable = mkEnableOption "Multi-vendor switch configurator service";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = literalExpression "self.packages.\${pkgs.system}.default";
      description = "The switch-configurator package to use";
    };

    configFile = mkOption {
      type = types.path;
      description = ''
        Path to the YAML configuration file.
        The file should contain switch definitions, credentials, and settings.
      '';
      example = "/etc/switch-configurator/config.yaml";
    };

    extraConfigFolders = mkOption {
      type = types.listOf types.path;
      default = [ ];
      description = ''
        Additional directories to scan for YAML configuration files.
        Files in these directories will be merged with the main config file
        using priority-based merging.

        /etc/switch-configurator is always included automatically.
      '';
      example = literalExpression ''
        [ "/var/lib/switch-configs" "/run/dynamic-switches" ]
      '';
    };

    configDirectoryMode = mkOption {
      type = types.str;
      default = "0750";
      description = ''
        File mode for created configuration directories.
        Default is 0750 (owner: rwx, group: r-x, others: none).
      '';
      example = "0770";
    };

    configDirectoryGroup = mkOption {
      type = types.str;
      default = "switch-configurator";
      description = "The group used for the config folder: /etc/switch-configurator";
    };

    port = mkOption {
      type = types.port;
      default = 4002;
      description = "Port for the REST API server";
    };

    socketPath = mkOption {
      type = types.nullOr types.str;
      default = "/run/switch-configurator/api.sock";
      description = ''
        Unix socket path for the backend API.
        Used by the web UI for local communication.
        Set to null to disable unix socket listening.
      '';
    };

    strictDeployment = mkOption {
      type = types.bool;
      default = false;
      description = ''
        When true, all switch configs must pass validation or the service
        refuses to start. When false (default), invalid switches are skipped
        and valid switches are deployed normally. Skipped switches appear
        as validation failures in the dashboard.
      '';
    };

    enableFileWatching = mkOption {
      type = types.bool;
      default = true;
      description = "Enable automatic configuration reload on file changes";
    };

    applyOnStartup = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Apply configuration to all switches on service startup.
        When enabled, the service will connect to all configured switches
        and apply the configuration once before starting the API server.
      '';
    };

    logLevel = mkOption {
      type = types.enum [
        "trace"
        "debug"
        "info"
        "warn"
        "error"
      ];
      default = "info";
      description = "Logging level";
    };

    user = mkOption {
      type = types.str;
      default = "switch-configurator";
      description = "User account under which the service runs";
    };

    group = mkOption {
      type = types.str;
      default = "switch-configurator";
      description = "Group under which the service runs";
    };

    extraGroups = mkOption {
      type = types.listOf types.str;
      default = [ "dialout" ];
      description = ''
        Supplementary groups for the systemd service.
        'dialout' is included by default for serial device access.
        These groups are only active within the service context.
      '';
    };

    readOnly = mkOption {
      type = types.bool;
      default = false;
      description = ''
        When false (default), the service can write overlay configuration
        files to config folders via the web UI or API. The config folders
        are added to ReadWritePaths in the systemd service.

        When true, the service runs in read-only mode: no overlay files
        can be saved, and the web UI save feature is disabled.
      '';
    };

    loadCredentials = mkOption {
      type = types.attrsOf types.path;
      default = { };
      description = ''
        Secret files handed to the service through systemd `LoadCredential=`,
        keyed by credential id. systemd reads each file as root and gives the
        service a private copy under `$CREDENTIALS_DIRECTORY`, so the source
        file can stay `0400 root` and the service user never needs access to
        it. Reference a credential from the switch config as
        `name_credential: <id>` (SNMP communities).
      '';
      example = literalExpression ''
        {
          snmp-ro = config.sops.secrets."snmp-ro".path;
        }
      '';
    };

    environmentVariables = mkOption {
      type = types.attrsOf types.str;
      default = { };
      description = "Environment variables to set for the service";
      example = literalExpression ''
        {
          RUST_LOG = "debug";
          RUST_BACKTRACE = "1";
        }
      '';
    };

    # Web UI options
    ui = {
      enable = mkEnableOption "Switch configurator web UI";

      port = mkOption {
        type = types.port;
        default = 8080;
        description = "Port for the web UI server";
      };

      backendSocket = mkOption {
        type = types.str;
        default = "/run/switch-configurator/api.sock";
        description = "Unix socket path to connect to the backend API";
      };

      logLevel = mkOption {
        type = types.enum [
          "trace"
          "debug"
          "info"
          "warn"
          "error"
        ];
        default = cfg.logLevel;
        defaultText = literalExpression "config.services.switch-configurator.logLevel";
        description = "Logging level for the web UI";
      };
    };
  };

  config = mkIf cfg.enable {
    # The service reads credentials as `$CREDENTIALS_DIRECTORY/<id>`: an id
    # with a slash or dots-only would point outside that directory.
    assertions = [
      {
        assertion = all (id: builtins.match "[A-Za-z0-9._-]+" id != null && id != "." && id != "..") (
          attrNames cfg.loadCredentials
        );
        message = "services.switch-configurator.loadCredentials: credential ids must be plain names (letters, digits, '.', '_', '-'), not paths.";
      }
    ];

    # Create user and group
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "Switch configurator service user";
    };

    users.groups.${cfg.group} = { };

    # Create config directories with proper permissions
    systemd.tmpfiles.rules = [
      "d /etc/switch-configurator ${cfg.configDirectoryMode} ${cfg.user} ${cfg.configDirectoryGroup} -"
    ];

    # Backend service
    systemd.services.switch-configurator = {
      description = "Multi-vendor Switch Configurator";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      environment = cfg.environmentVariables;

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        SupplementaryGroups = cfg.extraGroups;
        Restart = "on-failure";
        RestartSec = "10s";
        RuntimeDirectory = "switch-configurator";
        LoadCredential = mapAttrsToList (id: path: "${id}:${path}") cfg.loadCredentials;

        ExecStart = ''
          ${cfg.package}/bin/switch-configurator \
            --config-file ${cfg.configFile} \
            ${
              concatMapStringsSep " " (folder: "--config-folder ${folder}") (
                [ "/etc/switch-configurator" ] ++ cfg.extraConfigFolders
              )
            } \
            --port ${toString cfg.port} \
            ${optionalString (cfg.socketPath != null) "--socket ${cfg.socketPath}"} \
            ${if cfg.enableFileWatching then "--watch" else "--watch=false"} \
            ${if cfg.applyOnStartup then "--apply-on-startup" else ""} \
            ${if cfg.strictDeployment then "--strict-deployment" else ""} \
            --log-level ${cfg.logLevel}
        '';

        # Security hardening
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateNetwork = false;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        # Allow writing to config folders for overlay saves (unless readOnly)
        ReadWritePaths = lib.mkIf (!cfg.readOnly) (
          [ "/etc/switch-configurator" ] ++ cfg.extraConfigFolders
        );
      };
    };

    # Web UI service
    systemd.services.switch-configurator-ui = mkIf cfg.ui.enable {
      description = "Switch Configurator Web UI";
      wantedBy = [ "multi-user.target" ];
      after = [ "switch-configurator.service" ];
      requires = [ "switch-configurator.service" ];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        ExecStart = ''
          ${cfg.package}/bin/switch-configurator-ui \
            --backend-socket ${cfg.ui.backendSocket} \
            --listen 0.0.0.0:${toString cfg.ui.port} \
            --log-level ${cfg.ui.logLevel}
        '';

        # Security hardening
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateNetwork = false;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
      };
    };
  };
}
