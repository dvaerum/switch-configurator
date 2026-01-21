use crate::models::{ConfigResult, Port, PortMirror, StateDiff, SwitchState, Vlan};
use crate::validation::{ValidationConfig, ValidationResult, RollbackMethod};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum VendorError {
    #[error("SSH connection error: {0}")]
    SshError(String),

    #[error("Command execution error: {0}")]
    CommandError(String),

    #[error("Configuration validation error: {0}")]
    ValidationError(String),

    #[error("Unsupported feature: {0}")]
    #[allow(dead_code)]
    UnsupportedFeature(String),

    #[error("Parse error: {0}")]
    #[allow(dead_code)]
    ParseError(String),
}

/// Trait that all switch vendor implementations must implement
#[async_trait]
pub trait SwitchVendor: Send + Sync {
    /// Connect to the switch
    async fn connect(&mut self) -> Result<(), VendorError>;

    /// Disconnect from the switch
    async fn disconnect(&mut self) -> Result<(), VendorError>;

    /// Parse the running configuration and return current state
    async fn parse_current_state(&mut self) -> Result<SwitchState, VendorError>;

    /// Apply differential configuration based on state diff
    async fn apply_diff(&mut self, diff: &StateDiff) -> Result<Vec<ConfigResult>, VendorError>;

    /// Configure VLANs on the switch
    async fn configure_vlans(&mut self, vlans: &[Vlan]) -> Result<ConfigResult, VendorError>;

    /// Configure ports on the switch
    async fn configure_ports(&mut self, ports: &[Port]) -> Result<ConfigResult, VendorError>;

    /// Configure port mirroring/SPAN
    async fn configure_port_mirrors(
        &mut self,
        mirrors: &[PortMirror],
    ) -> Result<ConfigResult, VendorError>;

    /// Apply the complete configuration (now uses diff internally)
    async fn apply_configuration(&mut self) -> Result<Vec<ConfigResult>, VendorError>;

    /// Save the running configuration to startup configuration
    async fn save_configuration(&mut self) -> Result<(), VendorError>;

    /// Get the current running configuration
    async fn get_running_config(&mut self) -> Result<String, VendorError>;

    /// Validate the configuration before applying
    #[allow(dead_code)]
    fn validate_configuration(&self) -> Result<(), VendorError>;

    /// Run validation tests after applying configuration
    async fn run_validation_tests(
        &mut self,
        validation_config: &ValidationConfig,
    ) -> Result<ValidationResult, VendorError>;

    /// Rollback configuration to previous state
    async fn rollback_configuration(
        &mut self,
        method: RollbackMethod,
    ) -> Result<(), VendorError>;
}
