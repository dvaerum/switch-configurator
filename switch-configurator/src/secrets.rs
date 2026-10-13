//! Where SNMP community strings come from, and keeping them out of output.
//!
//! A community is given inline (`name`), or by reference: `name_file` (a path,
//! e.g. a sops-nix secret) or `name_credential` (a systemd `LoadCredential`
//! id, read from `$CREDENTIALS_DIRECTORY`). `resolve_snmp_secrets` reads the
//! reference into `SnmpCommunity::name` when a switch config is validated.
//! Error messages name the switch and the source, never the value.
//!
//! Referenced values are also registered here, so `scrub`/`scrub_json` can mask
//! them in API output that echoes switch state (a community read back from the
//! switch would otherwise show up verbatim). See
//! docs/decisions/0002-secret-redaction-scope.md.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};

use crate::models::{SnmpCommunity, SnmpConfig, SnmpReadsMode, SwitchConfig};

/// Where credential ids are looked up.
#[derive(Debug, Clone, Default)]
pub struct SecretContext {
    credentials_dir: Option<PathBuf>,
}

impl SecretContext {
    /// systemd's `$CREDENTIALS_DIRECTORY`, when the service runs with `LoadCredential=`.
    pub fn from_env() -> Self {
        Self { credentials_dir: std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from) }
    }

    pub fn with_credentials_dir(dir: impl Into<PathBuf>) -> Self {
        Self { credentials_dir: Some(dir.into()) }
    }

    pub fn without_credentials() -> Self {
        Self::default()
    }
}

/// The communities whose string can come from a reference, with a label for
/// error messages: the listed ones, and the service's own `snmp.reads.community`
/// when reads are switched on (a disabled feature is not validated or read).
fn labelled_communities(snmp: &SnmpConfig) -> Vec<(String, &SnmpCommunity)> {
    let mut all: Vec<(String, &SnmpCommunity)> = snmp
        .communities
        .iter()
        .enumerate()
        .map(|(i, c)| (format!("SNMP community #{}", i + 1), c))
        .collect();
    if let Some(reads) = snmp.reads.as_ref().filter(|r| r.mode == SnmpReadsMode::Auto) {
        if let Some(community) = &reads.community {
            all.push(("snmp.reads.community".to_string(), community));
        }
    }
    all
}

fn labelled_communities_mut(snmp: &mut SnmpConfig) -> Vec<(String, &mut SnmpCommunity)> {
    let mut all: Vec<(String, &mut SnmpCommunity)> = snmp
        .communities
        .iter_mut()
        .enumerate()
        .map(|(i, c)| (format!("SNMP community #{}", i + 1), c))
        .collect();
    if let Some(reads) = snmp.reads.as_mut().filter(|r| r.mode == SnmpReadsMode::Auto) {
        if let Some(community) = reads.community.as_mut() {
            all.push(("snmp.reads.community".to_string(), community));
        }
    }
    all
}

/// Every community must name a source, and at most one kind of reference.
/// Structural only (nothing is read), so it also suits overlays that are
/// saved but not yet applied. An inline `name` next to a reference is NOT an
/// error here: that is what an already-resolved config looks like, and
/// `resolve_snmp_secrets` rejects it if the two disagree.
pub fn check_snmp_secret_sources(switch: &SwitchConfig) -> Result<()> {
    let Some(snmp) = &switch.snmp else { return Ok(()) };
    for (label, community) in labelled_communities(snmp) {
        if community.name_file.is_some() && community.name_credential.is_some() {
            bail!(
                "switch '{}' {} sets more than one source: use only one of name_file and name_credential",
                switch.id, label
            );
        }
        if community.name.is_empty() && !community.has_reference() {
            bail!(
                "switch '{}' {} has no source: set one of name, name_file or name_credential",
                switch.id, label
            );
        }
    }
    Ok(())
}

