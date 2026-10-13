//! Masks secrets (SNMP community strings, SNMPv3 passwords) in command text
//! and config dumps, so they don't end up in logs or traces.
//!
//! Syntax-based on purpose: it covers whatever the three vendors' CLIs put
//! on the wire, including values the service never configured itself. It is
//! deliberately NOT applied to API responses that show the user's own
//! authored config (desired-config, preview-diff) or to `raw_config` backups:
//! the UI manages communities by name and `raw_config` is a restorable
//! backup. Secrets that come from files/credentials are protected there by
//! value instead (see the secret sources).

use regex::Regex;
use std::sync::OnceLock;

pub const MASK: &str = "<redacted>";

/// Redacts one command at a time. Stateful because FortiOS puts the
/// community string in a plain `set name "..."` line, which is only a secret
/// inside `config system snmp community ... end`; `set name` elsewhere is an
/// ordinary label (VLAN names, ...).
#[derive(Debug, Default, Clone)]
pub struct CommandRedactor {
    /// Nesting depth while inside `config system snmp community` (0 = outside).
    snmp_community_depth: usize,
    /// The last command was a `show ...`: its output may carry secrets in
    /// any vendor's format (running-config dumps), and wire chunks cut lines
    /// mid-way so they can't be reliably parsed, so wire logging withholds it.
    show_output: bool,
}

struct Patterns {
    community: Regex,
    host_community_keyword: Regex,
    cisco_host: Regex,
    fortios_password: Regex,
    fortios_set_name: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        // Aruba/Cisco: `[no] snmp-server community <string> ...`
        community: Regex::new(r#"(?i)^(\s*(?:no\s+)?snmp-server\s+community\s+)("[^"]*"|\S+)"#).unwrap(),
        // Aruba: `snmp-server host <ip> community "<string>"`
        host_community_keyword: Regex::new(r#"(?i)^(\s*(?:no\s+)?snmp-server\s+host\s+.*?\bcommunity\s+)("[^"]*"|\S+)"#).unwrap(),
        // Cisco: `snmp-server host <ip> [traps|informs] [version 1|2c] <string>`
        cisco_host: Regex::new(
            r#"(?i)^(\s*(?:no\s+)?snmp-server\s+host\s+\S+\s+(?:(?:traps|informs)\s+)?(?:version\s+(?:1|2c)\s+)?)(\S+)"#,
        )
        .unwrap(),
        // FortiOS SNMPv3: `set auth-pwd ...` / `set priv-pwd ...` (value may be `ENC ...`)
        fortios_password: Regex::new(r#"(?i)^(\s*set\s+(?:auth|priv)-pwd\s+).*$"#).unwrap(),
        fortios_set_name: Regex::new(r#"(?i)^(\s*set\s+name\s+).*$"#).unwrap(),
    })
}

impl CommandRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The command with any secret replaced by `MASK`. Call once per line,
    /// in the order the lines are sent/shown.
    pub fn redact(&mut self, command: &str) -> String {
        let p = patterns();
        let trimmed = command.trim();
        self.show_output = trimmed.to_ascii_lowercase().starts_with("show ");

        if self.snmp_community_depth == 0 {
            if trimmed.eq_ignore_ascii_case("config system snmp community") {
                self.snmp_community_depth = 1;
            }
        } else if trimmed.to_ascii_lowercase().starts_with("config ") {
            self.snmp_community_depth += 1;
        } else if trimmed.eq_ignore_ascii_case("end") {
            self.snmp_community_depth -= 1;
        } else if self.snmp_community_depth == 1 && p.fortios_set_name.is_match(command) {
            return p.fortios_set_name.replace(command, format!("${{1}}{MASK}")).into_owned();
        }

        if p.fortios_password.is_match(command) {
            return p.fortios_password.replace(command, format!("${{1}}{MASK}")).into_owned();
        }
        for pattern in [&p.community, &p.host_community_keyword] {
            if pattern.is_match(command) {
                return pattern.replace(command, format!("${{1}}{MASK}")).into_owned();
            }
        }
        if p.cisco_host.is_match(command) && !command.to_ascii_lowercase().contains("version 3") {
            return p.cisco_host.replace(command, format!("${{1}}{MASK}")).into_owned();
        }
        // Whatever the patterns above missed: mask secret values we know
        // (read from files/credentials), wherever they appear.
        crate::secrets::scrub(command)
    }
}

impl CommandRedactor {
    /// Redact received wire text (command echo + output) for trace logs,
    /// using the current block state WITHOUT advancing it. Output of a
    /// `show` command is withheld entirely (see `show_output`).
    pub fn peek_text(&self, text: &str) -> String {
        if self.show_output {
            return format!("<{} bytes of show output withheld>", text.len());
        }
        if self.snmp_community_depth > 0 {
            // The device echo of a `set name "<community>"` line can arrive
            // cut mid-line, which no line pattern would recognise.
            return format!("<{} bytes withheld inside an SNMP community block>", text.len());
        }
        self.peek_sent(text)
    }

