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

/// Remove "-- MORE --" pager prompt line(s) from an accumulated output buffer
/// after they've been answered with SPACE.
///
/// Without this, the stale marker stays in the buffer and keeps matching the
/// pager regex on every subsequent read, so the read loop spins forever sending
/// SPACE and never gets to detect the real command prompt. In the field this
/// showed up as a 60s "Timeout waiting for prompt" that failed the whole apply
/// (Aruba 2530 where `no page` didn't suppress paging). Page CONTENT before the
/// marker is preserved; only the pager prompt line itself is removed.
///
/// Returns true if at least one pager prompt was found and removed.
pub(crate) fn strip_pager_prompt(output: &mut Vec<u8>) -> bool {
    // Match "-- MORE --" (with flexible spacing) and consume the rest of that
    // line (", next page: Space, ..."). Operates on raw bytes so byte offsets
    // stay correct even with interleaved ANSI escape sequences.
    let re = regex::bytes::Regex::new(r"--\s*MORE\s*--[^\r\n]*").unwrap();
    if !re.is_match(output) {
        return false;
    }
    *output = re.replace_all(output, &b""[..]).into_owned();
    true
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

    #[test]
    fn test_strip_pager_prompt_removes_marker_keeps_content() {
        // Content of a page followed by the Aruba pager prompt line. After we
        // answer with SPACE, the marker must be removed so it is not re-detected
        // (which previously caused an endless SPACE loop -> 60s timeout).
        let mut buf = b"hostname \"SW1\"\r\nvlan 10\r\n-- MORE --, next page: Space, next line: Enter, quit: Control-C".to_vec();
        let stripped = strip_pager_prompt(&mut buf);
        assert!(stripped, "should report a pager prompt was stripped");
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("hostname \"SW1\""), "page content must be kept");
        assert!(text.contains("vlan 10"), "page content must be kept");
        assert!(!text.contains("MORE"), "pager marker must be gone: {text:?}");
    }

    #[test]
    fn test_strip_pager_prompt_no_marker_is_noop() {
        let mut buf = b"vlan 20\r\nSW1# ".to_vec();
        let before = buf.clone();
        let stripped = strip_pager_prompt(&mut buf);
        assert!(!stripped, "no pager prompt present");
        assert_eq!(buf, before, "buffer must be untouched when no marker");
    }

    #[test]
    fn test_strip_pager_prompt_removes_all_stale_markers() {
        // Two accumulated markers (a stale one plus the current one) must both go.
        let mut buf = b"page1\r\n-- MORE --\r\npage2\r\n-- MORE --, next page: Space".to_vec();
        assert!(strip_pager_prompt(&mut buf));
        let text = String::from_utf8_lossy(&buf);
        assert!(!text.contains("MORE"), "all markers gone: {text:?}");
        assert!(text.contains("page1") && text.contains("page2"), "content kept");
    }
}
