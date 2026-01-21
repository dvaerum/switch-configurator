# REST API Reference

The Switch Configurator provides a REST API for programmatic configuration management and monitoring.

## Base URL

By default, the API runs on port `4002`. This can be configured via the `--port` flag.

```
http://<host>:4002
```

## Authentication

Currently, the API does not implement authentication. It should be deployed behind a reverse proxy with appropriate authentication and authorization if exposed externally.

## Response Format

All responses are in JSON format. Successful responses include relevant data, while error responses follow this structure:

```json
{
  "error": "Error message description"
}
```

---

## Endpoints

### Health Check

Check if the service is running.

**Endpoint:** `GET /health`

**Response:** `200 OK`

```json
{
  "status": "ok",
  "service": "switch-configurator"
}
```

**Example:**

```bash
curl http://localhost:4002/health
```

---

### Service Status

Get detailed service status including recent operations, errors, and configuration metadata.

**Endpoint:** `GET /api/status`

**Response:** `200 OK`

```json
{
  "service": "switch-configurator",
  "version": "0.3.19",
  "status": "healthy",
  "uptime_seconds": 3600,
  "currently_configuring": [],
  "pending_config_reload": [],
  "configuration": {
    "loaded": true,
    "config_file": "/etc/switch-configurator/config.yaml",
    "switches_count": 5
  },
  "switches": [...],
  "recent_errors": []
}
```

**Key Fields:**
- `currently_configuring`: Array of switch IDs actively being configured (empty when idle)
- `pending_config_reload`: Array of switch IDs with queued config reloads from file watcher (waiting)

**Example:**

```bash
curl http://localhost:4002/api/status
```

---

### List Switches

List all configured switches with summary information.

**Endpoint:** `GET /switches`

**Response:** `200 OK`

```json
{
  "switches": [
    {
      "id": "aruba-office-01",
      "hostname": "aruba-switch-01",
      "model": "Aruba2930F",
      "management_ip": "192.168.1.10",
      "vlans": 5,
      "ports": 24
    },
    {
      "id": "cisco-core-01",
      "hostname": "cisco-core",
      "model": "CiscoCatalyst9300_24P_UPOE",
      "management_ip": "192.168.1.20",
      "vlans": 10,
      "ports": 24
    }
  ],
  "count": 2
}
```

**Fields:**
- `id` (string): Unique switch identifier used in configuration
- `hostname` (string): Switch hostname
- `model` (string): Switch model identifier
- `management_ip` (string): Management IP address
- `vlans` (number): Number of configured VLANs
- `ports` (number): Number of configured ports

**Example:**

```bash
curl http://localhost:4002/switches
```

---

### Apply Configuration (Async)

Apply the **in-memory** configuration to a specific switch. This does NOT re-read YAML files - it uses the configuration already loaded in memory.

**⚡ This endpoint is always asynchronous** - it returns immediately with `202 Accepted` and processes the configuration in the background. Poll `/api/status` to monitor progress.

**What it does:**
1. Validates switch exists in memory
2. Returns `202 Accepted` immediately
3. In background: Connects to the switch (SSH or serial)
4. In background: Retrieves current running configuration from the switch
5. In background: Computes diff between current and desired state
6. In background: Applies only necessary changes to the switch's running-config
7. In background: Saves to switch's startup-config (`write memory` - does NOT modify YAML files)

**When to use:**
- Re-push config to a switch that rebooted and lost its running config
- Retry after a failed apply attempt
- Apply to one specific switch without affecting others

**See also:** `POST /config/reload` to reload YAML from disk first.

**Endpoint:** `POST /switches/{id}/apply`

**Path Parameters:**
- `id` (string, required): Switch ID (from config `id` field - the unique identifier for multi-config merging)

**Response:**

**Accepted:** `202 ACCEPTED` - Configuration apply started

```json
{
  "status": "accepted",
  "message": "Configuration apply started for switch 'aruba-office-01'",
  "switch_id": "aruba-office-01",
  "poll_url": "/api/status",
  "hint": "Poll /api/status and check 'currently_configuring' array. When empty [], check switches[].last_result for the outcome."
}
```

