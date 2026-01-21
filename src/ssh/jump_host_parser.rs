use anyhow::{Context, Result};
use crate::models::{JumpHost, ResolvedJumpHost};

/// Parse user@hostname:port format into components
///
/// Examples:
///   "example.com"              -> (None, "example.com", None)
///   "example.com:2222"         -> (None, "example.com", Some(2222))
///   "user@example.com"         -> (Some("user"), "example.com", None)
///   "user@example.com:2222"    -> (Some("user"), "example.com", Some(2222))
fn parse_host_string(host: &str) -> Result<(Option<String>, String, Option<u16>)> {
    // Split on '@' to separate user from host:port
    let (user, host_port) = if let Some(at_pos) = host.find('@') {
        let user = host[..at_pos].to_string();
        let host_port = &host[at_pos + 1..];
        (Some(user), host_port)
    } else {
        (None, host)
    };

    // Split on ':' to separate host from port
    // Use rfind to handle IPv6 addresses (though rare in practice)
    let (hostname, port) = if let Some(colon_pos) = host_port.rfind(':') {
        let hostname = host_port[..colon_pos].to_string();
        let port_str = &host_port[colon_pos + 1..];
        let port = port_str.parse::<u16>()
            .with_context(|| format!("Invalid port number: {}", port_str))?;
        (hostname, Some(port))
    } else {
        (host_port.to_string(), None)
    };

    // Validate hostname is not empty
    if hostname.is_empty() {
        anyhow::bail!("Hostname cannot be empty");
    }

    Ok((user, hostname, port))
}

/// Resolve a JumpHost configuration into concrete connection parameters
///
/// Applies precedence rules:
/// - username: explicit field > embedded in host > current user > target username
/// - port: explicit field > embedded in host > 22
pub fn resolve_jump_host(
    jump_host: &JumpHost,
    target_username: &str,
) -> Result<ResolvedJumpHost> {
    // Parse the host string
    let (embedded_user, hostname, embedded_port) = parse_host_string(&jump_host.host)
        .with_context(|| format!("Failed to parse jump host: {}", jump_host.host))?;

    // Apply precedence rules for username
    // Priority: 1. Explicit username field, 2. Username from host string, 3. Current user, 4. Target username
    let username = jump_host.username.clone()
        .or(embedded_user)
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| target_username.to_string());

    // Apply precedence rules for port
    // Priority: 1. Explicit port field, 2. Port from host string, 3. Default SSH port (22)
    let port = jump_host.port
        .or(embedded_port)
        .unwrap_or(22);

    Ok(ResolvedJumpHost {
        hostname,
        port,
        username,
        ssh_key_path: jump_host.ssh_key_path.clone(),
        password: jump_host.password.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_host_simple() {
        let (user, host, port) = parse_host_string("example.com").unwrap();
        assert_eq!(user, None);
        assert_eq!(host, "example.com");
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_host_with_port() {
        let (user, host, port) = parse_host_string("example.com:2222").unwrap();
        assert_eq!(user, None);
        assert_eq!(host, "example.com");
        assert_eq!(port, Some(2222));
    }

    #[test]
    fn test_parse_host_with_user() {
        let (user, host, port) = parse_host_string("admin@example.com").unwrap();
        assert_eq!(user, Some("admin".to_string()));
        assert_eq!(host, "example.com");
        assert_eq!(port, None);
    }

    #[test]
    fn test_parse_host_full() {
        let (user, host, port) = parse_host_string("admin@example.com:2222").unwrap();
        assert_eq!(user, Some("admin".to_string()));
        assert_eq!(host, "example.com");
        assert_eq!(port, Some(2222));
    }

    #[test]
    fn test_parse_host_invalid_port() {
        assert!(parse_host_string("example.com:abc").is_err());
    }

    #[test]
    fn test_parse_host_empty() {
        assert!(parse_host_string("").is_err());
    }

    #[test]
    fn test_parse_host_only_at() {
        assert!(parse_host_string("@").is_err());
    }

    #[test]
    fn test_resolve_jump_host_defaults() {
        let jump_host = JumpHost {
            host: "example.com".to_string(),
            username: None,
            port: None,
            ssh_key_path: None,
            password: None,
        };

        let resolved = resolve_jump_host(&jump_host, "target_user").unwrap();
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 22);
        // Should default to current user from environment, or target_user if $USER not set
        let expected_username = std::env::var("USER").unwrap_or_else(|_| "target_user".to_string());
        assert_eq!(resolved.username, expected_username);
        assert_eq!(resolved.ssh_key_path, None);
        assert_eq!(resolved.password, None);
    }

    #[test]
    fn test_resolve_jump_host_embedded_values() {
        let jump_host = JumpHost {
            host: "jumpuser@example.com:2222".to_string(),
            username: None,
            port: None,
            ssh_key_path: Some("/path/to/key".to_string()),
            password: None,
        };

        let resolved = resolve_jump_host(&jump_host, "target_user").unwrap();
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.username, "jumpuser");
        assert_eq!(resolved.ssh_key_path, Some("/path/to/key".to_string()));
    }

    #[test]
    fn test_resolve_jump_host_explicit_override() {
        let jump_host = JumpHost {
            host: "jumpuser@example.com:2222".to_string(),
            username: Some("override_user".to_string()),
            port: Some(3333),
            ssh_key_path: None,
            password: Some("pass".to_string()),
        };

        let resolved = resolve_jump_host(&jump_host, "target_user").unwrap();
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 3333);  // Explicit override wins
        assert_eq!(resolved.username, "override_user");  // Explicit override wins
        assert_eq!(resolved.password, Some("pass".to_string()));
    }

    #[test]
    fn test_resolve_jump_host_with_all_auth() {
        let jump_host = JumpHost {
            host: "bastion.company.com".to_string(),
            username: Some("jumpuser".to_string()),
            port: None,
            ssh_key_path: Some("/path/to/key".to_string()),
            password: Some("fallback_password".to_string()),
        };

        let resolved = resolve_jump_host(&jump_host, "admin").unwrap();
        assert_eq!(resolved.hostname, "bastion.company.com");
        assert_eq!(resolved.port, 22);
        assert_eq!(resolved.username, "jumpuser");
        assert_eq!(resolved.ssh_key_path, Some("/path/to/key".to_string()));
        assert_eq!(resolved.password, Some("fallback_password".to_string()));
    }
}