/// Read every referenced community string into `SnmpCommunity::name`.
/// Idempotent: a community whose `name` already equals what its reference
/// resolves to is left alone.
pub fn resolve_snmp_secrets(switch: &mut SwitchConfig, ctx: &SecretContext) -> Result<()> {
    check_snmp_secret_sources(switch)?;
    let switch_id = switch.id.clone();
    let Some(snmp) = switch.snmp.as_mut() else { return Ok(()) };

    for (label, community) in labelled_communities_mut(snmp) {
        let (value, source) = match (&community.name_file, &community.name_credential) {
            (Some(path), _) => (read_secret_file(path, &switch_id, &label)?, format!("name_file {}", path.display())),
            (None, Some(id)) => (read_credential(id, ctx, &switch_id, &label)?, format!("name_credential '{}'", id)),
            (None, None) => continue,
        };
        if !community.name.is_empty() && community.name != value {
            bail!(
                "switch '{}' {} has an inline name that disagrees with its {}: use only one source",
                switch_id, label, source
            );
        }
        register_secret(&value);
        community.name = value;
    }
    Ok(())
}

fn read_secret_file(path: &std::path::Path, switch_id: &str, label: &str) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("switch '{}' {}: cannot read name_file {}", switch_id, label, path.display()))?;
    validate_value(raw, &format!("switch '{}' {} (name_file {})", switch_id, label, path.display()))
}

fn read_credential(id: &str, ctx: &SecretContext, switch_id: &str, community: &str) -> Result<String> {
    let label = format!("switch '{}' {} (name_credential '{}')", switch_id, community, id);
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\0') {
        bail!("{}: credential ids are plain names, not paths", label);
    }
    let Some(dir) = &ctx.credentials_dir else {
        bail!(
            "{}: $CREDENTIALS_DIRECTORY is not set; run the service with LoadCredential={}:<file> (services.switch-configurator.loadCredentials)",
            label,
            id
        );
    };
    let path = dir.join(id);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("{}: cannot read credential file {}", label, path.display()))?;
    validate_value(raw, &label)
}

/// Strip the trailing newline(s) editors and `echo` add, then check the value
/// is usable on a switch CLI. Errors say what is wrong, never show the value.
fn validate_value(raw: String, label: &str) -> Result<String> {
    let value = raw.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        bail!("{}: the secret is empty", label);
    }
    if value.chars().any(|c| c == '"' || c.is_control()) {
        bail!("{}: quotes, newlines and control characters are not allowed in a community string", label);
    }
    Ok(value)
}

fn registry() -> &'static Mutex<HashSet<String>> {
    static REGISTRY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Remember a secret value so `scrub` can mask it.
pub fn register_secret(value: &str) {
    if !value.is_empty() {
        registry().lock().unwrap().insert(value.to_string());
    }
}

/// `text` with every registered secret replaced by `<redacted>`.
pub fn scrub(text: &str) -> String {
    let secrets = registry().lock().unwrap();
    // Longest first, so a secret that contains another is masked as a whole.
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| text.contains(s.as_str())).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for secret in ordered {
        out = out.replace(secret.as_str(), crate::redact::MASK);
    }
    out
}