    /// Redact text we are about to SEND (the command itself) for trace
    /// logs: line-wise, using the current block state without advancing it,
    /// and never withheld, so a `show ...` command stays visible.
    pub fn peek_sent(&self, text: &str) -> String {
        let mut scratch = self.clone();
        text.split_inclusive('\n')
            .map(|piece| {
                let line = piece.trim_end_matches(['\r', '\n']);
                let tail = &piece[line.len()..];
                format!("{}{}", scratch.redact(line), tail)
            })
            .collect::<String>()
    }
}

/// Redact a whole command list (e.g. a command preview), in order.
pub fn redact_commands(commands: &[String]) -> Vec<String> {
    let mut redactor = CommandRedactor::new();
    commands.iter().map(|c| redactor.redact(c)).collect()
}

/// Redact multi-line text such as a `show running-config` dump.
pub fn redact_config_text(text: &str) -> String {
    let mut redactor = CommandRedactor::new();
    text.lines().map(|l| redactor.redact(l)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> Vec<String> {
        redact_commands(&lines.iter().map(|l| l.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn fortios_community_name_is_masked_only_inside_the_snmp_community_block() {
        let out = run(&[
            "config switch vlan",
            "edit 10",
            "set name \"office\"",
            "next",
            "end",
            "config system snmp community",
            "edit 1",
            "set name \"s3cret\"",
            "set status enable",
            "config hosts",
            "edit 1",
            "set ip 192.0.2.1 255.255.255.255",
            "next",
            "end",
            "set query-v2c-status enable",
            "next",
            "end",
            "config switch vlan",
            "set name \"after\"",
            "end",
        ]);
        assert_eq!(out[2], "set name \"office\"", "VLAN names are not secrets");
        assert_eq!(out[7], format!("set name {MASK}"));
        assert_eq!(out[8], "set status enable");
        assert_eq!(out[11], "set ip 192.0.2.1 255.255.255.255");
        assert_eq!(out[18], "set name \"after\"", "block closed by its matching end");
        assert!(!out.join("\n").contains("s3cret"));
    }

    #[test]
    fn nested_hosts_block_does_not_end_the_community_block_early() {
        let out = run(&[
            "config system snmp community",
            "edit 1",
            "config hosts",
            "end",
            "set name \"s3cret\"",
            "next",
            "end",
        ]);
        assert_eq!(out[4], format!("set name {MASK}"));
    }

    #[test]
    fn fortios_snmpv3_passwords_are_masked_anywhere() {
        let out = run(&["set auth-pwd hunter2hunter2", "  set priv-pwd ENC AbCdEf=="]);
        assert_eq!(out[0], format!("set auth-pwd {MASK}"));
        assert_eq!(out[1], format!("  set priv-pwd {MASK}"));
    }

    #[test]
    fn aruba_community_and_host_lines_are_masked() {
        let out = run(&[
            "snmp-server community \"s3cret\" unrestricted",
            "no snmp-server community \"s3cret\"",
            "snmp-server host 192.0.2.5 community \"s3cret\"",
            "snmp-server community public operator",
        ]);
        assert_eq!(out[0], format!("snmp-server community {MASK} unrestricted"));
        assert_eq!(out[1], format!("no snmp-server community {MASK}"));
        assert_eq!(out[2], format!("snmp-server host 192.0.2.5 community {MASK}"));
        assert_eq!(out[3], format!("snmp-server community {MASK} operator"));
    }

    #[test]
    fn cisco_community_and_host_lines_are_masked() {
        let out = run(&[
            "snmp-server community s3cret RO",
            "snmp-server community s3cret view RO-VIEW RO POLLERS",
            "snmp-server host 192.0.2.5 version 2c s3cret",
            "snmp-server host 192.0.2.5 informs version 2c s3cret",
            "snmp-server host 192.0.2.5 s3cret",
        ]);
        assert_eq!(out[0], format!("snmp-server community {MASK} RO"));
        assert_eq!(out[1], format!("snmp-server community {MASK} view RO-VIEW RO POLLERS"));
        assert_eq!(out[2], format!("snmp-server host 192.0.2.5 version 2c {MASK}"));
        assert_eq!(out[3], format!("snmp-server host 192.0.2.5 informs version 2c {MASK}"));
        assert_eq!(out[4], format!("snmp-server host 192.0.2.5 {MASK}"));
    }

    #[test]
    fn cisco_snmpv3_host_user_is_not_a_community_secret() {
        let out = run(&["snmp-server host 192.0.2.5 version 3 priv poller"]);
        assert_eq!(out[0], "snmp-server host 192.0.2.5 version 3 priv poller");
    }

    #[test]
    fn ordinary_commands_pass_through_unchanged() {
        let lines = ["show running-config", "vlan 10", "name \"snmp-server community\" is a vlan label? no", "end"];
        assert_eq!(run(&lines), lines.to_vec());
    }

    #[test]
    fn config_text_dump_is_redacted_line_by_line() {
        let dump = "hostname sw1\nsnmp-server community s3cret RO\nsnmp-server contact ops\n";
        let out = redact_config_text(dump);
        assert!(!out.contains("s3cret"));
        assert!(out.contains("hostname sw1"));
        assert!(out.contains("snmp-server contact ops"));
    }

    #[test]
    fn redactor_state_does_not_leak_between_instances() {
        let mut a = CommandRedactor::new();
        a.redact("config system snmp community");
        let mut b = CommandRedactor::new();
        assert_eq!(b.redact("set name \"label\""), "set name \"label\"");
    }

    #[test]
    fn show_output_is_withheld_but_sent_commands_stay_visible() {
        let mut redactor = CommandRedactor::new();
        redactor.redact("show running-config");
        assert_eq!(redactor.peek_sent("show running-config\r"), "show running-config\r");
        let shown = redactor.peek_text("snmp-server community s3cret RO\r\n");
        assert!(!shown.contains("s3cret"), "withheld output must not contain the secret: {shown}");

        // The next non-show command ends the withholding.
        redactor.redact("vlan 10");
        assert_eq!(redactor.peek_text("vlan 10\r\n"), "vlan 10\r\n");
    }

    #[test]
    fn wire_output_is_withheld_inside_a_community_block_even_when_chunks_split_a_line() {
        let mut redactor = CommandRedactor::new();
        redactor.redact("config system snmp community");
        redactor.redact("set name \"s3cret-split\"");
        // The device echo can arrive cut mid-line, so no pattern would match it.
        let shown = redactor.peek_text("me \"s3cret-split\"\r\n");
        assert!(!shown.contains("s3cret-split"), "split echo leaked: {shown}");

        redactor.redact("end");
        assert_eq!(redactor.peek_text("plain\r\n"), "plain\r\n", "withholding stops with the block");
    }

    #[test]
    fn registered_secret_values_are_masked_wherever_the_redactor_outputs() {
        crate::secrets::register_secret("registered-secret-13");
        let mut redactor = CommandRedactor::new();
        assert_eq!(redactor.redact("some command with registered-secret-13 in it"), format!("some command with {MASK} in it"));
        assert_eq!(redactor.peek_sent("x registered-secret-13\r"), format!("x {MASK}\r"));
        assert_eq!(redactor.peek_text("y registered-secret-13\r\n"), format!("y {MASK}\r\n"));
    }
}
