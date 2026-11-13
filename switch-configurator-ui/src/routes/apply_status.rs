//! What `/api/status` says about a switch's last apply, in the form both the
//! dashboard card and the switch's overview page show: the state (for the
//! badge), a one-line reason when it failed, and the full message.

use serde_json::Value;

/// Longest one-line reason shown on a card.
const SUMMARY_MAX_CHARS: usize = 220;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyStatus {
    /// `success`, `failed`, `configuring`, or `None` if never applied.
    pub state: Option<String>,
    /// First line of the failure, with the plumbing prefixes removed.
    pub summary: Option<String>,
    /// The full failure message exactly as the service recorded it.
    pub detail: Option<String>,
    pub last_applied: Option<String>,
}

/// The one-line reason of a recorded failure (`failed: <error>`): the first
/// line of the error without the `Failed to connect: SSH connection error:`
/// wrapper every connect failure carries, shortened to one card line.
pub fn failure_summary(last_result: &str) -> String {
    let error = last_result.strip_prefix("failed:").unwrap_or(last_result).trim();
    let first_line = error.lines().next().unwrap_or("").trim();
    let mut line = first_line;
    for prefix in ["Failed to connect: ", "SSH connection error: "] {
        line = line.strip_prefix(prefix).unwrap_or(line);
    }
    if line.chars().count() > SUMMARY_MAX_CHARS {
        let cut: String = line.chars().take(SUMMARY_MAX_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// The apply status of `switch_id` from a `/api/status` response; an apply in
/// progress wins over a stale last result.
pub fn from_status(status_json: Option<&Value>, switch_id: &str) -> ApplyStatus {
    let Some(status) = status_json else { return ApplyStatus::default() };
    let configuring = status["currently_configuring"]
        .as_array()
        .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(switch_id)));
    let entry = status["switches"]
        .as_array()
        .and_then(|switches| switches.iter().find(|s| s["id"].as_str() == Some(switch_id)));
    let last_result = entry.and_then(|s| s["last_result"].as_str());
    let last_applied = entry.and_then(|s| s["last_applied"].as_str()).map(str::to_string);

    let failed = last_result.filter(|r| r.starts_with("failed"));
    let state = if configuring {
        Some("configuring".to_string())
    } else {
        match last_result {
            Some(r) if r.starts_with("success") => Some("success".to_string()),
            Some(r) if r.starts_with("failed") => Some("failed".to_string()),
            _ => None,
        }
    };
    ApplyStatus {
        summary: failed.filter(|_| !configuring).map(failure_summary),
        detail: failed
            .filter(|_| !configuring)
            .map(|r| r.strip_prefix("failed:").unwrap_or(r).trim().to_string()),
        state,
        last_applied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ARUBA: &str = "failed: Failed to connect: SSH connection error: Serial connection failed after 3 attempts to /dev/serial_aruba-2530-8g-poe+: Serial device does not exist: /dev/serial_aruba-2530-8g-poe+\n\nPossible causes:\n- Device not connected to the system\n- Incorrect device path in configuration";

    fn status(result: Option<&str>, configuring: &[&str]) -> Value {
        let mut sw = json!({"id": "sw1", "last_applied": "2026-11-13T16:16:28Z"});
        if let Some(r) = result {
            sw["last_result"] = json!(r);
        }
        json!({"currently_configuring": configuring, "switches": [sw]})
    }

    #[test]
    fn a_connect_failure_is_summarised_in_one_line_without_the_plumbing_prefixes() {
        assert_eq!(
            failure_summary(ARUBA),
            "Serial connection failed after 3 attempts to /dev/serial_aruba-2530-8g-poe+: Serial device does not exist: /dev/serial_aruba-2530-8g-poe+"
        );
    }

    #[test]
    fn a_very_long_error_is_cut_to_one_card_line() {
        let long = format!("failed: {}", "x".repeat(500));
        let summary = failure_summary(&long);
        assert!(summary.chars().count() <= SUMMARY_MAX_CHARS + 1, "{}", summary.chars().count());
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn a_failed_switch_carries_its_reason_and_the_full_message() {
        let s = from_status(Some(&status(Some(ARUBA), &[])), "sw1");
        assert_eq!(s.state.as_deref(), Some("failed"));
        assert!(s.summary.unwrap().starts_with("Serial connection failed after 3 attempts"));
        assert!(s.detail.unwrap().contains("Possible causes:"));
        assert_eq!(s.last_applied.as_deref(), Some("2026-11-13T16:16:28Z"));
    }

    #[test]
    fn success_and_never_applied_have_no_failure_text() {
        let ok = from_status(Some(&status(Some("success"), &[])), "sw1");
        assert_eq!((ok.state.as_deref(), ok.summary, ok.detail), (Some("success"), None, None));
        let never = from_status(Some(&status(None, &[])), "sw1");
        assert_eq!(never.state, None);
        assert_eq!(from_status(None, "sw1"), ApplyStatus::default());
    }

    #[test]
    fn an_apply_in_progress_hides_a_stale_failure() {
        let s = from_status(Some(&status(Some(ARUBA), &["sw1"])), "sw1");
        assert_eq!(s.state.as_deref(), Some("configuring"));
        assert_eq!(s.summary, None);
    }
}
