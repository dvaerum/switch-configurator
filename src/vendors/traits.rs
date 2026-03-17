use crate::models::{ConfigResult, Port, PortMirror, StateDiff, SwitchModel, SwitchState, Vlan};
use crate::validation::{ValidationConfig, ValidationResult, RollbackMethod};
use async_trait::async_trait;
use thiserror::Error;
use tracing::{debug, warn};

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

    /// Get warnings accumulated during the last configuration cycle
    /// (e.g., hardware model mismatch, deprecated features, etc.)
    fn get_warnings(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Extract a hardware identifier from the running config and verify it matches the
/// configured switch model. This is shared across all vendors.
///
/// Each vendor provides its own regex pattern to extract the hardware identifier from
/// the running config text. The function compares the extracted identifier against the
/// known product numbers for the configured `SwitchModel`.
///
/// Returns any warnings to be added to `SwitchState.warnings`.
pub fn verify_hardware_model(
    config_text: &str,
    model: &SwitchModel,
    extraction_pattern: &regex::Regex,
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Try to extract the hardware identifier from the config
    if let Some(caps) = extraction_pattern.captures(config_text) {
        if let Some(detected) = caps.get(1) {
            let detected_id = detected.as_str();
            let known_products = model.product_numbers();

            if known_products.is_empty() {
                debug!(
                    "Detected hardware identifier: {} (no known product numbers for {:?} to verify against)",
                    detected_id, model
                );
            } else if known_products.contains(&detected_id) {
                debug!(
                    "Hardware identifier {} matches configured model {:?}",
                    detected_id, model
                );
            } else {
                let warning = format!(
                    "Hardware model mismatch: switch reports {} but configured model {:?} expects one of {:?}",
                    detected_id, model, known_products
                );
                warn!("{}", warning);
                warnings.push(warning);
            }
        }
    } else {
        debug!("No hardware identifier found in running config for model {:?}", model);
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create the standard Aruba hardware ID regex
    fn aruba_pattern() -> regex::Regex {
        regex::Regex::new(r";\s*([A-Z]{1,2}\d{3,5}[A-Z]?)\s+Configuration Editor").unwrap()
    }

    #[test]
    fn test_verify_hardware_model_match() {
        let config = "; J9773A Configuration Editor; Created on release #WC.16.11.0018\nhostname \"test\"\n";
        let warnings = verify_hardware_model(config, &SwitchModel::Aruba2530_24G_POE, &aruba_pattern());
        assert!(warnings.is_empty(), "Matching product should produce no warnings");
    }

    #[test]
    fn test_verify_hardware_model_mismatch() {
        let config = "; J9855A Configuration Editor; Created on release #WC.16.11.0018\n";
        let warnings = verify_hardware_model(config, &SwitchModel::Aruba2530_24G_POE, &aruba_pattern());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("mismatch"));
        assert!(warnings[0].contains("J9855A"));
    }

    #[test]
    fn test_verify_hardware_model_no_match_in_config() {
        let config = "hostname \"test\"\nvlan 1\n   name \"default\"\n";
        let warnings = verify_hardware_model(config, &SwitchModel::Aruba2530_24G_POE, &aruba_pattern());
        assert!(warnings.is_empty(), "No identifier found should produce no warnings");
    }

    #[test]
    fn test_verify_hardware_model_empty_product_numbers() {
        // Cisco and FortiSwitch have empty product number lists — should produce no warnings
        let pattern = regex::Regex::new(r"!\s*model\s+(\S+)").unwrap();
        let config = "! model C9300-24P\nhostname test\n";
        let warnings = verify_hardware_model(config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &pattern);
        assert!(warnings.is_empty(), "Empty product_numbers list should produce no warnings (just debug log)");
    }

    #[test]
    fn test_verify_hardware_model_all_aruba_models() {
        let pattern = aruba_pattern();

        // Each model should match its own product number
        for (model, product) in &[
            (SwitchModel::Aruba2530_24G_POE, "J9773A"),
            (SwitchModel::Aruba2530_8G_POE, "J9774A"),
            (SwitchModel::Aruba2530_48G_2SFP, "J9855A"),
            (SwitchModel::Aruba2540_24G, "JL354A"),
            (SwitchModel::Aruba2540_48G_4SFP, "JL355A"),
            (SwitchModel::Aruba2930F, "JL253A"),
        ] {
            let config = format!("; {} Configuration Editor; Created on release #WC.16.11.0018\n", product);
            let warnings = verify_hardware_model(&config, model, &pattern);
            assert!(warnings.is_empty(), "{:?} should match product {} but got warnings: {:?}", model, product, warnings);
        }

        // Cross-model should warn
        let config = "; JL253A Configuration Editor; Created on release #WC.16.11.0018\n";
        let warnings = verify_hardware_model(config, &SwitchModel::Aruba2530_24G_POE, &pattern);
        assert_eq!(warnings.len(), 1, "2930F product on 2530 model should warn");
    }
}
