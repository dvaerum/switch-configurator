# Switch Configuration Validation Design

## Overview

Multi-layered validation system to ensure switch configurations are safe before being persisted.

## Validation Layers

### Layer 1: Pre-Application Validation (Already Exists)
- ✅ YAML schema validation
- ✅ Port range validation
- ✅ VLAN ID validation (1-4094)
- ✅ Duplicate port/VLAN detection

**Location**: `src/config.rs`, `src/models.rs`

### Layer 2: State Verification (Already Exists)
- ✅ Parse current state
- ✅ Compute diff
- ✅ Apply only necessary changes
- ✅ Verify commands executed without errors

**Location**: `src/diff/mod.rs`, vendor implementations

### Layer 3: Post-Application Tests (NEW - To Implement)

Before saving configuration permanently, run validation tests.

#### 3.1 Connectivity Tests

**Purpose**: Ensure the switch remains reachable after config changes.

```yaml
validation:
  connectivity:
    enabled: true
    tests:
      - type: ssh_reconnect
        description: "Verify SSH connection still works"
        timeout: 10s

      - type: ping_management_ip
        description: "Ping the switch management IP"
        timeout: 5s
        count: 3

      - type: tcp_port_check
        description: "Verify management ports are accessible"
        ports: [22, 80, 443]
        timeout: 5s
```

**Implementation**:
- After applying config, disconnect and reconnect via SSH/serial
- Verify management interface responds
- Test from switch-configurator host

#### 3.2 Interface State Tests

**Purpose**: Verify ports are in expected operational state.

```yaml
validation:
  interface_checks:
    enabled: true
    tests:
      - type: port_status
        description: "Check ports are up/down as expected"
        verify_enabled_ports_are_up: true
        verify_disabled_ports_are_down: true

      - type: vlan_membership
        description: "Verify VLAN assignments match config"

      - type: poe_status
        description: "Check PoE is enabled on correct ports"
        verify_poe: true
```

**Implementation**:
- Run `show interfaces brief` or equivalent
- Parse output and compare to desired state
- Verify critical ports are operational

#### 3.3 Network Reachability Tests

**Purpose**: Verify network connectivity through the switch.

```yaml
validation:
  network_tests:
    enabled: true
    tests:
      - type: gateway_reachable
        description: "Verify default gateway is reachable"
        target: "192.168.1.1"
        source_vlan: 10  # Test from specific VLAN

      - type: external_endpoint
        description: "Test internet connectivity"
        target: "8.8.8.8"
        protocol: icmp

      - type: http_endpoint
        description: "Verify web services are reachable"
        url: "http://monitoring.local/healthz"
        expected_status: 200
        source_vlan: 100
```

**Implementation**:
- Execute ping/curl commands from the switch
- Use `ping vlan 10 192.168.1.1` or similar
- Verify expected responses

#### 3.4 VLAN Routing Tests

**Purpose**: Verify inter-VLAN routing works if configured.

```yaml
validation:
  vlan_routing:
    enabled: true
    tests:
      - type: inter_vlan_ping
        description: "Test routing between VLANs"
        source_vlan: 10
        target_ip: "192.168.20.1"  # IP in VLAN 20
        expect_success: true

      - type: vlan_isolation
        description: "Verify VLANs are isolated when expected"
        source_vlan: 100
        target_ip: "192.168.200.1"  # IP in isolated VLAN 200
        expect_success: false  # Should NOT be reachable
```

#### 3.5 Critical Service Tests (Advanced)

**Purpose**: Verify specific services/devices are reachable after config change.

```yaml
validation:
  critical_services:
    enabled: true
    tests:
      - type: device_reachable
        description: "Verify critical camera is online"
        device_ip: "192.168.50.100"
        device_name: "Main Entrance Camera"
        protocol: icmp

      - type: service_port
        description: "Verify RTSP stream is accessible"
        device_ip: "192.168.50.100"
        port: 554
        protocol: tcp
```

## Validation Workflow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Parse Current State                                      │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. Compute Diff                                             │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. Apply Changes (to running-config only, DON'T SAVE)      │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. Run Validation Tests                                     │
│    - Connectivity tests                                     │
│    - Interface state verification                           │
│    - Network reachability tests                             │
│    - Critical service checks                                │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
               ┌──────┴──────┐
               │   All Pass? │
               └──────┬──────┘
                      │
          ┌───────────┴──────────┐
          │                      │
      YES │                      │ NO
          ▼                      ▼
┌─────────────────────┐  ┌─────────────────────┐
│ 5. Save Config      │  │ 5. Rollback Config  │
│    write memory     │  │    Option A: Revert │
└─────────────────────┘  │    Option B: Reload │
                         └─────────────────────┘
```

## Configuration Format

Add a `validation` section to each switch configuration:

```yaml
switches:
  - hostname: switch-01
    model: Aruba2930F
    management_ip: "192.168.1.10"
    credentials:
      username: admin
      password: secret

    # ... vlans, ports, etc ...

    # New validation section
    validation:
      enabled: true  # Enable validation for this switch
      timeout: 60s   # Total validation timeout

      # Fail behavior
      on_failure: rollback  # Options: rollback, save_anyway, manual
      rollback_method: reload  # Options: reload, revert_commands

      # Test suites to run
      tests:
        connectivity:
          - type: ssh_reconnect
            timeout: 10s

          - type: ping_management_ip
            count: 3
            timeout: 5s

        interface_checks:
          - type: port_status
            critical_ports: ["1", "2", "24"]  # Must be up

          - type: vlan_membership
            verify_all: true

        network_tests:
          - type: gateway_reachable
            target: "192.168.1.1"
            source_vlan: 10

          - type: external_endpoint
            target: "8.8.8.8"
            required: false  # Warning only if fails

        critical_services:
          - type: device_reachable
            device_ip: "192.168.50.100"
            device_name: "Main Camera"
            required: true  # Fail validation if unreachable

