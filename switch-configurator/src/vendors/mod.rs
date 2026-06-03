pub mod aruba;
pub mod cisco;
pub mod fortiswitch;
pub mod traits;

#[cfg(test)]
mod tests;

pub use traits::SwitchVendor;

use crate::config::RuntimeConfig;
use crate::models::{SwitchConfig, Vendor};
use anyhow::Result;

/// Factory function to create the appropriate vendor implementation
pub fn create_vendor(config: &SwitchConfig) -> Result<Box<dyn SwitchVendor>> {
    create_vendor_with_runtime(config, &RuntimeConfig::default(), false)
}

/// Factory function with runtime configuration and enforce_port_config setting
pub fn create_vendor_with_runtime(
    config: &SwitchConfig,
    runtime_config: &RuntimeConfig,
    enforce_port_config: bool,
) -> Result<Box<dyn SwitchVendor>> {
    match config.model().vendor() {
        Vendor::Aruba => Ok(Box::new(aruba::ArubaSwitch::new(
            config.clone(),
            runtime_config.clone(),
            enforce_port_config,
        ))),
        Vendor::Cisco => Ok(Box::new(cisco::CiscoSwitch::new(
            config.clone(),
            runtime_config.clone(),
            enforce_port_config,
        ))),
        Vendor::Fortiswitch => Ok(Box::new(fortiswitch::FortiswitchSwitch::new(
            config.clone(),
            runtime_config.clone(),
            enforce_port_config,
        ))),
    }
}