**Errors:**

- `404 NOT FOUND` - Switch not found
```json
{
  "error": "Switch with id 'unknown-id' not found"
}
```

- `409 CONFLICT` - Switch is busy (being configured or has pending reload)
```json
{
  "error": "Switch 'switch-01' is already being configured",
  "switch_id": "switch-01"
}
```
or
```json
{
  "error": "Switch 'switch-01' has a pending config reload queued",
  "switch_id": "switch-01"
}
```

**Note:** The conflict detection is **per-switch** - you can configure multiple different switches in parallel. A switch is considered "busy" if it's either being configured OR has a pending config reload queued by the file watcher.

**Monitoring Progress:**

Poll `/api/status` to check progress:
- `currently_configuring`: Array of switch IDs currently being configured (empty when idle)
- `pending_config_reload`: Array of switch IDs with queued reloads from file watcher
- `switches[].last_result`: Shows the outcome after completion

```bash
# Poll status
curl http://localhost:4002/api/status | jq '{currently_configuring, switches: [.switches[] | {id, last_result}]}'
```

**Example:**

```bash
# Start async apply
curl -X POST http://localhost:4002/switches/aruba-office-01/apply
# Returns immediately with 202 Accepted

# Poll for completion
while true; do
  STATUS=$(curl -s http://localhost:4002/api/status)
  CONFIGURING=$(echo "$STATUS" | jq -r '.currently_configuring | length')
  if [ "$CONFIGURING" = "0" ]; then
    echo "Done! Checking result..."
    echo "$STATUS" | jq '.switches[] | select(.id == "aruba-office-01") | .last_result'
    break
  fi
  echo "Still configuring $CONFIGURING switch(es)..."
  sleep 2
done
```

**Notes:**
- This is an idempotent operation - running it multiple times will only apply changes when the current state differs from desired state
- The operation can take several seconds to minutes depending on switch response time and number of changes
- If connection or configuration fails, the switch state is left unchanged
- Configuration is automatically saved after successful application
- Multiple different switches can be configured in parallel
- Concurrent applies to the same switch return `409 Conflict`
- **File watcher integration**: If the file watcher is applying to a switch or has a pending reload queued, the API returns `409 Conflict` for that switch
- **Pending queue**: When file watcher triggers a reload for a busy switch, the reload is queued (max 1 per switch). Newer config changes replace older pending reloads.

---

### Reload and Apply Configuration (Single Switch)

Reload configuration from YAML files on disk and apply to a **specific switch** only.

**⚡ This endpoint is always asynchronous** - it returns immediately with `202 Accepted` and processes the configuration in the background. Poll `/api/status` to monitor progress.

**What it does:**
1. Validates switch is not busy (being configured or has pending reload)
2. Re-reads all YAML configuration files from disk
3. Merges configs (if using multi-config mode)
4. Updates in-memory config for the specified switch only
5. Returns `202 Accepted` immediately
6. In background: Applies configuration to the switch

**When to use:**
- After manually editing YAML files and want to apply to just one switch
- To reload and apply config for one switch without affecting others
- Testing configuration changes on a single switch before rolling out to all

**Difference from other endpoints:**
| Endpoint | Reads YAML? | Applies to | Use case |
|----------|-------------|------------|----------|
| `POST /switches/{id}/apply` | No | One switch | Re-push in-memory config |
| `POST /switches/{id}/reload` | Yes | One switch | Reload YAML and apply to one |
| `POST /config/reload` | Yes | All switches | Reload YAML and apply to all |

**Endpoint:** `POST /switches/{id}/reload`

**Path Parameters:**
- `id` (string, required): Switch ID (from config `id` field)

**Response:**

**Accepted:** `202 ACCEPTED` - Configuration reload and apply started

```json
{
  "status": "accepted",
  "message": "Configuration reload and apply started for switch 'aruba-office-01'",
  "switch_id": "aruba-office-01",
  "poll_url": "/api/status",
  "hint": "Poll /api/status and check 'currently_configuring' array. When empty or switch not in list, check switches[].last_result for the outcome."
}
```

