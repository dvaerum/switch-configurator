//! Log output setup. Under systemd the journal already stamps every line with
//! the time and unit, so the service's own timestamp and colour codes would
//! only repeat it.

use tracing::Level;

/// Is stderr connected to the systemd journal? systemd sets `JOURNAL_STREAM`
/// to `<device>:<inode>` of the socket it gave the service; the variable alone
/// can be inherited by a child whose stderr went somewhere else, so it only
/// counts when it names the stream stderr really is.
pub fn is_journal_stream(journal_stream: Option<&str>, stderr: Option<(u64, u64)>) -> bool {
    let Some((dev, ino)) = journal_stream.and_then(|v| v.split_once(':')) else { return false };
    match (dev.parse::<u64>(), ino.parse::<u64>(), stderr) {
        (Ok(dev), Ok(ino), Some(actual)) => actual == (dev, ino),
        _ => false,
    }
}

#[cfg(unix)]
fn stderr_identity() -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    // /proc/self/fd/2 follows to the open file, whatever kind it is.
    std::fs::metadata("/proc/self/fd/2").ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn stderr_identity() -> Option<(u64, u64)> {
    None
}

/// Install the global log subscriber. `with_target` is the per-binary choice.
pub fn init(level: Level, with_target: bool) {
    let journald = is_journal_stream(std::env::var("JOURNAL_STREAM").ok().as_deref(), stderr_identity());
    let builder = tracing_subscriber::fmt().with_max_level(level).with_target(with_target);
    if journald {
        builder.without_time().with_ansi(false).init();
    } else {
        builder.init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_journal_variable_means_not_the_journal() {
        assert!(!is_journal_stream(None, Some((8, 1234))));
    }

    #[test]
    fn a_variable_naming_the_stderr_stream_means_the_journal() {
        assert!(is_journal_stream(Some("8:1234"), Some((8, 1234))));
    }

    #[test]
    fn an_inherited_variable_for_another_stream_does_not_count() {
        assert!(!is_journal_stream(Some("8:1234"), Some((8, 99))));
        assert!(!is_journal_stream(Some("8:1234"), Some((9, 1234))));
    }

    #[test]
    fn a_malformed_or_unverifiable_variable_does_not_count() {
        assert!(!is_journal_stream(Some("garbage"), Some((8, 1234))));
        assert!(!is_journal_stream(Some("8:x"), Some((8, 1234))));
        assert!(!is_journal_stream(Some("8:1234"), None), "stderr could not be inspected");
    }
}
