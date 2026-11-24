//! Port status and MAC table over SNMP: pick the target from the switch config,
//! walk the tables, map them.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{bail, Result};

use super::client::{Session, SnmpTarget};
use super::gate::{Gate, Walked};
use super::mapping::*;
use crate::models::{MacTableEntry, PoeUnit, PortStatusEntry, SwitchConfig};

/// The agent cuts a walk short when walks overlap (see `gate`), so every
/// lookup goes through a gate: one at a time per switch, shared by callers.
static MAC_GATE: LazyLock<Gate<Vec<MacTableEntry>>> = LazyLock::new(Gate::new);
static PORT_GATE: LazyLock<Gate<PortStatusRows>> = LazyLock::new(Gate::new);
static UNIT_GATE: LazyLock<Gate<Option<PoeUnit>>> = LazyLock::new(Gate::new);

/// Walks repeated because the first result was short, per switch, since the service started.
static REWALKS: LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> = LazyLock::new(Default::default);

fn note_rewalks<T>(key: &str, walked: &Walked<T>) {
    // A caller that shared another caller's walk did not walk itself.
    if walked.shared || walked.attempts <= 1 {
        return;
    }
    let mut counts = REWALKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(key.to_string()).or_insert(0) += u64::from(walked.attempts - 1);
}

/// How many extra walks the gate has needed for this switch (0 for a healthy agent).
pub fn rewalks_for(target: &SnmpTarget) -> u64 {
    REWALKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&switch_key(target)).copied().unwrap_or(0)
}

fn switch_key(target: &SnmpTarget) -> String {
    format!("{}:{}", target.host, target.port)
}

/// What the response note says about how the walk went: rows, time, and
/// whether it was shared or repeated.
fn walk_summary<T>(walked: &Walked<T>) -> String {
    let mut text = format!("{} entries in {:.1} s", walked.entries, walked.elapsed.as_secs_f64());
    if walked.attempts > 1 {
        text.push_str(&format!(", walked {} times because the first result was short", walked.attempts));
    }
    if walked.shared {
        text.push_str(", shared with a concurrent request");
    }
    text
}

/// How long one SNMP request may take before it counts as lost.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Where to send SNMP reads for this switch, or `None` when the service has no
/// read access: `snmp.reads` off, or no community resolved.
pub fn target_for(switch: &SwitchConfig) -> Option<SnmpTarget> {
    if !crate::snmp_reads::reads_enabled(switch) {
        return None;
    }
    let community = switch.snmp.as_ref()?.reads.as_ref()?.community.as_ref()?.name.clone();
    if community.is_empty() {
        return None;
    }
    Some(SnmpTarget {
        host: switch.management_ip().to_string(),
        port: switch.settings.snmp_port,
        community,
        timeout: REQUEST_TIMEOUT,
    })
}

/// Per-port link, speed, duplex and PoE state. The text returned in place of
/// console output only says where the data came from.
pub async fn fetch_port_status(
    target: &SnmpTarget,
    supports_poe: impl Fn(&str) -> bool,
) -> Result<(Vec<PortStatusEntry>, String)> {
    let walking = target.clone();
    let walked = PORT_GATE
        .run(&switch_key(target), move || {
            let target = walking.clone();
            async move {
                let session = Session::connect(&target).await?;
                let (if_name, if_oper_status, if_high_speed, duplex, peth) = tokio::try_join!(
                    session.walk(IF_NAME),
                    session.walk(IF_OPER_STATUS),
                    session.walk(IF_HIGH_SPEED),
                    session.walk(DOT3_DUPLEX_STATUS),
                    session.walk(PETH_PORT_TABLE),
                )?;
                if if_name.is_empty() {
                    bail!(
                        "{}:{} answered over SNMP but lists no interfaces (IF-MIB not populated)",
                        target.host,
                        target.port
                    );
                }
                let entries = if_name.len();
                // The power table is walked once and split by column.
                let column = |wanted: &[u32]| -> Vec<_> { peth.iter().filter(|(oid, _)| oid.starts_with(wanted)).cloned().collect() };
                let peth_admin = column(PETH_PORT_ADMIN_ENABLE);
                let peth_detection = column(PETH_PORT_DETECTION_STATUS);
                let peth_class = column(PETH_PORT_POWER_CLASS);
                let peth_counters: Vec<_> = [
                    PETH_PORT_MPS_ABSENT_COUNTER,
                    PETH_PORT_INVALID_SIGNATURE_COUNTER,
                    PETH_PORT_POWER_DENIED_COUNTER,
                    PETH_PORT_OVERLOAD_COUNTER,
                    PETH_PORT_SHORT_COUNTER,
                ]
                .into_iter()
                .flat_map(column)
                .collect();
                Ok((PortStatusRows { if_name, if_oper_status, if_high_speed, duplex, peth_admin, peth_detection, peth_class, peth_counters }, entries))
            }
        })
        .await?;
    note_rewalks(&switch_key(target), &walked);
    let entries = port_status_from(&walked.value, supports_poe);
    let note = format!("(read over SNMP from {}:{}; {}; no console output)", target.host, target.port, walk_summary(&walked));
    Ok((entries, note))
}