**Errors:**

- `404 NOT FOUND` - Switch not found in configuration files
```json
{
  "error": "Switch with id 'unknown-id' not found in configuration files"
}
```

- `409 CONFLICT` - Switch is busy (being configured or has pending reload)
```json
{
  "error": "Switch 'switch-01' is already being configured",
  "switch_id": "switch-01"
}
```
or
```json
{
  "error": "Switch 'switch-01' has a pending config reload queued",
  "switch_id": "switch-01"
}
```

- `500 INTERNAL SERVER ERROR` - Configuration reload failed
```json
{
  "error": "Failed to reload configuration: Parse error at line 10"
}
```

**Example:**

```bash
# Reload YAML and apply to single switch
curl -X POST http://localhost:4002/switches/aruba-office-01/reload

# Poll for completion
while true; do
  STATUS=$(curl -s http://localhost:4002/api/status)
  CONFIGURING=$(echo "$STATUS" | jq -r '.currently_configuring | length')
  if [ "$CONFIGURING" = "0" ]; then
    echo "Done! Checking result..."
    echo "$STATUS" | jq '.switches[] | select(.id == "aruba-office-01") | .last_result'
    break
  fi
  echo "Still configuring..."
  sleep 2
done
```

---

### Get Running Configuration

Retrieve the current running configuration from a switch, including both raw config text and parsed state.

**Endpoint:** `GET /switches/{id}/config`

**Path Parameters:**
- `id` (string, required): Switch ID (from config `id` field)

**Response:**

**Success:** `200 OK`

```json
{
  "id": "aruba-office-01",
  "hostname": "aruba-switch-01",
  "model": "Aruba2930F",
  "management_ip": "192.168.1.10",
  "raw_config": "Running configuration:\n\n; J9779A Configuration Editor...\nhostname \"aruba-switch-01\"\nvlan 1\n   name \"DEFAULT_VLAN\"\n...",
  "parsed_state": {
    "vlans": [
      {"id": 1, "name": "DEFAULT_VLAN", "description": null, "ip_config": "none"},
      {"id": 10, "name": "management", "description": null, "ip_config": "dhcp"}
    ],
    "ports": [
      {"port_id": "1", "mode": "access", "vlan": 10, "allowed_vlans": [], "enabled": true, "poe_enabled": false, "mac_notify": false, "speed_duplex": "auto"}
    ],
    "port_mirrors": [],
    "snmp": {
      "communities": [{"name": "public", "access": "operator"}],
      "trap_receivers": [],
      "enabled_traps": []
    },
    "management_vlan": 10
  }
}
```

**Response Fields:**
- `id` (string): Switch identifier
- `hostname` (string): Switch hostname
- `model` (string): Switch model (e.g., `Aruba2930F`)
- `management_ip` (string): Management IP address
- `raw_config` (string): Raw `show running-config` output from the switch (vendor-specific format)
- `parsed_state` (object|null): Structured representation of the switch state, or `null` if parsing failed
  - `vlans`: Array of VLAN configurations
  - `ports`: Array of port configurations
  - `port_mirrors`: Array of port mirror/SPAN sessions
  - `snmp`: SNMP configuration (communities, traps, receivers)
  - `management_vlan`: Management VLAN ID if configured

**Errors:**

- `404 NOT FOUND` - Switch not found
```json
{
  "error": "Switch with id 'unknown-id' not found"
}
```

- `409 CONFLICT` - Switch is currently being configured (apply in progress)
```json
{
  "error": "Switch 'switch-01' is currently being configured",
  "switch_id": "switch-01"
}
```

- `500 INTERNAL SERVER ERROR` - Connection or retrieval failed
```json
{
  "error": "Connection failed: SSH connection timeout"
}
```

**Example:**

