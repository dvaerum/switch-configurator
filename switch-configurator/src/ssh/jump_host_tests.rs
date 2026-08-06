// Integration tests for jump host functionality
// Note: These tests focus on configuration and validation logic
// Actual SSH connection tests require real servers and are covered in manual/integration tests

#[cfg(test)]
mod tests {
    use crate::models::{ConnectionType, Credentials, JumpHost, SwitchConfig};
    use crate::ssh::jump_host_parser::resolve_jump_host;

    #[test]
    fn test_jump_host_deserialization_simple() {
        let yaml = r#"
username: admin
password: secret
jump_hosts:
  - host: "bastion.example.com"
    ssh_key_path: "/path/to/key"
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(creds.username, "admin");
        assert_eq!(creds.password, Some("secret".to_string()));
        assert!(creds.jump_hosts.is_some());

        let jump_hosts = creds.jump_hosts.unwrap();
        assert_eq!(jump_hosts.len(), 1);
        assert_eq!(jump_hosts[0].host, "bastion.example.com");
        assert_eq!(jump_hosts[0].ssh_key_path, Some("/path/to/key".to_string()));
    }

    #[test]
    fn test_jump_host_deserialization_multi_hop() {
        let yaml = r#"
username: admin
password: secret
jump_hosts:
  - host: "jump1@bastion1.com:22"
    ssh_key_path: "/path/to/key1"
  - host: "bastion2.com:2222"
    username: "jump2"
    password: "pass2"
  - host: "bastion3.com"
    ssh_key_path: "/path/to/key3"
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();

        let jump_hosts = creds.jump_hosts.unwrap();
        assert_eq!(jump_hosts.len(), 3);

        // First hop
        assert_eq!(jump_hosts[0].host, "jump1@bastion1.com:22");
        assert_eq!(jump_hosts[0].ssh_key_path, Some("/path/to/key1".to_string()));

        // Second hop
        assert_eq!(jump_hosts[1].host, "bastion2.com:2222");
        assert_eq!(jump_hosts[1].username, Some("jump2".to_string()));
        assert_eq!(jump_hosts[1].password, Some("pass2".to_string()));

        // Third hop
        assert_eq!(jump_hosts[2].host, "bastion3.com");
        assert_eq!(jump_hosts[2].ssh_key_path, Some("/path/to/key3".to_string()));
    }

    #[test]
    fn test_jump_host_with_fallback_auth() {
        let yaml = r#"
username: admin
password: secret
jump_hosts:
  - host: "bastion.example.com"
    ssh_key_path: "/path/to/key"
    password: "fallback_password"
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();
        let jump_hosts = creds.jump_hosts.unwrap();

        assert_eq!(jump_hosts[0].ssh_key_path, Some("/path/to/key".to_string()));
        assert_eq!(jump_hosts[0].password, Some("fallback_password".to_string()));
    }

    #[test]
    fn test_jump_host_resolution_precedence() {
        // Test that explicit fields take precedence over embedded values
        let jump_host = JumpHost {
            host: "user1@host.com:1111".to_string(),
            username: Some("user2".to_string()),  // Should override user1
            port: Some(2222),                      // Should override 1111
            ssh_key_path: Some("/key".to_string()),
            password: None,
        };

        let resolved = resolve_jump_host(&jump_host, "target_user").unwrap();

        assert_eq!(resolved.username, "user2");  // Explicit wins
        assert_eq!(resolved.port, 2222);         // Explicit wins
        assert_eq!(resolved.hostname, "host.com");
    }

    #[test]
    fn test_jump_host_resolution_username_fallback_chain() {
        // When no explicit username or embedded username is provided,
        // the resolver falls back to $USER env var, then target username
        let jump_host = JumpHost {
            host: "bastion.com".to_string(),
            username: None,
            port: None,
            ssh_key_path: Some("/key".to_string()),
            password: None,
        };

        let resolved = resolve_jump_host(&jump_host, "admin").unwrap();

        // Username precedence: explicit field > embedded in host > $USER env var > target username
        // In most environments $USER is set, so that takes precedence over target username
        let expected_username = std::env::var("USER").unwrap_or_else(|_| "admin".to_string());
        assert_eq!(resolved.username, expected_username);
        assert_eq!(resolved.port, 22);           // Default port
    }

    #[test]
    fn test_jump_host_resolution_embedded_username() {
        let jump_host = JumpHost {
            host: "jumpuser@bastion.com".to_string(),
            username: None,
            port: None,
            ssh_key_path: Some("/key".to_string()),
            password: None,
        };

        let resolved = resolve_jump_host(&jump_host, "admin").unwrap();

        assert_eq!(resolved.username, "jumpuser");  // From embedded
        assert_eq!(resolved.hostname, "bastion.com");
    }

