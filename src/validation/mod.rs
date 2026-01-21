pub mod tests;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for validation tests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationConfig {
    /// Enable validation tests
    #[serde(default)]
    pub enabled: bool,

    /// Total timeout for all validation tests
    #[serde(default = "default_timeout")]
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,

    /// What to do if validation fails
    #[serde(default)]
    pub on_failure: FailureAction,

    /// How to rollback if validation fails
    #[serde(default)]
    pub rollback_method: RollbackMethod,

    /// List of validation tests to run
    #[serde(default)]
    pub tests: Vec<ValidationTest>,
}

fn default_timeout() -> Duration {
    Duration::from_secs(60)
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout: default_timeout(),
            on_failure: FailureAction::default(),
            rollback_method: RollbackMethod::default(),
            tests: Vec::new(),
        }
    }
}

/// Action to take when validation fails
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    /// Rollback to previous configuration
    Rollback,
    /// Save configuration anyway (log warnings)
    SaveAnyway,
    /// Stop and wait for manual intervention
    Manual,
}

impl Default for FailureAction {
    fn default() -> Self {
        Self::Rollback
    }
}

/// Method to use for rolling back configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackMethod {
    /// Reload switch from startup-config (requires reboot)
    Reload,
    /// Send revert commands to undo changes
    RevertCommands,
    /// Copy startup-config to running-config
    RestoreBackup,
}

impl Default for RollbackMethod {
    fn default() -> Self {
        Self::RestoreBackup
    }
}

/// A single validation test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTest {
    /// Test type
    #[serde(flatten)]
    pub test_type: TestType,

    /// Timeout for this specific test
    #[serde(default = "default_test_timeout")]
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,

    /// Is this test required for validation to pass?
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_test_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_required() -> bool {
    true
}

/// Types of validation tests
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TestType {
    /// Test that we can still execute commands on the switch
    CommandExecution {
        #[serde(default = "default_test_command")]
        command: String,
    },

    /// Ping the management IP from the host running switch-configurator
    PingManagementIp {
        #[serde(default = "default_ping_count")]
        count: u32,
    },

    /// Check specific TCP ports are accessible
    TcpPortCheck {
        ports: Vec<u16>,
    },

    /// Ping a target from the switch
    GatewayReachable {
        target: String,
        #[serde(default)]
        source_vlan: Option<u16>,
    },

    /// Test if a device is reachable from the switch
    DeviceReachable {
        device_ip: String,
        device_name: String,
        #[serde(default = "default_ping_count")]
        count: u32,
    },

    /// Verify port status matches expected state
    PortStatus {
        #[serde(default)]
        critical_ports: Vec<String>,
    },

    /// Verify VLAN membership is correct
    VlanMembership,
}

fn default_test_command() -> String {
    "show version".to_string()
}

fn default_ping_count() -> u32 {
    3
}

/// Result of running validation tests
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Did all required tests pass?
    pub passed: bool,

    /// Total number of tests run
    pub tests_run: usize,

    /// Number of tests that passed
    pub tests_passed: usize,

    /// Number of tests that failed
    pub tests_failed: usize,

    /// Details of failed tests
    pub failures: Vec<TestFailure>,

    /// Total time taken for validation
    pub duration: Duration,
}

/// Details of a failed test
#[derive(Debug, Clone)]
pub struct TestFailure {
    /// Name/description of the test
    pub test_name: String,

    /// Was this a required test?
    pub required: bool,

    /// Error message
    pub error: String,

    /// Time taken before failure
    pub duration: Duration,
}

impl ValidationResult {
    /// Create a new validation result
    pub fn new() -> Self {
        Self {
            passed: true,
            tests_run: 0,
            tests_passed: 0,
            tests_failed: 0,
            failures: Vec::new(),
            duration: Duration::from_secs(0),
        }
    }

    /// Record a test success
    pub fn record_success(&mut self) {
        self.tests_run += 1;
        self.tests_passed += 1;
    }

    /// Record a test failure
    pub fn record_failure(&mut self, failure: TestFailure) {
        self.tests_run += 1;
        self.tests_failed += 1;

        if failure.required {
            self.passed = false;
        }

        self.failures.push(failure);
    }

    /// Finalize the result with total duration
    pub fn finalize(&mut self, duration: Duration) {
        self.duration = duration;
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new()
    }
}