```bash
# Get running configuration with parsed state
curl http://localhost:4002/switches/aruba-office-01/config | jq

# Extract just the parsed VLANs
curl http://localhost:4002/switches/aruba-office-01/config | jq '.parsed_state.vlans'

# Extract just the parsed ports
curl http://localhost:4002/switches/aruba-office-01/config | jq '.parsed_state.ports'

# Save raw config to file for backup
curl http://localhost:4002/switches/aruba-office-01/config | jq -r '.raw_config' > backup.conf

# Compare desired vs actual VLANs
echo "=== Desired ===" && curl -s http://localhost:4002/switches/aruba-office-01/desired-config | jq '.vlans'
echo "=== Actual ===" && curl -s http://localhost:4002/switches/aruba-office-01/config | jq '.parsed_state.vlans'
```

**Notes:**
- This retrieves the **actual** running configuration from the switch hardware, not the desired configuration from YAML
- `parsed_state` has the same structure as `/switches/{id}/desired-config`, making comparison easy
- If parsing fails, `parsed_state` will be `null` but `raw_config` will still be available
- Useful for backup, verification, troubleshooting, and comparing desired vs actual state
- The `raw_config` format is vendor-specific (Aruba CLI, Cisco IOS, FortiSwitch CLI)
- Returns `409 Conflict` if the switch is busy (being configured or has pending reload queued)
- **File watcher integration**: Also returns `409 Conflict` if the file watcher is applying to this switch or has a pending reload queued

---

### Reload Configuration (Global)

Reload configuration from YAML files on disk and apply to **all** switches.

**⚡ This endpoint is always asynchronous** - it returns immediately with `202 Accepted` and processes configuration in the background. Poll `/api/status` to monitor progress.

**Config flow:**
```
YAML files (disk) --reload--> Memory --apply--> All switches (hardware)
```

**What it does:**
1. Re-reads all YAML configuration files from disk
2. Merges configs (if using multi-config mode)
3. Validates merged configuration
4. Updates in-memory config
5. Returns `202 Accepted` immediately
6. In background: Spawns parallel tasks to apply configuration to all switches

**When to use:**
- After manually editing YAML files (if file watcher is disabled)
- To force a full reload and re-apply to all switches
- After recovering from a config validation error

**See also:** `POST /switches/{id}/apply` to apply to just one switch without reloading.

**Note:** The file watcher automatically performs this operation when YAML files change on disk.

**Endpoint:** `POST /config/reload`

**Response:** `202 ACCEPTED`

```json
{
  "status": "accepted",
  "message": "Configuration reload started",
  "switches_configuring": ["switch-01", "switch-02"],
  "switches_skipped": []
}
```

**Response Fields:**
- `switches_configuring`: Array of switch IDs where configuration apply was started
- `switches_skipped`: Array of switch IDs that were skipped (already being configured)

**Monitoring Progress:**

Poll `/api/status` to check progress:
- `currently_configuring`: Array of switch IDs currently being configured (empty when idle)
- `switches[].last_result`: Shows the outcome after completion

```bash
# Poll status
curl http://localhost:4002/api/status | jq '{currently_configuring, switches: [.switches[] | {id, last_result}]}'
```

**Error Responses:**

- `500 INTERNAL SERVER ERROR` - Configuration file error or missing config paths

```json
{
  "error": "Switch ID 'orphan-switch' (from orphan.yaml) missing required fields: hostname, model. Hint: Check if switch ID matches between main config and folder configs."
}
```

```json
{
  "error": "Configuration paths not set - use --config-file to specify configuration"
}
```

**Example:**

```bash
# Reload and apply to all switches (async)
curl -X POST http://localhost:4002/config/reload

# Poll for completion
while true; do
  STATUS=$(curl -s http://localhost:4002/api/status)
  CONFIGURING=$(echo "$STATUS" | jq -r '.currently_configuring | length')
  if [ "$CONFIGURING" = "0" ]; then
    echo "Done! All switches configured."
    echo "$STATUS" | jq '.switches[] | {id, last_result}'
    break
  fi
  echo "Still configuring $CONFIGURING switch(es)..."
  sleep 2
done
```

---

### Get Desired Configuration

Get the in-memory desired configuration for a specific switch. This returns the configuration stored in memory (loaded from YAML), NOT the actual running configuration from the switch hardware.