/// `scrub` applied to every string inside a JSON value (keys untouched).
pub fn scrub_json(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) => Value::String(scrub(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(scrub_json).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, scrub_json(v))).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AllowedHost, ConnectionType, Credentials, SnmpAccess, SnmpCommunity, SnmpConfig, SwitchModel};

    fn switch_with(communities: Vec<SnmpCommunity>) -> SwitchConfig {
        SwitchConfig {
            id: "sw1".to_string(),
            hostname: Some("sw1".to_string()),
            model: Some(SwitchModel::Fortiswitch124F_FPOE),
            management_ip: Some("192.0.2.10".to_string()),
            credentials: Some(Credentials {
                username: "admin".to_string(),
                password: Some("pw".to_string()),
                ssh_key_path: None,
                port: 22,
                connection_type: ConnectionType::Ssh,
                serial_device: None,
                baud_rate: 115200,
                jump_hosts: None,
                enable_secret: None,
            }),
            vlans: vec![],
            ports: vec![],
            port_mirrors: vec![],
            snmp: Some(SnmpConfig { communities, trap_receivers: vec![], enabled_traps: vec![], reads: None }),
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings::default(),
        }
    }

    fn file_community(path: &std::path::Path) -> SnmpCommunity {
        SnmpCommunity { name: String::new(), name_file: Some(path.to_path_buf()), ..SnmpCommunity::new("", SnmpAccess::Operator) }
    }

    fn credential_community(id: &str) -> SnmpCommunity {
        SnmpCommunity { name: String::new(), name_credential: Some(id.to_string()), ..SnmpCommunity::new("", SnmpAccess::Operator) }
    }

    fn community_name(switch: &SwitchConfig) -> String {
        switch.snmp.as_ref().unwrap().communities[0].name.clone()
    }

    // ---- YAML / JSON shape ----

    #[test]
    fn yaml_accepts_each_single_source_and_keeps_inline_configs_working() {
        let inline: SnmpCommunity = serde_yaml::from_str("name: public\naccess: operator").unwrap();
        assert_eq!(inline.name, "public");
        let file: SnmpCommunity = serde_yaml::from_str("name_file: /run/secrets/snmp-ro\naccess: operator").unwrap();
        assert_eq!(file.name_file.unwrap(), PathBuf::from("/run/secrets/snmp-ro"));
        let cred: SnmpCommunity = serde_yaml::from_str("name_credential: snmp-ro").unwrap();
        assert_eq!(cred.name_credential.as_deref(), Some("snmp-ro"));
        assert_eq!(cred.access, SnmpAccess::Unrestricted, "access keeps its default");
    }

    #[test]
    fn allowed_hosts_parse_auto_addresses_and_cidrs() {
        let c: SnmpCommunity =
            serde_yaml::from_str("name: x\nallowed_hosts: [auto, 192.0.2.1, 192.0.2.0/24, '2001:db8::/32']").unwrap();
        assert_eq!(c.allowed_hosts[0], AllowedHost::Auto);
        assert_eq!(c.allowed_hosts[1], AllowedHost::Network("192.0.2.1".to_string()));
        assert_eq!(c.allowed_hosts.len(), 4);
    }

    #[test]
    fn allowed_hosts_reject_garbage() {
        for bad in ["not-an-ip", "192.0.2.1/33", "2001:db8::/129", "192.0.2.1/x"] {
            let yaml = format!("name: x\nallowed_hosts: ['{bad}']");
            assert!(serde_yaml::from_str::<SnmpCommunity>(&yaml).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn a_resolved_reference_serialises_as_the_reference_only() {
        let mut c = file_community(std::path::Path::new("/run/secrets/snmp-ro"));
        c.name = "the-actual-secret".to_string(); // as after resolution
        let json = serde_json::to_string(&c).unwrap();
        let yaml = serde_yaml::to_string(&c).unwrap();
        for text in [&json, &yaml] {
            assert!(!text.contains("the-actual-secret"), "resolved value leaked: {text}");
            assert!(text.contains("/run/secrets/snmp-ro"));
        }
    }

    #[test]
    fn an_inline_community_still_serialises_its_name() {
        let json = serde_json::to_string(&SnmpCommunity::new("public", SnmpAccess::Operator)).unwrap();
        assert!(json.contains("\"name\":\"public\""));
        assert!(!json.contains("name_file"));
    }

    #[test]
    fn serialisation_round_trips() {
        let mut original = SnmpCommunity::new("public", SnmpAccess::Operator);
        original.allowed_hosts = vec![AllowedHost::Auto, AllowedHost::Network("192.0.2.0/24".to_string())];
        let back: SnmpCommunity = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(back, original);
    }

    // ---- exactly one source ----

    #[test]
    fn exactly_one_source_is_required() {
        let ok = switch_with(vec![SnmpCommunity::new("public", SnmpAccess::Operator)]);
        assert!(check_snmp_secret_sources(&ok).is_ok());

        let none = switch_with(vec![SnmpCommunity::new("", SnmpAccess::Operator)]);
        let err = check_snmp_secret_sources(&none).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("no source"), "unhelpful error: {err}");

        let mut two_refs = SnmpCommunity::new("", SnmpAccess::Operator);
        two_refs.name_file = Some(PathBuf::from("/run/secrets/x"));
        two_refs.name_credential = Some("x".to_string());
        let err = check_snmp_secret_sources(&switch_with(vec![two_refs])).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("more than one"), "unhelpful error: {err}");
    }

    #[test]
    fn an_inline_name_that_disagrees_with_its_reference_is_rejected_without_echoing_either() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        std::fs::write(&path, "value-from-file-7").unwrap();
        let mut conflicting = file_community(&path);
        conflicting.name = "value-inline-7".to_string();

        let err = resolve_snmp_secrets(&mut switch_with(vec![conflicting]), &SecretContext::without_credentials())
            .unwrap_err()
            .to_string();

        assert!(err.contains("disagrees"), "unhelpful error: {err}");
        assert!(!err.contains("value-from-file-7") && !err.contains("value-inline-7"), "value echoed: {err}");
    }

    // ---- file sources ----

    #[test]
    fn name_file_is_read_and_its_trailing_newline_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        std::fs::write(&path, "file-secret-1\n").unwrap();
        let mut switch = switch_with(vec![file_community(&path)]);

        resolve_snmp_secrets(&mut switch, &SecretContext::without_credentials()).unwrap();

        assert_eq!(community_name(&switch), "file-secret-1");
    }

    #[test]
    fn resolving_twice_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        std::fs::write(&path, "file-secret-2\n").unwrap();
        let mut switch = switch_with(vec![file_community(&path)]);
        let ctx = SecretContext::without_credentials();

        resolve_snmp_secrets(&mut switch, &ctx).unwrap();
        resolve_snmp_secrets(&mut switch, &ctx).unwrap();

        assert_eq!(community_name(&switch), "file-secret-2");
    }

    #[test]
    fn a_changed_file_is_picked_up_on_the_next_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        let ctx = SecretContext::without_credentials();
        std::fs::write(&path, "old-secret-3").unwrap();
        let mut switch = switch_with(vec![file_community(&path)]);
        resolve_snmp_secrets(&mut switch, &ctx).unwrap();

        // A reload starts again from the reference (name empty), not from the old value.
        std::fs::write(&path, "new-secret-3").unwrap();
        let mut reloaded = switch_with(vec![file_community(&path)]);
        resolve_snmp_secrets(&mut reloaded, &ctx).unwrap();

        assert_eq!(community_name(&reloaded), "new-secret-3");
    }

    #[test]
    fn unreadable_or_empty_or_unsafe_files_fail_without_echoing_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SecretContext::without_credentials();

        let missing = dir.path().join("missing");
        let mut switch = switch_with(vec![file_community(&missing)]);
        let err = resolve_snmp_secrets(&mut switch, &ctx).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("missing"), "unhelpful error: {err}");

        let empty = dir.path().join("empty");
        std::fs::write(&empty, "\n").unwrap();
        let err = resolve_snmp_secrets(&mut switch_with(vec![file_community(&empty)]), &ctx).unwrap_err().to_string();
        assert!(err.contains("empty"), "unhelpful error: {err}");

        for (file, content) in [("quote", "has\"quote"), ("multi", "line1\nline2"), ("ctrl", "bad\u{7}bell")] {
            let path = dir.path().join(file);
            std::fs::write(&path, content).unwrap();
            let err = resolve_snmp_secrets(&mut switch_with(vec![file_community(&path)]), &ctx).unwrap_err().to_string();
            assert!(err.contains("not allowed"), "{file}: unhelpful error: {err}");
            for leak in ["quote", "line2", "bell"] {
                assert!(!err.contains(&format!("has\"{leak}")) && !err.contains("line1"), "value echoed: {err}");
            }
        }
    }

    // ---- credential sources ----

    #[test]
    fn name_credential_is_read_from_the_credentials_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("snmp-ro"), "cred-secret-4\n").unwrap();
        let mut switch = switch_with(vec![credential_community("snmp-ro")]);

        resolve_snmp_secrets(&mut switch, &SecretContext::with_credentials_dir(dir.path())).unwrap();

        assert_eq!(community_name(&switch), "cred-secret-4");
    }

    #[test]
    fn name_credential_without_a_credentials_directory_says_what_is_missing() {
        let mut switch = switch_with(vec![credential_community("snmp-ro")]);
        let err = resolve_snmp_secrets(&mut switch, &SecretContext::without_credentials()).unwrap_err().to_string();
        assert!(err.contains("CREDENTIALS_DIRECTORY") && err.contains("LoadCredential"), "unhelpful error: {err}");
    }

    #[test]
    fn credential_ids_cannot_escape_the_credentials_directory() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../secret", "a/b", "/etc/passwd", "..", ""] {
            let mut switch = switch_with(vec![credential_community(bad)]);
            assert!(
                resolve_snmp_secrets(&mut switch, &SecretContext::with_credentials_dir(dir.path())).is_err(),
                "credential id {bad:?} must be rejected"
            );
        }
    }

    // ---- scrubbing ----

    #[test]
    fn registered_secrets_are_scrubbed_from_text_and_json() {
        register_secret("scrub-me-5");
        assert_eq!(scrub("community scrub-me-5 added"), "community <redacted> added");

        let json = serde_json::json!({"raw": "snmp-server community scrub-me-5 RO", "n": 3, "nested": ["x scrub-me-5"]});
        let out = scrub_json(json);
        assert_eq!(out["raw"], "snmp-server community <redacted> RO");
        assert_eq!(out["nested"][0], "x <redacted>");
        assert_eq!(out["n"], 3);
    }

    #[test]
    fn resolving_registers_the_secret_for_scrubbing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        std::fs::write(&path, "scrub-me-6").unwrap();
        let mut switch = switch_with(vec![file_community(&path)]);

        resolve_snmp_secrets(&mut switch, &SecretContext::without_credentials()).unwrap();

        assert_eq!(scrub("read back: scrub-me-6"), "read back: <redacted>");
    }

    #[test]
    fn scrub_with_nothing_registered_for_the_text_leaves_it_alone() {
        assert_eq!(scrub("nothing secret here"), "nothing secret here");
    }

    // ---- snmp.reads.community ----

    fn with_reads(mode: crate::models::SnmpReadsMode, community: SnmpCommunity) -> SwitchConfig {
        let mut sw = switch_with(vec![]);
        sw.snmp.as_mut().unwrap().reads = Some(crate::models::SnmpReads { mode, community: Some(community) });
        sw
    }

    #[test]
    fn the_reads_community_is_resolved_like_any_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snmp-ro");
        std::fs::write(&path, "reads-secret-14\n").unwrap();
        let mut sw = with_reads(crate::models::SnmpReadsMode::Auto, file_community(&path));

        resolve_snmp_secrets(&mut sw, &SecretContext::without_credentials()).unwrap();

        let community = sw.snmp.as_ref().unwrap().reads.as_ref().unwrap().community.as_ref().unwrap();
        assert_eq!(community.name, "reads-secret-14");
        assert_eq!(scrub("x reads-secret-14"), "x <redacted>", "it is registered for scrubbing too");
    }

    #[test]
    fn a_reads_community_that_cannot_be_read_names_snmp_reads_in_the_error() {
        let mut sw = with_reads(crate::models::SnmpReadsMode::Auto, file_community(std::path::Path::new("/nonexistent/x")));
        let err = resolve_snmp_secrets(&mut sw, &SecretContext::without_credentials()).unwrap_err().to_string();
        assert!(err.contains("sw1") && err.contains("snmp.reads.community"), "unhelpful error: {err}");
    }

    #[test]
    fn a_reads_community_with_no_source_is_an_error_only_when_reads_are_on() {
        let nameless = SnmpCommunity::new("", SnmpAccess::Operator);
        let on = with_reads(crate::models::SnmpReadsMode::Auto, nameless.clone());
        assert!(check_snmp_secret_sources(&on).unwrap_err().to_string().contains("snmp.reads.community"));

        let off = with_reads(crate::models::SnmpReadsMode::Off, nameless);
        assert!(check_snmp_secret_sources(&off).is_ok(), "a disabled feature is not validated");
    }

    #[test]
    fn a_disabled_reads_community_is_not_even_read() {
        let off = &mut with_reads(crate::models::SnmpReadsMode::Off, file_community(std::path::Path::new("/nonexistent/x")));
        assert!(resolve_snmp_secrets(off, &SecretContext::without_credentials()).is_ok());
    }
}
