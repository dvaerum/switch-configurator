/// Enhanced error messages for common configuration mistakes

use serde::Serialize;
use std::path::PathBuf;

/// Detailed information about a config source that contributed to a switch
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSource {
    pub file_path: PathBuf,
    pub priority: u16,
    pub provides_fields: Vec<String>,
}

/// Detailed information about a switch merge/validation failure
#[derive(Debug, Clone, Serialize)]
pub struct SwitchValidationError {
    pub switch_id: String,
    pub missing_fields: Vec<String>,
    pub contributing_sources: Vec<ConfigSource>,
}

impl SwitchValidationError {
    /// Format a single-line error message suitable for logging
    pub fn format_log_message(&self) -> String {
        let filename_context = self
            .contributing_sources
            .first()
            .map(|s| {
                s.file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| s.file_path.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());

        let sources_summary: Vec<String> = self
            .contributing_sources
            .iter()
            .map(|s| {
                let filename = s
                    .file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| s.file_path.to_string_lossy().to_string());
                let fields = if s.provides_fields.is_empty() {
                    "empty".to_string()
                } else {
                    s.provides_fields.join(", ")
                };
                format!("{} (priority {}, provides: {})", filename, s.priority, fields)
            })
            .collect();

        let hint = self.get_contextual_hint();

        format!(
            "Switch ID '{}' (from {}) missing required fields: {}. Contributing files: [{}]. Hint: {}",
            self.switch_id,
            filename_context,
            self.missing_fields.join(", "),
            sources_summary.join("; "),
            hint
        )
    }

    /// Get a contextual hint based on the error situation
    fn get_contextual_hint(&self) -> &'static str {
        let identity_fields = ["hostname", "model", "management_ip", "credentials"];

        // Check if any source provides any identity fields
        let any_source_has_identity = self.contributing_sources.iter().any(|s| {
            s.provides_fields.iter().any(|f| identity_fields.contains(&f.as_str()))
        });

        match self.contributing_sources.len() {
            0 => {
                // No sources found - switch ID doesn't exist anywhere
                "This switch ID was not found in any config file. Check for typos in the 'id' field."
            }
            1 => {
                // Single file - just missing fields
                "Add the missing fields to your config file."
            }
            _ if !any_source_has_identity => {
                // Multiple files but none provide identity - likely ID mismatch with main config
                "No config file provides identity fields (hostname, model, etc.). Check if switch ID matches between main config and folder configs."
            }
            _ => {
                // Multiple files, some identity exists but still missing some
                "Identity fields are split across files but some are missing. Ensure all required fields exist in at least one config file."
            }
        }
    }

    /// Format a detailed, multi-line error message for API responses
    pub fn format_detailed_message(&self) -> String {
        // Get the first contributing filename for context
        let filename_context = self
            .contributing_sources
            .first()
            .map(|s| {
                s.file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| s.file_path.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());

        let mut msg = format!(
            "Switch ID '{}' (from {}) is missing required fields:\n  {}\n",
            self.switch_id,
            filename_context,
            self.missing_fields.join(", ")
        );

        msg.push_str("\nConfig files contributing to this switch ID:\n");
        if self.contributing_sources.is_empty() {
            msg.push_str("  (no config files found for this switch ID)\n");
        } else {
            for source in &self.contributing_sources {
                let filename = source
                    .file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| source.file_path.to_string_lossy().to_string());

                let fields_str = if source.provides_fields.is_empty() {
                    "(empty)".to_string()
                } else {
                    source.provides_fields.join(", ")
                };

                msg.push_str(&format!(
                    "  - \"{}\" (priority: {}) provides: {}\n",
                    filename, source.priority, fields_str
                ));
            }
        }

        // Add fix suggestions
        msg.push_str("\nTo fix this, add the missing fields to one of your config files:\n");
        for field in &self.missing_fields {
            match field.as_str() {
                "hostname" => msg.push_str("  hostname: \"your-switch-hostname\"\n"),
                "model" => msg.push_str("  model: Aruba2930F  # or other supported model\n"),
                "management_ip" => msg.push_str("  management_ip: \"192.168.1.10\"\n"),
                "credentials" => {
                    msg.push_str("  credentials:\n");
                    msg.push_str("    username: admin\n");
                    msg.push_str("    password: yourpassword\n");
                }
                _ => msg.push_str(&format!("  {}: <value>\n", field)),
            }
        }

        // Add contextual hint
        msg.push_str(&format!("\nHint: {}\n", self.get_contextual_hint()));

        msg
    }
}