**Endpoint:** `GET /switches/{id}/desired-config`

**Path Parameters:**
- `id` (string, required): Switch ID

**Response:** `200 OK`

```json
{
  "id": "aruba-office-01",
  "hostname": "aruba-switch-01",
  "model": "Aruba2930F",
  "management_ip": "192.168.1.10",
  "vlans": [
    {"id": 10, "name": "management"},
    {"id": 100, "name": "users"}
  ],
  "ports": [
    {"port_id": "1", "mode": "access", "vlan": 100, "enabled": true}
  ],
  "port_mirrors": [],
  "snmp": null
}
```

**Error Response:** `404 NOT FOUND`

```json
{
  "error": "Switch 'unknown-id' not found"
}
```

**Example:**

```bash
curl http://localhost:4002/switches/aruba-office-01/desired-config | jq
```

---

### Set Switch Configuration (Create/Replace)

Create a new switch configuration or completely replace an existing one in memory.

**Endpoint:** `PUT /switches/{id}/desired-config`

**Path Parameters:**
- `id` (string, required): Switch ID - MUST match the `id` field in the request body

**Request Body:**

```json
{
  "id": "new-switch-01",
  "hostname": "new-switch",
  "model": "Aruba2930F",
  "management_ip": "192.168.1.100",
  "credentials": {
    "username": "admin",
    "password": "secret"
  },
  "vlans": [
    {"id": 10, "name": "management"}
  ],
  "ports": [
    {"port_id": "1", "mode": "access", "vlan": 10, "enabled": true}
  ]
}
```

**Required fields for NEW switches:**
- `id` - Switch identifier (must match URL)
- `hostname` - Switch hostname
- `model` - Switch model (e.g., `Aruba2930F`, `CiscoCatalyst9300_24P_UPOE`)
- `management_ip` - Management IP address
- `credentials` - Authentication credentials

**Response:**

- `201 CREATED` - New switch created
- `200 OK` - Existing switch replaced

```json
{
  "status": "ok",
  "message": "Switch 'new-switch-01' created",
  "switch_id": "new-switch-01"
}
```

**Error Responses:**

- `400 BAD REQUEST` - ID mismatch or missing required fields

```json
{
  "error": "ID mismatch: URL parameter 'sw-01' does not match request body id 'sw-02'"
}
```

```json
{
  "error": "New switch 'new-switch-01' requires fields: hostname, model, management_ip, credentials"
}
```

**Example:**

```bash
# Create a new switch
curl -X PUT http://localhost:4002/switches/new-switch-01/desired-config \
  -H "Content-Type: application/json" \
  -d '{
    "id": "new-switch-01",
    "hostname": "new-switch",
    "model": "Aruba2930F",
    "management_ip": "192.168.1.100",
    "credentials": {"username": "admin", "password": "secret"},
    "vlans": [{"id": 10, "name": "default"}]
  }'

# Replace existing switch config
curl -X PUT http://localhost:4002/switches/aruba-office-01/desired-config \
  -H "Content-Type: application/json" \
  -d @switch-config.json
```

---

### Patch Switch Configuration (Partial Update)

Update specific fields of an existing switch configuration. Merges changes into the existing config.

**Endpoint:** `PATCH /switches/{id}/desired-config`

**Path Parameters:**
- `id` (string, required): Switch ID - MUST match the `id` field in the request body

**Merge Behavior:**
- **Simple fields** (hostname, model, etc.): Replaced if provided
- **vlans**: Merged by VLAN `id` - add new, update existing
- **ports**: Merged by `port_id` - add new, update existing
- **port_mirrors**: Merged by `session_id` - add new, update existing
- **snmp**: Replaced entirely if provided

**Request Body:**

```json
{
  "id": "aruba-office-01",
  "vlans": [
    {"id": 200, "name": "new-vlan"}
  ],
  "ports": [
    {"port_id": "5", "mode": "access", "vlan": 200, "enabled": true}
  ]
}
```

**Response:** `200 OK`

```json
{
  "status": "ok",
  "message": "Switch 'aruba-office-01' config updated",
  "switch_id": "aruba-office-01"
}
```