/// The unit's PoE totals (budget, consumption), or `None` when the switch has no PoE table.
pub async fn fetch_poe_unit(target: &SnmpTarget) -> Result<Option<PoeUnit>> {
    let walking = target.clone();
    let walked = UNIT_GATE
        .run(&switch_key(target), move || {
            let target = walking.clone();
            async move {
                let session = Session::connect(&target).await?;
                let rows = session.walk(PETH_MAIN_PSE_TABLE).await?;
                let entries = rows.len();
                Ok((poe_unit_from(&rows), entries))
            }
        })
        .await?;
    note_rewalks(&switch_key(target), &walked);
    Ok((*walked.value).clone())
}

/// The learned MAC table (BRIDGE-MIB: no VLAN).
///
/// Walked ONCE, front to back, and split by column: a FortiSwitch's agent
/// answers nothing (the request times out) when walks of the FDB table overlap,
/// so the columns can't be fetched side by side the way the interface tables are.
pub async fn fetch_mac_table(target: &SnmpTarget) -> Result<(Vec<MacTableEntry>, String)> {
    let walking = target.clone();
    let walked = MAC_GATE
        .run(&switch_key(target), move || {
            let target = walking.clone();
            async move {
                let session = Session::connect(&target).await?;
                let rows = session.walk(FDB_TABLE).await?;
                let column = |wanted: &[u32]| -> Vec<_> { rows.iter().filter(|(oid, _)| oid.starts_with(wanted)).cloned().collect() };
                let entries = mac_table_from(&MacTableRows {
                    address: column(FDB_ADDRESS),
                    port: column(FDB_PORT),
                    status: column(FDB_STATUS),
                });
                let count = entries.len();
                Ok((entries, count))
            }
        })
        .await?;
    note_rewalks(&switch_key(target), &walked);
    let note = format!(
        "(read over SNMP from {}:{}; {}; no console output; BRIDGE-MIB has no VLAN)",
        target.host,
        target.port,
        walk_summary(&walked)
    );
    Ok(((*walked.value).clone(), note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ConnectionType, Credentials, SnmpAccess, SnmpCommunity, SnmpConfig, SnmpReads, SnmpReadsMode, SwitchModel,
    };
    use crate::snmp::testing::{agent_rows, agent_serving, lab_agent_rows};

    fn switch(reads: Option<SnmpReads>) -> SwitchConfig {
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
            snmp: Some(SnmpConfig { communities: vec![], trap_receivers: vec![], enabled_traps: vec![], reads }),
            validation: None,
            vendor_specific: std::collections::HashMap::new(),
            management_vlan: None,
            settings: crate::config::Settings { snmp_port: 1161, ..crate::config::Settings::default() },
        }
    }

    fn reads(mode: SnmpReadsMode, name: &str) -> Option<SnmpReads> {
        Some(SnmpReads { mode, community: Some(SnmpCommunity::new(name, SnmpAccess::Operator)) })
    }

    #[test]
    fn the_target_is_the_management_ip_with_the_configured_port_and_the_reads_community() {
        let target = target_for(&switch(reads(SnmpReadsMode::Auto, "svc-secret"))).unwrap();
        assert_eq!((target.host.as_str(), target.port), ("192.0.2.10", 1161));
        assert_eq!(target.community, "svc-secret");
    }

    #[test]
    fn there_is_no_target_when_reads_are_off_or_the_community_is_empty() {
        assert!(target_for(&switch(reads(SnmpReadsMode::Off, "svc-secret"))).is_none());
        assert!(target_for(&switch(None)).is_none());
        assert!(target_for(&switch(reads(SnmpReadsMode::Auto, ""))).is_none(), "an unresolved secret is not a credential");
    }

    fn target(addr: std::net::SocketAddr) -> SnmpTarget {
        SnmpTarget { host: addr.ip().to_string(), port: addr.port(), community: "public".to_string(), timeout: Duration::from_millis(500) }
    }

    fn poe_ports_1_to_8(name: &str) -> bool {
        name.strip_prefix("port").and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n <= 8)
    }

    #[tokio::test]
    async fn port_status_is_read_over_snmp_and_matches_the_console_shape() {
        let (addr, _agent) = agent_serving(lab_agent_rows()).await;

        let (entries, note) = fetch_port_status(&target(addr), poe_ports_1_to_8).await.unwrap();

        assert_eq!(entries.len(), 11);
        let port1 = entries.iter().find(|e| e.port_id == "port1").unwrap();
        assert_eq!(
            (port1.link_status.as_deref(), port1.speed.as_deref(), port1.duplex.as_deref(), port1.poe_status.as_deref()),
            (Some("up"), Some("1000"), Some("full"), Some("searching"))
        );
        let port2 = entries.iter().find(|e| e.port_id == "port2").unwrap();
        assert_eq!((port2.link_status.as_deref(), port2.speed.clone()), (Some("down"), None));
        assert!(note.to_lowercase().contains("snmp"), "raw_output should say where the data came from: {note}");
    }

    #[tokio::test]
    async fn a_switch_that_answers_but_lists_no_interfaces_is_an_error_not_an_empty_success() {
        // An agent with only unrelated data (e.g. SNMP on, MIBs not populated).
        let (addr, _agent) = agent_serving(agent_rows(&[&[(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], crate::snmp::SnmpValue::Octets(b"x".to_vec()))]])).await;
        let err = fetch_port_status(&target(addr), poe_ports_1_to_8).await.unwrap_err().to_string();
        assert!(err.contains("no interfaces"), "unhelpful error: {err}");
    }

    #[tokio::test]
    async fn an_unreachable_switch_is_an_error_naming_the_address_not_the_community() {
        let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut t = target(silent.local_addr().unwrap());
        t.community = "top-secret-community".to_string();
        t.timeout = Duration::from_millis(150);

        let err = format!("{:#}", fetch_port_status(&t, poe_ports_1_to_8).await.unwrap_err());

        assert!(err.contains("127.0.0.1"), "unhelpful error: {err}");
        assert!(!err.contains("top-secret-community"), "community leaked: {err}");
    }

    #[tokio::test]
    async fn the_mac_table_is_read_without_vlans() {
        let (addr, _agent) = agent_serving(lab_agent_rows()).await;

        let (entries, _) = fetch_mac_table(&target(addr)).await.unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].mac_address, "00:00:5e:00:53:01");
        assert!(entries.iter().all(|e| e.vlan_id.is_none()));
    }

    #[tokio::test]
    async fn an_empty_mac_table_is_a_valid_answer() {
        let p = crate::snmp::testing::lab_port_rows();
        let (addr, _agent) = agent_serving(agent_rows(&[&p.if_name])).await;
        let (entries, _) = fetch_mac_table(&target(addr)).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn the_mac_table_walk_never_has_two_requests_in_flight() {
        // The lab FortiSwitch times out when several FDB walks overlap, so the
        // table is walked once, front to back.
        use crate::snmp::testing::agent_serving_tracked;
        let (addr, _agent, in_flight) = agent_serving_tracked(lab_agent_rows(), Duration::from_millis(8)).await;

        let (entries, _) = fetch_mac_table(&target(addr)).await.unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(in_flight.max_seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
