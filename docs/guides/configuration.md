# Configuration Guide

Complete guide to writing and managing switch configurations.

## Configuration File Structure

Configuration files use YAML format:

```yaml
merge_priority: 50   # Optional: for multi-config merging (0-9999, lower = higher priority)

switches:            # List of switches to manage
  - id: my-switch    # Required: unique identifier
    hostname: ...
    model: ...
    vlans: ...
    ports: ...
    settings: ...    # Per-switch settings
```

## Switch Configuration

Each switch in the `switches` list requires these fields:

### Required Fields

```yaml
- id: my-switch-01              # Unique ID (required for multi-config merging)
  hostname: my-switch-01        # Display name / DNS hostname
  model: Aruba2930F             # Switch model (see supported models)
  management_ip: "192.168.1.10" # IP address for SSH (or placeholder for serial)
  credentials:                  # Authentication details
    username: admin
    password: admin
    connection_type: ssh        # "ssh" or "serial"
```

**Note:** The `id` field is required and must be unique across all switches. It's used as the merge key when using [multi-config merging](#multi-config-merging).

### Supported Switch Models

**Aruba:**
- `Aruba2530_24G_POE`
- `Aruba2530_8G_POE`
- `Aruba2540_24G`
- `Aruba2930F`

**Cisco:**
- `CiscoCatalyst9300_24P_UPOE`

**FortiSwitch:**
- `Fortiswitch124F_FPOE`

See [CLAUDE.md](../../CLAUDE.md) for detailed vendor implementation information.

## Credentials

### SSH with Password

```yaml
credentials:
  username: admin
  password: secure-password
  connection_type: ssh
  port: 22  # Optional, defaults to 22
```

### SSH with Key

```yaml
credentials:
  username: admin
  ssh_key_path: /path/to/private/key
  connection_type: ssh
  port: 22
```

**Note:** When using SSH keys, don't set the `password` field.

### Serial Connection

```yaml
credentials:
  username: admin
  password: admin
  connection_type: serial
  serial_device: /dev/serial_aruba-2930F  # Use udev symlinks for stable paths
  baud_rate: 9600  # Aruba typically uses 9600, Cisco uses 9600
```

**Finding Serial Devices:**
```bash
# List udev symlinks (recommended for stable paths)
ls -l /dev/serial_*

# Or list by-id paths
ls -l /dev/serial/by-id/
```

## VLAN Configuration

### Basic VLAN

```yaml
vlans:
  - id: 10
    name: management
    description: Management VLAN  # Optional
```

### VLAN with DHCP IP

```yaml
vlans:
  - id: 10
    name: management
    ip_config: dhcp  # Get IP from DHCP server
```

### VLAN with Static IP

```yaml
vlans:
  - id: 20
    name: servers
    ip_config:
      address: "10.0.20.1"
      netmask: "255.255.255.0"
```

### Layer 2-Only VLAN

```yaml
vlans:
  - id: 30
    name: guest
    ip_config: none  # No IP address (default)
```

**Note:** If `ip_config` is not specified, it defaults to `none`.

## Port Configuration

### Access Port

```yaml
ports:
  - port_id: "1"              # Port identifier (vendor-specific)
    mode: access              # Access mode (one VLAN)
    vlan: 10                  # VLAN ID
    description: User device  # Optional
    enabled: true             # Enable/disable port
    poe_enabled: false        # PoE on/off
    mac_notify: false         # MAC address change notifications (see SNMP section)
```

### Trunk Port

```yaml
ports:
  - port_id: "24"
    mode: trunk                      # Trunk mode (multiple VLANs)
    vlan: 10                         # Native VLAN (untagged)
    allowed_vlans: [10, 20, 30]      # All allowed VLANs
    description: Uplink to core
    enabled: true
    poe_enabled: false
    mac_notify: false                # MAC notifications (default: false)
```

**Important:** For trunk ports, `allowed_vlans` should include the native VLAN.

### Port Fields Reference