**Error Responses:**

- `400 BAD REQUEST` - ID mismatch
- `404 NOT FOUND` - Switch doesn't exist (use PUT to create)

```json
{
  "error": "Switch 'unknown-id' not found. Use PUT to create new switches."
}
```

**Examples:**

```bash
# Add a new VLAN
curl -X PATCH http://localhost:4002/switches/aruba-office-01/desired-config \
  -H "Content-Type: application/json" \
  -d '{
    "id": "aruba-office-01",
    "vlans": [{"id": 300, "name": "guest-wifi"}]
  }'

# Update hostname
curl -X PATCH http://localhost:4002/switches/aruba-office-01/desired-config \
  -H "Content-Type: application/json" \
  -d '{"id": "aruba-office-01", "hostname": "new-hostname"}'

# Add port configuration
curl -X PATCH http://localhost:4002/switches/aruba-office-01/desired-config \
  -H "Content-Type: application/json" \
  -d '{
    "id": "aruba-office-01",
    "ports": [{"port_id": "10", "mode": "trunk", "vlan": 1, "allowed_vlans": [10, 20, 30]}]
  }'
```

---

### Delete Switch Configuration

Remove a switch from the in-memory configuration.

**Endpoint:** `DELETE /switches/{id}/desired-config`

**Path Parameters:**
- `id` (string, required): Switch ID

**Response:** `200 OK`

```json
{
  "status": "ok",
  "message": "Switch 'aruba-office-01' deleted"
}
```

**Error Response:** `404 NOT FOUND`

```json
{
  "error": "Switch 'unknown-id' not found"
}
```

**Example:**

```bash
curl -X DELETE http://localhost:4002/switches/old-switch-01/desired-config
```

**Note:** This only removes the switch from in-memory config. It does NOT:
- Modify the switch hardware
- Delete any YAML configuration files

---

## Error Handling

All endpoints return appropriate HTTP status codes:

- `200 OK` - Request successful
- `404 NOT FOUND` - Resource not found (switch not in configuration)
- `500 INTERNAL SERVER ERROR` - Server error (connection failure, configuration error, etc.)

Error responses include a JSON body with an `error` field describing the issue:

```json
{
  "error": "Detailed error message"
}
```

---

## Common Use Cases

### Check Service Health

```bash
# Quick health check
curl http://localhost:4002/health

# Detailed status with recent operations
curl http://localhost:4002/api/status | jq
```

### List All Switches

```bash
# Get all switches
curl http://localhost:4002/switches | jq

# Extract just switch IDs
curl http://localhost:4002/switches | jq -r '.switches[].id'

# Extract just hostnames
curl http://localhost:4002/switches | jq -r '.switches[].hostname'

# Find switches by model
curl http://localhost:4002/switches | jq '.switches[] | select(.model == "Aruba2930F")'
```

### Apply Configuration (Async)

```bash
# Apply to single switch (returns 202 immediately)
curl -X POST http://localhost:4002/switches/aruba-office-01/apply

# Apply and wait for completion
apply_and_wait() {
  local switch_id=$1

  # Start apply
  RESPONSE=$(curl -s -w "%{http_code}" -X POST "http://localhost:4002/switches/$switch_id/apply")
  HTTP_CODE="${RESPONSE: -3}"

  if [ "$HTTP_CODE" = "409" ]; then
    echo "Another apply in progress, waiting..."
    sleep 5
    apply_and_wait "$switch_id"
    return
  fi

  if [ "$HTTP_CODE" != "202" ]; then
    echo "Failed to start apply: HTTP $HTTP_CODE"
    return 1
  fi

  # Poll for completion
  while true; do
    STATUS=$(curl -s http://localhost:4002/api/status)
    CONFIGURING=$(echo "$STATUS" | jq -r '.currently_configuring | length')
    if [ "$CONFIGURING" = "0" ]; then
      echo "Apply complete for $switch_id"
      echo "$STATUS" | jq ".switches[] | select(.id == \"$switch_id\") | .last_result"
      return 0
    fi
    sleep 2
  done
}

apply_and_wait "aruba-office-01"
```

