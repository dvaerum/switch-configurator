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

    /// Generate CLI commands for a given diff without executing them.
    /// Used for preview/dry-run in the web UI.
    fn generate_commands_for_diff(&self, diff: &StateDiff) -> crate::models::CommandPreview;

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

        // J9854A (2530-24G-PoE+-2SFP+) should also match Aruba2530_24G_POE
        let config_j9854a = "; J9854A Configuration Editor; Created on release #WC.16.11.0018\n";
        let warnings_j9854a = verify_hardware_model(config_j9854a, &SwitchModel::Aruba2530_24G_POE, &pattern);
        assert!(warnings_j9854a.is_empty(), "J9854A should match Aruba2530_24G_POE but got warnings: {:?}", warnings_j9854a);

        // Cross-model should warn
        let config = "; JL253A Configuration Editor; Created on release #WC.16.11.0018\n";
        let warnings = verify_hardware_model(config, &SwitchModel::Aruba2530_24G_POE, &pattern);
        assert_eq!(warnings.len(), 1, "2930F product on 2530 model should warn");
    }

    // ============================================================================
    // Cisco Model Detection Tests
    // ============================================================================

    fn cisco_pattern() -> regex::Regex {
        regex::Regex::new(r"(?m)^switch\s+\d+\s+provision\s+(\S+)").unwrap()
    }

    #[test]
    fn test_cisco_model_detection_match() {
        let config = "!\nversion 16.9\nhostname IT-04263\nswitch 1 provision c9300-24u\nip ssh version 2\n";
        let warnings = verify_hardware_model(config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &cisco_pattern());
        assert!(warnings.is_empty(), "c9300-24u should match CiscoCatalyst9300_24P_UPOE, got: {:?}", warnings);
    }

    #[test]
    fn test_cisco_model_detection_mismatch() {
        let config = "!\nversion 16.9\nhostname test\nswitch 1 provision c9200-48p\n";
        let warnings = verify_hardware_model(config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &cisco_pattern());
        assert_eq!(warnings.len(), 1, "c9200-48p should not match C9300");
        assert!(warnings[0].contains("c9200-48p"));
        assert!(warnings[0].contains("mismatch"));
    }

    #[test]
    fn test_cisco_model_detection_no_provision_line() {
        let config = "!\nversion 16.9\nhostname test\nip ssh version 2\n";
        let warnings = verify_hardware_model(config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &cisco_pattern());
        assert!(warnings.is_empty(), "No provision line should produce no warnings");
    }

    #[test]
    fn test_cisco_model_detection_stacked() {
        // Stacked switches have multiple provision lines — we match the first one
        let config = "switch 1 provision c9300-24u\nswitch 2 provision c9300-24u\n";
        let warnings = verify_hardware_model(config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &cisco_pattern());
        assert!(warnings.is_empty(), "Stacked c9300-24u should match");
    }

    // ============================================================================
    // FortiSwitch Model Detection Tests
    // ============================================================================

    fn forti_pattern() -> regex::Regex {
        regex::Regex::new(r"Version:\s*(FortiSwitch-\S+)\s+v").unwrap()
    }

    #[test]
    fn test_forti_model_detection_match_124f() {
        let output = "Version: FortiSwitch-124F-FPOE v7.2.8,build0660,241119 (GA.MR8)\nSerial-Number: S124FFTF24000746\n";
        let warnings = verify_hardware_model(output, &SwitchModel::Fortiswitch124F_FPOE, &forti_pattern());
        assert!(warnings.is_empty(), "FortiSwitch-124F-FPOE should match Fortiswitch124F_FPOE, got: {:?}", warnings);
    }

    #[test]
    fn test_forti_model_detection_mismatch_108f_on_124f() {
        // 108F hardware configured as 124F model — should warn
        let output = "Version: FortiSwitch-108F-POE v7.2.8,build0660,241119 (GA.MR8)\nSerial-Number: S108FPTV21002683\n";
        let warnings = verify_hardware_model(output, &SwitchModel::Fortiswitch124F_FPOE, &forti_pattern());
        assert_eq!(warnings.len(), 1, "108F hardware on 124F config should warn");
        assert!(warnings[0].contains("FortiSwitch-108F-POE"));
        assert!(warnings[0].contains("mismatch"));
    }

    #[test]
    fn test_forti_model_detection_no_version_line() {
        let output = "Serial-Number: S108FPTV21002683\nHostname: test\n";
        let warnings = verify_hardware_model(output, &SwitchModel::Fortiswitch124F_FPOE, &forti_pattern());
        assert!(warnings.is_empty(), "No version line should produce no warnings");
    }

    // ============================================================================
    // Cross-vendor Product Numbers Tests
    // ============================================================================

    #[test]
    fn test_all_models_have_product_numbers() {
        // All models that we support model detection for should have product numbers
        assert!(!SwitchModel::Aruba2530_24G_POE.product_numbers().is_empty(), "Aruba 2530-24G");
        assert!(!SwitchModel::Aruba2530_8G_POE.product_numbers().is_empty(), "Aruba 2530-8G");
        assert!(!SwitchModel::Aruba2530_48G_2SFP.product_numbers().is_empty(), "Aruba 2530-48G");
        assert!(!SwitchModel::Aruba2540_24G.product_numbers().is_empty(), "Aruba 2540-24G");
        assert!(!SwitchModel::Aruba2540_48G_4SFP.product_numbers().is_empty(), "Aruba 2540-48G");
        assert!(!SwitchModel::Aruba2930F.product_numbers().is_empty(), "Aruba 2930F");
        assert!(!SwitchModel::CiscoCatalyst9300_24P_UPOE.product_numbers().is_empty(), "Cisco C9300");
        assert!(!SwitchModel::Fortiswitch124F_FPOE.product_numbers().is_empty(), "FortiSwitch 124F");
    }

    // ============================================================================
    // Gap 6: FortiSwitch realistic model detection integration
    // ============================================================================

    #[test]
    fn test_forti_model_detection_realistic_get_system_status() {
        let pattern = forti_pattern();

        // Realistic multi-line `get system status` output from a FortiSwitch-124F-FPOE
        let output_124f = "\
Version: FortiSwitch-124F-FPOE v7.2.8,build0660,241119 (GA.MR8)
Virus-DB: 1.00000(2018-04-09 18:07)
Serial-Number: S124FFTF24000746
Hostname: forti-124f
Distribution: International
Branch point: 0660
Release Version Information: GA.MR8
System time: Thu Mar 13 11:22:33 2025";

        let warnings = verify_hardware_model(output_124f, &SwitchModel::Fortiswitch124F_FPOE, &pattern);
        assert!(warnings.is_empty(),
                "Realistic 124F-FPOE output should match Fortiswitch124F_FPOE, got: {:?}", warnings);

        // Now test with 108F output — should MISMATCH against 124F model
        let output_108f = "\
Version: FortiSwitch-108F-POE v7.2.8,build0660,241119 (GA.MR8)
Virus-DB: 1.00000(2018-04-09 18:07)
Serial-Number: S108FPTV21002683
Hostname: forti-108f
Distribution: International
Branch point: 0660
Release Version Information: GA.MR8
System time: Thu Mar 13 11:22:33 2025";

        let warnings_108f = verify_hardware_model(output_108f, &SwitchModel::Fortiswitch124F_FPOE, &pattern);
        assert_eq!(warnings_108f.len(), 1,
                   "108F output should MISMATCH against 124F model");
        assert!(warnings_108f[0].contains("FortiSwitch-108F-POE"),
                "Warning should mention the detected model");
        assert!(warnings_108f[0].contains("mismatch"),
                "Warning should mention mismatch");
    }

    // ============================================================================
    // Gap 7: Cisco realistic running-config model detection integration
    // ============================================================================

    #[test]
    fn test_cisco_model_detection_realistic_running_config() {
        let pattern = cisco_pattern();

        // Realistic multi-line Cisco running config
        let running_config = "\
!
version 16.9
no service pad
service timestamps debug datetime msec
service timestamps log datetime msec
no service password-encryption
!
hostname IT-04263
!
boot-start-marker
boot-end-marker
!
no aaa new-model
switch 1 provision c9300-24u
!
ip routing
!
license boot level network-advantage addon dna-advantage
!
interface GigabitEthernet1/0/1
 description Uplink
 switchport mode trunk
!
end";

        let warnings = verify_hardware_model(running_config, &SwitchModel::CiscoCatalyst9300_24P_UPOE, &pattern);
        assert!(warnings.is_empty(),
                "Realistic c9300-24u running config should match CiscoCatalyst9300_24P_UPOE, got: {:?}", warnings);
    }
}