settings:
  # Global validation defaults
  validation_defaults:
    enabled: true
    timeout: 60s
    on_failure: rollback
    rollback_method: reload
```

## Implementation Plan

### Phase 1: Core Validation Framework (src/validation/mod.rs)

```rust
pub struct ValidationConfig {
    pub enabled: bool,
    pub timeout: Duration,
    pub on_failure: FailureAction,
    pub rollback_method: RollbackMethod,
    pub tests: Vec<ValidationTest>,
}

pub enum FailureAction {
    Rollback,      // Revert to previous config
    SaveAnyway,    // Save despite failures (log warnings)
    Manual,        // Stop and wait for manual intervention
}

pub enum RollbackMethod {
    Reload,             // Reboot switch (serial connection helpful)
    RevertCommands,     // Send revert commands
    RestoreBackup,      // Copy startup-config to running-config
}

pub struct ValidationTest {
    pub name: String,
    pub test_type: TestType,
    pub timeout: Duration,
    pub required: bool,  // Fail validation if test fails?
}

pub enum TestType {
    SshReconnect,
    PingManagementIp { count: u32 },
    TcpPortCheck { ports: Vec<u16> },
    PortStatus { critical_ports: Vec<String> },
    VlanMembership,
    GatewayReachable { target: String, source_vlan: u16 },
    DeviceReachable { ip: String, name: String },
    // ... more test types
}

pub struct ValidationResult {
    pub passed: bool,
    pub tests_run: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub failures: Vec<TestFailure>,
    pub duration: Duration,
}
```

### Phase 2: Test Executors (src/validation/tests.rs)

Each test type has an executor that runs the actual validation.

### Phase 3: Integration into Vendor Trait

Update `SwitchVendor` trait:

```rust
#[async_trait]
pub trait SwitchVendor: Send + Sync {
    // ... existing methods ...

    /// Run validation tests after applying config
    async fn validate_configuration(
        &mut self,
        validation_config: &ValidationConfig
    ) -> Result<ValidationResult, VendorError>;

    /// Rollback to previous configuration
    async fn rollback_configuration(
        &mut self,
        method: RollbackMethod
    ) -> Result<(), VendorError>;
}
```

### Phase 4: Update apply_configuration() Flow

```rust
async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError> {
    // 1. Parse current state
    let current = self.parse_current_state().await?;

    // 2. Compute diff
    let diff = crate::diff::compute_diff(&current, &self.config);

    if !diff.has_changes() {
        return Ok(vec![]);
    }

    // 3. Apply changes (DON'T SAVE YET)
    self.apply_diff(&diff).await?;

    // 4. NEW: Run validation if enabled
    if let Some(validation_config) = &self.config.validation {
        if validation_config.enabled {
            let result = self.validate_configuration(validation_config).await?;

            if !result.passed {
                // Validation failed - rollback
                warn!("Validation failed: {} tests failed", result.tests_failed);
                self.rollback_configuration(validation_config.rollback_method).await?;
                return Err(VendorError::ValidationError(
                    format!("Configuration validation failed: {:?}", result.failures)
                ));
            }

            info!("✅ Validation passed: {}/{} tests successful",
                  result.tests_passed, result.tests_run);
        }
    }

    // 5. Save configuration (only if validation passed)
    self.save_configuration().await?;

    Ok(vec![])
}
```

## Rollback Strategies

### Strategy 1: Reload from Startup Config
```
# Most switches boot from startup-config
reload  # or: boot system
```
**Pros**: Clean slate, **Cons**: Downtime

### Strategy 2: Revert Commands
```
# Send opposite commands to undo changes
no vlan 100
interface 10
  no shutdown
```
**Pros**: Fast, **Cons**: Complex to implement

### Strategy 3: Configuration Replace
```
# Some switches support atomic replace
copy startup-config running-config
```
**Pros**: Fast, no reboot, **Cons**: Not all vendors support

## Security Considerations

1. **Validation credentials**: Tests may need different credentials than config management
2. **Test isolation**: Ensure tests don't affect production traffic
3. **Timeout handling**: All tests must have timeouts to prevent hangs
4. **Logging**: Log all validation results for audit trail

## Monitoring Integration

Consider integrating with external monitoring:

```yaml
validation:
  external_monitoring:
    enabled: true
    webhook_url: "http://monitoring.local/api/switch-config-applied"
    wait_for_confirmation: true
    confirmation_timeout: 300s  # Wait 5 minutes for external OK
```

## Examples

### Minimal Validation (Quick Safety Check)
```yaml
validation:
  enabled: true
  timeout: 30s
  tests:
    connectivity:
      - type: ssh_reconnect
```

### Comprehensive Validation (Production)
```yaml
validation:
  enabled: true
  timeout: 120s
  on_failure: rollback
  rollback_method: reload
  tests:
    connectivity:
      - type: ssh_reconnect
      - type: ping_management_ip
      - type: tcp_port_check
        ports: [22, 443]
    interface_checks:
      - type: port_status
      - type: vlan_membership
    network_tests:
      - type: gateway_reachable
        target: "192.168.1.1"
    critical_services:
      - type: device_reachable
        device_ip: "192.168.50.100"
```

## Future Enhancements

1. **Dry-run validation**: Predict what tests would run without applying config
2. **Test templates**: Reusable test suites
3. **Metrics**: Track validation success rates over time
4. **Notification**: Alert on validation failures
5. **Progressive rollout**: Apply to subset of switches first