### Backup Configurations

```bash
# Backup single switch by ID
curl -s http://localhost:4002/switches/aruba-office-01/config | \
  jq -r '.config' > backups/aruba-office-01-$(date +%Y%m%d).conf

# Backup all switches (using ID for filename)
mkdir -p backups
for switch_id in $(curl -s http://localhost:4002/switches | jq -r '.switches[].id'); do
  echo "Backing up $switch_id..."
  curl -s "http://localhost:4002/switches/$switch_id/config" | \
    jq -r '.config' > "backups/$switch_id-$(date +%Y%m%d).conf"
done
```

### Monitor for Changes

```bash
# Watch for configuration changes
watch -n 30 'curl -s http://localhost:4002/switches | jq'

# Monitor recent errors
watch -n 10 'curl -s http://localhost:4002/api/status | jq .recent_errors'
```

### Integration with CI/CD

```bash
#!/bin/bash
# Apply configuration and verify (async-aware)

SWITCH_ID=${SWITCH_ID:-"aruba-office-01"}
TIMEOUT=300  # 5 minutes max

# Start async apply
RESPONSE=$(curl -s -w "%{http_code}" -X POST "http://localhost:4002/switches/$SWITCH_ID/apply")
HTTP_CODE="${RESPONSE: -3}"

if [ "$HTTP_CODE" = "404" ]; then
  echo "ERROR: Switch not found: $SWITCH_ID"
  exit 1
fi

if [ "$HTTP_CODE" = "409" ]; then
  echo "ERROR: Another configuration is in progress"
  exit 1
fi

if [ "$HTTP_CODE" != "202" ]; then
  echo "ERROR: Unexpected response: HTTP $HTTP_CODE"
  exit 1
fi

echo "Apply started for $SWITCH_ID, polling for completion..."

# Poll for completion
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT ]; do
  STATUS=$(curl -s http://localhost:4002/api/status)
  CONFIGURING=$(echo "$STATUS" | jq -r '.currently_configuring | length')

  if [ "$CONFIGURING" = "0" ]; then
    # Check result
    RESULT=$(echo "$STATUS" | jq -r ".switches[] | select(.id == \"$SWITCH_ID\") | .last_result")
    if [ -n "$RESULT" ] && [ "$RESULT" != "null" ]; then
      echo "Configuration completed for $SWITCH_ID"
      echo "Result: $RESULT"
      exit 0
    fi
  fi

  sleep 5
  ELAPSED=$((ELAPSED + 5))
done

echo "ERROR: Timeout waiting for configuration to complete"
exit 1
```

---

## Rate Limiting

Currently, there is no rate limiting implemented. When deploying in production:

1. Use a reverse proxy (nginx, Caddy) with rate limiting
2. Implement authentication/authorization at the proxy level
3. Consider switch connection limits (serial connections are exclusive)

---

## Security Considerations

⚠️ **Important Security Notes:**

1. **No Built-in Authentication**: The API has no authentication. Deploy behind a reverse proxy with authentication if exposed to networks beyond localhost.

2. **Credentials in Memory**: Switch credentials are stored in memory during runtime. Ensure proper access controls on the host system.

3. **Serial Device Access**: When using serial connections, the service needs appropriate permissions (dialout group membership on Linux).

4. **SSH Host Key Verification**: Currently simplified (accepts all). Consider implementing proper host key verification for production use.

5. **Configuration File Permissions**: Ensure configuration files containing passwords have restrictive permissions (chmod 600).

---

## Future Enhancements

Planned API improvements:

- [ ] Authentication and authorization (API tokens, mTLS)
- [ ] WebSocket support for real-time status updates
- [ ] Bulk operations endpoint (apply to multiple switches)
- [ ] Configuration validation endpoint (dry-run mode)
- [ ] Scheduled configuration application
- [ ] Configuration diff preview endpoint
- [ ] Switch discovery endpoint
- [ ] Metrics endpoint (Prometheus format)
- [x] ~~Configuration API (PUT/PATCH/DELETE)~~ - Implemented!