/// Tracks which config files contributed to a switch during merge
#[derive(Debug, Clone)]
pub struct SwitchMergeTracker {
    pub switch_id: String,
    pub sources: Vec<ConfigSource>,
}

impl SwitchMergeTracker {
    pub fn new(switch_id: String) -> Self {
        Self {
            switch_id,
            sources: Vec::new(),
        }
    }

    /// Record a config file's contribution to this switch
    pub fn add_source(
        &mut self,
        file: PathBuf,
        priority: u16,
        switch: &crate::models::SwitchConfig,
    ) {
        let mut fields = Vec::new();

        if switch.hostname.is_some() {
            fields.push("hostname".to_string());
        }
        if switch.model.is_some() {
            fields.push("model".to_string());
        }
        if switch.management_ip.is_some() {
            fields.push("management_ip".to_string());
        }
        if switch.credentials.is_some() {
            fields.push("credentials".to_string());
        }
        if !switch.vlans.is_empty() {
            fields.push(format!("vlans({})", switch.vlans.len()));
        }
        if !switch.ports.is_empty() {
            fields.push(format!("ports({})", switch.ports.len()));
        }
        if !switch.port_mirrors.is_empty() {
            fields.push(format!("mirrors({})", switch.port_mirrors.len()));
        }
        if switch.snmp.is_some() {
            fields.push("snmp".to_string());
        }

        self.sources.push(ConfigSource {
            file_path: file,
            priority,
            provides_fields: fields,
        });
    }

    /// Convert to a validation error with the given missing fields
    pub fn source_files(&self) -> Vec<PathBuf> {
        self.sources.iter().map(|s| s.file_path.clone()).collect()
    }

    pub fn to_validation_error(&self, missing_fields: Vec<String>) -> SwitchValidationError {
        SwitchValidationError {
            switch_id: self.switch_id.clone(),
            missing_fields,
            contributing_sources: self.sources.clone(),
        }
    }
}