**Common Fields:**
- `port_id` - Port identifier (vendor-specific format, supports ranges like "1-5")
- `mode` - `access` or `trunk`
- `vlan` - Untagged/native VLAN, by **ID** (`vlan: 10`) or by **name** (`vlan: "Users"`)
- `tagged_vlans` (alias `allowed_vlans`) - Tagged VLANs, each by ID or name (trunk mode)
- `description` - Port description (optional)
- `enabled` - `true` to enable port, `false` to disable
- `poe_enabled` - `true` to enable Power over Ethernet, `false` to disable
- `mac_notify` - `true` to enable MAC address change notifications (requires global SNMP trap, see [SNMP Configuration](#snmp-configuration))
- `speed_duplex` - Port speed/duplex setting (e.g., `auto`, `100-full`, `1000-full`)

### Referencing VLANs by Name

Both `vlan` (untagged) and `tagged_vlans` accept a VLAN **name** as an alternative
to a numeric ID. The name is matched against the switch's defined `vlans` list.

```yaml
vlans:
  - { id: 10, name: Users }
  - { id: 30, name: Voice }
ports:
  - port_id: "15"
    vlan: "Users"                 # untagged VLAN by name → resolves to 10
    tagged_vlans: ["Voice", 40]   # names and IDs may be mixed; order preserved
```

Rules:
- **Type-strict:** a bare integer is always an ID (`vlan: 10`); a quoted string is
  always a name lookup (`vlan: "10"` looks up a VLAN *named* `10`, not ID 10).
- Name matching is **case-sensitive** and exact.
- VLAN names referenced by ports must be **unique**; a name mapping to two IDs is
  a hard error (ambiguous).
- An **unknown untagged** VLAN name is a hard error (the switch is skipped).
- An **unknown tagged** VLAN name is dropped with a warning on normal loads, or a
  hard error under `--strict-deployment`.

Names are resolved to numeric IDs at load time, so switches always receive
ordinary numeric VLAN commands — this is purely a config-authoring convenience.
See [`examples/vlan-by-name.yaml`](../../examples/vlan-by-name.yaml).

**Note:** For `mac_notify` to work, you must also enable the global `mac-notify` trap in the [SNMP configuration section](#snmp-configuration).

### Port ID Formats by Vendor

**Aruba:**
```yaml
port_id: "1"      # Simple numbers
port_id: "23"
port_id: "24"
```

**Cisco:**
```yaml
port_id: "GigabitEthernet1/0/1"
port_id: "TenGigabitEthernet1/1/1"
```

**FortiSwitch:**
```yaml
port_id: "port1"
port_id: "port24"
```

### Port Range Syntax

You can configure multiple ports with identical settings using port range syntax:

**Range (consecutive ports):**
```yaml
- port_id: "1-5"
  mode: access
  vlan: 20
  # Expands to ports: 1, 2, 3, 4, 5
```

**Comma-separated list:**
```yaml
- port_id: "7,9,11"
  mode: access
  vlan: 20
  # Expands to ports: 7, 9, 11
```

**Mixed syntax:**
```yaml
- port_id: "1-5,7,10-12"
  mode: trunk
  vlan: 10
  allowed_vlans: [10, 20]
  # Expands to ports: 1, 2, 3, 4, 5, 7, 10, 11, 12
```

**Vendor-specific formats:**
```yaml
# Aruba
port_id: "1-5"  # Ports 1 through 5

# Cisco
port_id: "GigabitEthernet1/0/1-5"  # GigabitEthernet1/0/1 through GigabitEthernet1/0/5

# FortiSwitch
port_id: "port1-8"  # port1 through port8
```

**Benefits:**
- **Concise configuration:** 1 line instead of 5+ for identical ports
- **Easy maintenance:** Change all similar ports at once
- **Less error-prone:** No copy-paste mistakes

**Examples:**

Access ports for users (PoE enabled):
```yaml
- port_id: "1-20"
  mode: access
  vlan: 20
  description: User workstations
  enabled: true
  poe_enabled: true
```

Disabled ports (security):
```yaml
- port_id: "13-22"
  mode: access
  vlan: 999
  description: Unused
  enabled: false
  poe_enabled: false
```

**Important Notes:**
- Port ranges are expanded at configuration load time
- Each port in the range becomes an individual port configuration
- All ports in a range will have identical settings
- You'll see log messages showing the expansion: `Port expansion: 3 config entries → 15 individual ports`

## Port Mirroring (SPAN)

Mirror traffic from one or more source ports to a destination port:

```yaml
port_mirrors:
  - session_id: "1"               # Session identifier
    source_ports: ["15"]          # Ports to monitor
    destination_port: "22"        # Port for analyzer
    direction: both               # rx, tx, or both
```

**Port ranges in mirrors:**
```yaml
port_mirrors:
  - session_id: "1"
    source_ports: ["1-5", "10"]   # Monitor ports 1,2,3,4,5,10
    destination_port: "22"
    direction: both
```

**Note:** The `destination_port` must be a single port. Source ports support range syntax.

### Multiple Source Ports

```yaml
port_mirrors:
  - session_id: "1"
    source_ports: ["1", "2", "5"]  # Multiple sources
    destination_port: "22"
    direction: both
```

### Direction Options

- `both` - Mirror both incoming (rx) and outgoing (tx) traffic
- `rx` - Mirror only incoming traffic
- `tx` - Mirror only outgoing traffic

**Note:** Aruba switches typically support only one mirror session at a time.

### Aruba Mirror Syntax (Technical Note)

Aruba switches use different command syntax depending on the model:

| Model Series | Running-Config Syntax | Example |
|--------------|----------------------|---------|
| 2530/2540 | `mirror-port <dest>` | `mirror-port 42` |
| 2930F+ | `mirror <id> port <dest>` | `mirror 1 port 42` |

The parser automatically detects both syntaxes when reading the switch's running configuration. Source ports are identified by the `monitor` command within interface blocks.

When applying configurations, the service always generates the newer `mirror 1 port <dest>` syntax, which is compatible with all supported Aruba models.

## SNMP Configuration

Configure SNMP for monitoring and management. SNMP settings include community strings (authentication), trap receivers (monitoring servers), and trap types (events to monitor).

### Basic SNMP Setup

```yaml
snmp:
  # Community strings for SNMP access
  communities:
    - name: "public"
      access: operator  # Read-only

  # Servers to receive SNMP traps
  trap_receivers:
    - host: "192.168.1.100"
      community: "public"
      version: "2c"

  # Types of traps to enable
  enabled_traps:
    - link-change    # Port up/down events
    - mac-notify     # MAC address changes
```

### SNMP Communities

Community strings control SNMP access to the switch:

```yaml
communities:
  - name: "public"
    access: operator      # Read-only access

  - name: "private"
    access: manager       # Read-write access

  - name: "admin"
    access: unrestricted  # Full administrative access
```

**Access Levels:**
- `operator` - Read-only (view configuration)
- `manager` - Read-write (modify configuration)
- `unrestricted` - Full administrative access

### SNMP Trap Receivers

Trap receivers are monitoring servers that receive SNMP notifications:

```yaml
trap_receivers:
  - host: "192.168.1.100"     # IP address of monitoring server
    community: "public"        # Community string to use
    version: "2c"              # SNMP version (optional, default: 2c)

  - host: "10.0.0.50"
    community: "monitoring"
    version: "2c"
```

**Multiple Receivers:**
You can configure multiple trap receivers to send notifications to different monitoring systems.

### Enabled Trap Types

Control which types of events generate SNMP traps:

```yaml
enabled_traps:
  - mac-notify       # MAC address learning/changes
  - link-change      # Port up/down events
  - authentication   # Authentication failures
  - stp              # Spanning Tree Protocol events
  - all              # Enable all trap types
```

**Common Trap Types:**
- `mac-notify` - Generates traps when MAC addresses are learned or removed
- `link-change` - Port status changes (up/down)
- `authentication` - SNMP authentication failures
- `stp` - Spanning Tree topology changes
- `all` - Enables all available trap types

### Per-Port MAC Notification Control

**NEW**: Individual ports can enable or disable MAC notification traps independently, even when the global `mac-notify` trap is enabled.

**Important:** Both settings must be enabled for traps to be sent:

| Global SNMP Trap | Port `mac_notify` | Result |
|------------------|-------------------|--------|
| ✅ Enabled | ✅ `true` | Port **SENDS** MAC notification traps |
| ✅ Enabled | ❌ `false` | Port **DOES NOT** send traps |
| ❌ Not enabled | ✅ `true` | Port **DOES NOT** send traps (global disabled) |
| ❌ Not enabled | ❌ `false` | Port **DOES NOT** send traps |

**Example: Selective MAC Tracking**

```yaml
snmp:
  communities:
    - name: "public"
      access: operator

  trap_receivers:
    - host: "192.168.1.100"
      community: "public"

  # Enable MAC-notify trap globally (required)
  enabled_traps:
    - mac-notify
    - link-change

ports:
  # Ports 1-10: Enable MAC tracking (guest/dynamic devices)
  - port_id: "1-10"
    mode: access
    vlan: 20
    mac_notify: true      # Send MAC change traps
    description: "Guest ports - MAC tracking enabled"

  # Ports 11-15: Disable MAC tracking (security cameras)
  - port_id: "11-15"
    mode: access
    vlan: 30
    mac_notify: false     # Don't send MAC change traps
    poe_enabled: true
    description: "Security cameras - stable devices"

  # Port 24: Uplink - no MAC tracking
  - port_id: "24"
    mode: trunk
    vlan: 10
    allowed_vlans: [10, 20, 30]
    mac_notify: false
    description: "Uplink"
```

**Use Cases:**
- **Enable on guest/dynamic ports** - Track devices that frequently connect/disconnect
- **Disable on stable devices** - Reduce noise from security cameras, printers, servers
- **Selective monitoring** - Only track specific VLANs or port groups

**How It Works:**
1. **Global trap** (`enabled_traps: [mac-notify]`) enables the SNMP trap type on the switch
2. **Per-port setting** (`mac_notify: true/false`) controls which specific ports send traps
3. Both must be true for a port to send MAC notification SNMP traps

**Default Behavior:**
- If `mac_notify` is omitted from a port, it defaults to `false` (no traps)
- You must explicitly set `mac_notify: true` on ports where you want MAC tracking

### Complete SNMP Example

```yaml
switches:
  - id: monitored-switch
    hostname: monitored-switch
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

      - id: 20
        name: users
        ip_config: none

    ports:
      # Monitored ports with MAC tracking
      - port_id: "1-5"
        mode: access
        vlan: 20
        enabled: true
        mac_notify: true    # Enable MAC tracking

      # Stable devices without MAC tracking
      - port_id: "6-10"
        mode: access
        vlan: 20
        enabled: true
        poe_enabled: true
        mac_notify: false   # Disable MAC tracking

    # SNMP Configuration
    snmp:
      communities:
        - name: "public"
          access: operator
        - name: "private"
          access: manager

      trap_receivers:
        - host: "192.168.1.100"
          community: "public"
          version: "2c"
        - host: "10.0.0.50"
          community: "monitoring"
          version: "2c"

      enabled_traps:
        - mac-notify       # Required for per-port mac_notify to work
        - link-change
        - authentication
```

**See Also:**
- [SNMP Trap Example](../../examples/snmp-trap-example.yaml) - Complete working example
- [Per-Port MAC Notify Example](../../examples/per-port-mac-notify-control.yaml) - Detailed MAC notification example

## Per-Switch Settings

Each switch can have its own settings:

```yaml
switches:
  - id: my-switch
    hostname: my-switch
    # ... other fields ...
    settings:
      ssh_timeout_secs: 30    # SSH connection timeout (default: 30)
      max_retries: 3          # Connection retry attempts (default: 3)
      enforce_port_config: false  # Reset unconfigured ports to defaults (default: false)
```

### enforce_port_config

When `true`, ports **not defined** in your config will be reset to defaults (disabled, VLAN 1). When `false` (default), only ports explicitly listed in your config are modified.

```yaml
settings:
  enforce_port_config: true  # Reset unlisted ports to defaults
```

**Note:** Use `--dry-run` CLI flag to preview changes without applying.

## Multi-Config Merging

Split your configuration across multiple YAML files for better organization. This is useful for:
- Separating credentials from version-controlled configs
- Sharing common VLANs/settings across multiple switches
- Per-team or per-application port configurations
- Environment-specific overrides (dev/staging/production)

### Basic Usage

```bash
# Single file (traditional)
switch-configurator --config-file config.yaml

# Multi-config: main file + one or more folders
switch-configurator --config-file main.yaml --config-folder ./common
switch-configurator --config-file main.yaml --config-folder ./common --config-folder ./overrides

# Preview merged result
switch-configurator --config-file main.yaml --config-folder ./common --show-merged-config
```

### The `id` Field (Required)

Every switch must have a unique `id` field. This is the key used to merge configurations:

```yaml
switches:
  - id: sw-office-01           # Required: unique identifier for merging
    hostname: aruba-office-01
    management_ip: "192.168.1.10"
    model: Aruba2930F
```

Switches with the **same `id`** across different files are merged together. Switches with **different `id`** values are treated as separate switches.

### Merge Priority

Control which config file "wins" when the same field is defined in multiple files using `merge_priority`:

```yaml
# main.yaml
merge_priority: 5    # Lower number = higher priority (wins)

switches:
  - id: sw-office-01
    hostname: aruba-office-01
    # ...
```

**Priority Rules:**
- Range: 0-9999 (lower number = higher priority)
- Main config file: can use 0-10, defaults to **50**
- Folder config files: must use 11+, defaults to **100**

**Example:**
```yaml
# main.yaml (priority 5 - highest, wins conflicts)
merge_priority: 5

# common/base.yaml (priority 100 - default for folders)
# No merge_priority specified, defaults to 100

# overrides/prod.yaml (priority 50 - between main and common)
merge_priority: 50
```

### How Merging Works

Merging uses **component replacement** (not field-level merging):

| Component | Merge Key | Behavior |
|-----------|-----------|----------|
| VLANs | `id` | Entire VLAN replaced by highest priority |
| Ports | `port_id` | Entire port config replaced by highest priority |
| Port Mirrors | `session_id` | Entire mirror replaced by highest priority |
| SNMP | sub-lists | Each list (`communities`, `trap_receivers`, `enabled_traps`) replaced as whole |
| Credentials | - | Entire object replaced by highest priority |
| Settings | - | Entire object replaced by highest priority |

### Example: Modular Configuration

**Directory structure:**
```
switch-config/
├── main.yaml              # Credentials + identity (priority 5)
├── common/
│   ├── vlans.yaml         # Standard VLANs (priority 100)
│   └── snmp.yaml          # SNMP settings (priority 100)
└── sites/
    └── office-ports.yaml  # Site-specific ports (priority 100)
```

**main.yaml** (credentials + switch identity):
```yaml
merge_priority: 5

switches:
  - id: sw-office-01
    hostname: aruba-office-01
    management_ip: "192.168.1.10"
    model: Aruba2930F
    credentials:
      username: admin
      password: secret123
      connection_type: ssh
```

**common/vlans.yaml** (shared VLANs):
```yaml
# Uses default priority 100
switches:
  - id: sw-office-01
    # Only id required - identity fields inherited from main.yaml
    vlans:
      - id: 10
        name: management
        ip_config: dhcp
      - id: 20
        name: users
      - id: 100
        name: guest
```

**sites/office-ports.yaml** (site-specific ports):
```yaml
switches:
  - id: sw-office-01
    ports:
      - port_id: "1-20"
        mode: access
        vlan: 20
        poe_enabled: true
      - port_id: "24"
        mode: trunk
        vlan: 10
        allowed_vlans: [10, 20, 100]
```

**Run with:**
```bash
switch-configurator --config-file main.yaml \
  --config-folder ./common \
  --config-folder ./sites \
  --one-off --dry-run
```

**Result:** Switch `sw-office-01` gets:
- Credentials from `main.yaml` (priority 5)
- VLANs from `common/vlans.yaml` (priority 100)
- Ports from `sites/office-ports.yaml` (priority 100)

### Overriding Values

Higher priority configs override lower priority ones:

**common/vlans.yaml** (priority 100):
```yaml
switches:
  - id: sw-office-01
    vlans:
      - id: 10
        name: management
        ip_config: dhcp
```

**overrides/production.yaml** (priority 50 - higher than 100):
```yaml
merge_priority: 50

switches:
  - id: sw-office-01
    vlans:
      - id: 10
        name: mgmt-prod           # Overrides "management"
        ip_config:
          address: "192.168.10.1"  # Overrides dhcp
          netmask: "255.255.255.0"
```

**Result:** VLAN 10 uses the production.yaml version (priority 50 beats 100).

### Identity Field Validation

These fields only need to exist in **one** config file per switch:
- `hostname`
- `management_ip`
- `model`
- `credentials`

If present in multiple files, values **must match exactly** or you'll get a conflict error.

### Folder Scanning

- Scans for `*.yaml` and `*.yml` files
- Files loaded in **alphabetical order**
- Use numeric prefixes for explicit ordering: `00-base.yaml`, `10-vlans.yaml`, `20-ports.yaml`

### Debugging

```bash
# Show final merged configuration
switch-configurator --config-file main.yaml --config-folder ./common --show-merged-config

# Enable debug logging to see merge process
switch-configurator --config-file main.yaml --config-folder ./common --log-level debug
```

### Common Patterns

**Pattern 1: Credentials separate from config**
```
config/
├── main.yaml           # Just credentials (gitignored)
└── switches/           # All switch configs (version controlled)
    └── office.yaml
```

**Pattern 2: Shared base + per-switch overrides**
```
config/
├── main.yaml           # All switches with credentials
├── common/             # Shared VLANs, SNMP
└── switches/           # Per-switch port configs
```

**Pattern 3: Environment-specific**
```
config/
├── main.yaml
├── common/
├── env/
│   ├── dev.yaml        # merge_priority: 50
│   └── prod.yaml       # merge_priority: 50
```

## Complete Example

```yaml
switches:
  - id: aruba-core-01           # Required unique identifier
    hostname: aruba-core-01
    model: Aruba2930F
    management_ip: "192.168.1.10"

    credentials:
      username: admin
      password: secure-password
      connection_type: ssh

    vlans:
      # Management VLAN with DHCP
      - id: 10
        name: management
        description: Management VLAN
        ip_config: dhcp

      # User VLAN with static gateway
      - id: 20
        name: users
        description: User workstations
        ip_config:
          address: "10.0.20.1"
          netmask: "255.255.255.0"

      # Server VLAN with static gateway
      - id: 30
        name: servers
        description: Production servers
        ip_config:
          address: "10.0.30.1"
          netmask: "255.255.255.0"

      # Guest VLAN (Layer 2 only)
      - id: 100
        name: guest
        description: Guest WiFi
        ip_config: none

    ports:
      # Access port for management
      - port_id: "1"
        mode: access
        vlan: 10
        description: Management console
        enabled: true
        poe_enabled: false

      # Access ports for users (with PoE)
      - port_id: "2"
        mode: access
        vlan: 20
        description: User workstation
        enabled: true
        poe_enabled: true

      - port_id: "3"
        mode: access
        vlan: 20
        description: User workstation
        enabled: true
        poe_enabled: true

      # Trunk port for uplink
      - port_id: "23"
        mode: trunk
        vlan: 10
        allowed_vlans: [10, 20, 30, 100]
        description: Uplink to core
        enabled: true
        poe_enabled: false

      # Trunk port for access point
      - port_id: "24"
        mode: trunk
        vlan: 20
        allowed_vlans: [20, 100]
        description: Wireless AP
        enabled: true
        poe_enabled: true

    port_mirrors:
      # Mirror server traffic for monitoring
      - session_id: "1"
        source_ports: ["5"]
        destination_port: "22"
        direction: both

    settings:
      ssh_timeout_secs: 30
      max_retries: 3
```

## Configuration Best Practices

### 1. Use Descriptive Names

```yaml
# Good
vlans:
  - id: 10
    name: management
    description: Network management and monitoring

# Less helpful
vlans:
  - id: 10
    name: vlan10
```

### 2. Document Port Assignments

```yaml
ports:
  - port_id: "1"
    mode: access
    vlan: 10
    description: Server-01 eth0  # Which device is connected
```

### 3. Group Related Configuration

```yaml
# Group VLANs by function
vlans:
  # Infrastructure VLANs
  - id: 10
    name: management
  - id: 11
    name: monitoring

  # User VLANs
  - id: 20
    name: users
  - id: 21
    name: contractors
```

### 4. Use Consistent Naming

Pick a naming scheme and stick to it:
- `mgmt`, `user`, `guest` OR
- `management`, `users`, `guests`

### 5. Version Control

Keep your configs in git:

```bash
git init
git add config.yaml
git commit -m "Initial switch configuration"
```

### 6. Separate Credentials

Don't commit passwords to git. Consider:

```yaml
# config.yaml (committed to git)
credentials:
  username: admin
  ssh_key_path: /etc/switch-configurator/ssh_key
  connection_type: ssh
```

Or use environment variables and template the config.

### 7. Test Before Production

```bash
# Always test with dry-run first
switch-configurator --config-file config.yaml --one-off --dry-run

# Then apply
switch-configurator --config-file config.yaml --one-off
```

## Validation

The configurator validates your configuration on startup:

- VLAN IDs must be between 1-4094
- Port IDs must match vendor format
- IP addresses must be valid
- Required fields must be present

**Example validation error:**
```
Error: VLAN ID 5000 is invalid (must be 1-4094)
```

## Examples

See the [examples directory](../../examples/) for complete, working configurations:

- [aruba-serial.yaml](../../examples/aruba-serial.yaml) - Serial connection
- [aruba-ssh.yaml](../../examples/aruba-ssh.yaml) - SSH connection
- [multi-vendor.yaml](../../examples/multi-vendor.yaml) - Mixed vendors
- [vlan-ip-configs.yaml](../../examples/vlan-ip-configs.yaml) - All IP config modes
- [port-mirroring.yaml](../../examples/port-mirroring.yaml) - SPAN configuration

## Next Steps

- **[Examples](../../examples/README.md)** - Complete configuration examples
- **[CLAUDE.md](../../CLAUDE.md)** - Detailed architecture and field reference
- **[Troubleshooting](troubleshooting.md)** - Common configuration issues
