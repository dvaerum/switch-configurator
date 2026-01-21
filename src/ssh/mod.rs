pub mod client;
pub mod connection;
pub mod serial;
pub mod jump_host;
pub mod jump_host_parser;
pub mod jump_chain;

#[cfg(test)]
mod jump_host_tests;

pub use client::SshClient;
pub use connection::ConnectionClient;
pub use serial::SerialClient;

use std::borrow::Cow;

/// Expand tilde (~) in paths to the user's home directory.
///
/// Supports:
/// - `~/path` -> `/home/user/path`
/// - `~` -> `/home/user`
/// - Other paths are returned unchanged
///
/// Returns the original path if home directory cannot be determined.
pub fn expand_tilde(path: &str) -> Cow<'_, str> {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return Cow::Owned(home.to_string_lossy().into_owned());
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return Cow::Owned(format!("{}/{}", home.to_string_lossy(), rest));
        }
    }
    Cow::Borrowed(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde_with_path() {
        let expanded = expand_tilde("~/.ssh/id_ed25519");
        // Should start with home directory (not ~)
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/.ssh/id_ed25519"));
    }

    #[test]
    fn test_expand_tilde_alone() {
        let expanded = expand_tilde("~");
        assert!(!expanded.starts_with('~'));
        assert!(!expanded.is_empty());
    }

    #[test]
    fn test_expand_tilde_absolute_path_unchanged() {
        let path = "/etc/ssh/keys";
        let expanded = expand_tilde(path);
        assert_eq!(expanded, path);
    }

    #[test]
    fn test_expand_tilde_relative_path_unchanged() {
        let path = "relative/path";
        let expanded = expand_tilde(path);
        assert_eq!(expanded, path);
    }

    #[test]
    fn test_expand_tilde_not_at_start_unchanged() {
        let path = "/some/~path";
        let expanded = expand_tilde(path);
        assert_eq!(expanded, path);
    }
}