/// Enhance error messages with helpful suggestions based on field path and error content
pub fn enhance_parse_error(field_path: &str, error_msg: &str) -> String {
    // Check for common error patterns and provide helpful suggestions

    // Missing required fields
    if error_msg.contains("missing field") {
        if field_path.contains("switches") && error_msg.contains("management_ip") {
            return format!(
                "{}\n\n\
                 Tip: Every switch must have a management_ip field.\n\
                 Add it like this:\n\
                 management_ip: \"192.168.1.10\"",
                error_msg
            );
        }
        if field_path.contains("switches") && error_msg.contains("credentials") {
            return format!(
                "{}\n\n\
                 Tip: Every switch must have credentials configured.\n\
                 Add them like this:\n\
                 credentials:\n\
                   username: admin\n\
                   password: yourpassword\n\
                 Or use SSH key:\n\
                 credentials:\n\
                   username: admin\n\
                   ssh_key_path: /path/to/key",
                error_msg
            );
        }
    }

    // Type mismatches
    if error_msg.contains("invalid type: string")
        && error_msg.contains("expected a sequence")
        && field_path.contains("source_ports")
    {
        return format!(
            "{}\n\n\
             Tip: source_ports must be an array, not a string.\n\
             Instead of: source_ports: \"33,34,35,36\"\n\
             Use: source_ports: [\"33\", \"34\", \"35\", \"36\"]",
            error_msg
        );
    }

    // Invalid enum values
    if error_msg.contains("unknown variant") {
        if field_path.contains(".mode") {
            return format!(
                "{}\n\n\
                 Tip: Valid port modes are 'access' or 'trunk'.",
                error_msg
            );
        }
        if field_path.contains(".direction") {
            return format!(
                "{}\n\n\
                 Tip: Valid mirror directions are 'both', 'rx', or 'tx'.",
                error_msg
            );
        }
    }

    // Default: return original error
    error_msg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhance_missing_management_ip() {
        let error = enhance_parse_error(
            "switches[0]",
            "switches[0]: missing field `management_ip` at line 2 column 5"
        );
        assert!(error.contains("Tip:"));
        assert!(error.contains("management_ip"));
        assert!(error.contains("192.168.1.10"));
    }

    #[test]
    fn test_enhance_missing_credentials() {
        let error = enhance_parse_error(
            "switches[0]",
            "switches[0]: missing field `credentials` at line 2 column 5"
        );
        assert!(error.contains("Tip:"));
        assert!(error.contains("credentials"));
        assert!(error.contains("username"));
    }

    #[test]
    fn test_enhance_source_ports_type_mismatch() {
        let error = enhance_parse_error(
            "switches[0].port_mirrors[0].source_ports",
            "switches[0].port_mirrors[0].source_ports: invalid type: string \"33,34,35,36\", expected a sequence"
        );
        assert!(error.contains("Tip:"));
        assert!(error.contains("array"));
        assert!(error.contains("[\"33\", \"34\", \"35\", \"36\"]"));
    }

    #[test]
    fn test_enhance_invalid_port_mode() {
        let error = enhance_parse_error(
            "switches[0].ports[0].mode",
            "switches[0].ports[0].mode: unknown variant `wrong_mode`, expected `access` or `trunk`"
        );
        assert!(error.contains("Tip:"));
        assert!(error.contains("access"));
        assert!(error.contains("trunk"));
    }

    #[test]
    fn test_no_enhancement_for_unknown_error() {
        let original = "some random error";
        let error = enhance_parse_error("some.field", original);
        assert_eq!(error, original);
    }

    #[test]
    fn test_switch_validation_error_log_format_single_source() {
        // Single source file, no identity fields - hint should say "Add missing fields"
        let error = SwitchValidationError {
            switch_id: "test-switch".to_string(),
            missing_fields: vec![
                "hostname".to_string(),
                "model".to_string(),
                "credentials".to_string(),
            ],
            contributing_sources: vec![ConfigSource {
                file_path: PathBuf::from("/etc/switch-configurator/test-switch.yaml"),
                priority: 100,
                provides_fields: vec!["ports(18)".to_string(), "mirrors(1)".to_string()],
            }],
        };

        let msg = error.format_log_message();

        // Should be single line (no newlines)
        assert!(!msg.contains('\n'), "Log message should be single line");

        // Check it contains key information
        assert!(msg.contains("Switch ID 'test-switch'"));
        assert!(msg.contains("(from test-switch.yaml)"));
        assert!(msg.contains("hostname, model, credentials"));
        assert!(msg.contains("test-switch.yaml (priority 100"));
        assert!(msg.contains("Hint: Add the missing fields"));
    }

    #[test]
    fn test_switch_validation_error_log_format_no_sources() {
        // No sources - hint should mention switch ID not found
        let error = SwitchValidationError {
            switch_id: "orphan-switch".to_string(),
            missing_fields: vec!["hostname".to_string()],
            contributing_sources: vec![],
        };

        let msg = error.format_log_message();
        assert!(msg.contains("Hint: This switch ID was not found"));
    }

    #[test]
    fn test_switch_validation_error_log_format_multiple_no_identity() {
        // Multiple sources but none provide identity - hint should mention ID mismatch
        let error = SwitchValidationError {
            switch_id: "test-switch".to_string(),
            missing_fields: vec!["hostname".to_string(), "model".to_string()],
            contributing_sources: vec![
                ConfigSource {
                    file_path: PathBuf::from("ports.yaml"),
                    priority: 100,
                    provides_fields: vec!["ports(18)".to_string()],
                },
                ConfigSource {
                    file_path: PathBuf::from("mirrors.yaml"),
                    priority: 100,
                    provides_fields: vec!["mirrors(1)".to_string()],
                },
            ],
        };

        let msg = error.format_log_message();
        assert!(msg.contains("Hint: No config file provides identity fields"));
    }

    #[test]
    fn test_switch_validation_error_log_format_partial_identity() {
        // Multiple sources, some identity but still missing - hint should mention split fields
        let error = SwitchValidationError {
            switch_id: "sw-01".to_string(),
            missing_fields: vec!["credentials".to_string()],
            contributing_sources: vec![
                ConfigSource {
                    file_path: PathBuf::from("main.yaml"),
                    priority: 10,
                    provides_fields: vec!["hostname".to_string(), "model".to_string()],
                },
                ConfigSource {
                    file_path: PathBuf::from("ports.yaml"),
                    priority: 100,
                    provides_fields: vec!["ports(24)".to_string()],
                },
            ],
        };

        let msg = error.format_log_message();
        assert!(msg.contains("Hint: Identity fields are split across files"));
    }

    #[test]
    fn test_switch_validation_error_detailed_format() {
        let error = SwitchValidationError {
            switch_id: "test-switch".to_string(),
            missing_fields: vec![
                "hostname".to_string(),
                "model".to_string(),
                "credentials".to_string(),
            ],
            contributing_sources: vec![ConfigSource {
                file_path: PathBuf::from("/etc/switch-configurator/test-switch.yaml"),
                priority: 100,
                provides_fields: vec!["ports(18)".to_string(), "mirrors(1)".to_string()],
            }],
        };

        let msg = error.format_detailed_message();

        // Should be multi-line
        assert!(msg.contains('\n'), "Detailed message should be multi-line");

        // Check it contains key information
        assert!(msg.contains("Switch ID 'test-switch'"), "Should mention Switch ID");
        assert!(msg.contains("(from test-switch.yaml)"), "Should include filename after ID");
        assert!(msg.contains("hostname"));
        assert!(msg.contains("model"));
        assert!(msg.contains("credentials"));
        assert!(msg.contains("test-switch.yaml"));
        assert!(msg.contains("priority: 100"));
        assert!(msg.contains("ports(18)"));
        assert!(msg.contains("Hint:"));
    }

    #[test]
    fn test_switch_validation_error_empty_sources() {
        let error = SwitchValidationError {
            switch_id: "orphan-switch".to_string(),
            missing_fields: vec!["hostname".to_string()],
            contributing_sources: vec![],
        };

        let msg = error.format_detailed_message();
        assert!(msg.contains("no config files found"));
        assert!(msg.contains("Hint: This switch ID was not found"));
    }

    #[test]
    fn test_switch_validation_error_multiple_sources() {
        let error = SwitchValidationError {
            switch_id: "sw-01".to_string(),
            missing_fields: vec!["credentials".to_string()],
            contributing_sources: vec![
                ConfigSource {
                    file_path: PathBuf::from("main.yaml"),
                    priority: 10,
                    provides_fields: vec!["hostname".to_string(), "model".to_string()],
                },
                ConfigSource {
                    file_path: PathBuf::from("ports.yaml"),
                    priority: 100,
                    provides_fields: vec!["ports(24)".to_string()],
                },
            ],
        };

        let msg = error.format_detailed_message();
        assert!(msg.contains("main.yaml"));
        assert!(msg.contains("ports.yaml"));
        assert!(msg.contains("priority: 10"));
        assert!(msg.contains("priority: 100"));
    }
}