    #[test]
    fn test_switch_config_with_jump_hosts() {
        let yaml = r#"
id: test-sw-01
hostname: remote-switch
model: Aruba2930F
management_ip: "10.0.10.20"
credentials:
  username: admin
  password: switch-pass
  jump_hosts:
    - host: "bastion.company.com"
      ssh_key_path: "/path/to/bastion_key"
vlans: []
ports: []
"#;

        let config: SwitchConfig = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(config.hostname, Some("remote-switch".to_string()));
        assert!(config.credentials.is_some());

        let creds = config.credentials.unwrap();
        assert!(creds.jump_hosts.is_some());

        let jump_hosts = creds.jump_hosts.unwrap();
        assert_eq!(jump_hosts.len(), 1);
        assert_eq!(jump_hosts[0].host, "bastion.company.com");
    }

    #[test]
    fn test_empty_jump_hosts_array() {
        let yaml = r#"
username: admin
password: secret
jump_hosts: []
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();

        // Empty array should be Some([])
        assert!(creds.jump_hosts.is_some());
        assert_eq!(creds.jump_hosts.unwrap().len(), 0);
    }

    #[test]
    fn test_no_jump_hosts_field() {
        let yaml = r#"
username: admin
password: secret
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();

        // No jump_hosts field should be None
        assert!(creds.jump_hosts.is_none());
    }

    #[test]
    fn test_jump_host_invalid_port() {
        let jump_host = JumpHost {
            host: "bastion.com:invalid".to_string(),
            username: None,
            port: None,
            ssh_key_path: None,
            password: None,
        };

        let result = resolve_jump_host(&jump_host, "admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_jump_host_empty_hostname() {
        let jump_host = JumpHost {
            host: "".to_string(),
            username: None,
            port: None,
            ssh_key_path: None,
            password: None,
        };

        let result = resolve_jump_host(&jump_host, "admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_jump_host_only_at_symbol() {
        let jump_host = JumpHost {
            host: "@".to_string(),
            username: None,
            port: None,
            ssh_key_path: None,
            password: None,
        };

        let result = resolve_jump_host(&jump_host, "admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_credentials_equality_with_jump_hosts() {
        let creds1 = Credentials {
            username: "admin".to_string(),
            password: Some("pass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            enable_secret: None,
            jump_hosts: Some(vec![JumpHost {
                host: "bastion.com".to_string(),
                username: None,
                port: None,
                ssh_key_path: Some("/key".to_string()),
                password: None,
            }]),
        };

        let creds2 = Credentials {
            username: "admin".to_string(),
            password: Some("pass".to_string()),
            ssh_key_path: None,
            port: 22,
            connection_type: ConnectionType::Ssh,
            serial_device: None,
            baud_rate: 9600,
            enable_secret: None,
            jump_hosts: Some(vec![JumpHost {
                host: "bastion.com".to_string(),
                username: None,
                port: None,
                ssh_key_path: Some("/key".to_string()),
                password: None,
            }]),
        };

        // JumpHost derives PartialEq, so we can compare
        assert_eq!(creds1.jump_hosts, creds2.jump_hosts);
    }

    #[test]
    fn test_jump_host_serialization_roundtrip() {
        // Test without password (password has skip_serializing so it won't roundtrip)
        let jump_host = JumpHost {
            host: "user@bastion.com:2222".to_string(),
            username: Some("override".to_string()),
            port: Some(3333),
            ssh_key_path: Some("/path/to/key".to_string()),
            password: None,  // Password is not serialized for security
        };

        let yaml = serde_yaml::to_string(&jump_host).unwrap();
        let deserialized: JumpHost = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized, jump_host);
    }

    #[test]
    fn test_jump_host_password_not_serialized() {
        // Verify that password field is not included in serialization
        let jump_host = JumpHost {
            host: "bastion.com".to_string(),
            username: Some("user".to_string()),
            port: None,
            ssh_key_path: None,
            password: Some("secret_password".to_string()),
        };

        let yaml = serde_yaml::to_string(&jump_host).unwrap();

        // Password should not appear in serialized output
        assert!(!yaml.contains("secret_password"));
        assert!(!yaml.contains("password:"));
    }

    #[test]
    fn test_multi_hop_chain_order() {
        let yaml = r#"
username: admin
password: secret
jump_hosts:
  - host: "hop1.com"
    ssh_key_path: "/key1"
  - host: "hop2.com"
    ssh_key_path: "/key2"
  - host: "hop3.com"
    ssh_key_path: "/key3"
"#;

        let creds: Credentials = serde_yaml::from_str(yaml).unwrap();
        let jump_hosts = creds.jump_hosts.unwrap();

        // Verify order is preserved
        assert_eq!(jump_hosts[0].host, "hop1.com");
        assert_eq!(jump_hosts[1].host, "hop2.com");
        assert_eq!(jump_hosts[2].host, "hop3.com");
    }
}
