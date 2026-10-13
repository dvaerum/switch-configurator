//! Raw SNMP table rows -> the API's types. Pure functions, tested against rows
//! recorded from a FortiSwitch 108F-POE (7.2.8), with MAC addresses replaced
//! by reserved documentation addresses.
//!
//! Where the console and SNMP disagree on shape, SNMP is mapped to the console's
//! shape so a caller can't tell which path answered (apart from fields only one
//! of them has):
//! - speed and duplex only exist for a port that is up (SNMP reports a nominal
//!   speed for a down port; the console prints none);
//! - the same interface name can appear twice (FortiSwitch lists `internal`
//!   twice): the entry that has a speed wins;
//! - a MAC entry's port is the bridge port number, like the console's `port-id`.

use std::collections::BTreeMap;

use super::{Row, SnmpValue};
use crate::models::{MacTableEntry, PortStatusEntry};

// IF-MIB / EtherLike-MIB / POWER-ETHERNET-MIB / BRIDGE-MIB columns.
pub const IF_NAME: &[u32] = &[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 1];
pub const IF_OPER_STATUS: &[u32] = &[1, 3, 6, 1, 2, 1, 2, 2, 1, 8];
pub const IF_HIGH_SPEED: &[u32] = &[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 15];
pub const DOT3_DUPLEX_STATUS: &[u32] = &[1, 3, 6, 1, 2, 1, 10, 7, 2, 1, 19];
pub const PETH_PORT_ADMIN_ENABLE: &[u32] = &[1, 3, 6, 1, 2, 1, 105, 1, 1, 1, 3];
pub const PETH_PORT_DETECTION_STATUS: &[u32] = &[1, 3, 6, 1, 2, 1, 105, 1, 1, 1, 6];
/// The whole dot1dTpFdbEntry row (all three columns below).
pub const FDB_TABLE: &[u32] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1];
pub const FDB_ADDRESS: &[u32] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 1];
pub const FDB_PORT: &[u32] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 2];
pub const FDB_STATUS: &[u32] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 3];

/// The walked columns port status is built from.
#[derive(Debug, Default, Clone)]
pub struct PortStatusRows {
    pub if_name: Vec<Row>,
    pub if_oper_status: Vec<Row>,
    pub if_high_speed: Vec<Row>,
    pub duplex: Vec<Row>,
    pub peth_admin: Vec<Row>,
    pub peth_detection: Vec<Row>,
}

/// The walked columns the MAC table is built from.
#[derive(Debug, Default, Clone)]
pub struct MacTableRows {
    pub address: Vec<Row>,
    pub port: Vec<Row>,
    pub status: Vec<Row>,
}

/// The OID arcs after `column` (the table row index), if `oid` is in that column.
fn index_of<'a>(oid: &'a [u32], column: &[u32]) -> Option<&'a [u32]> {
    oid.strip_prefix(column)
}

fn by_index(rows: &[Row], column: &[u32]) -> BTreeMap<Vec<u32>, SnmpValue> {
    rows.iter()
        .filter_map(|(oid, value)| index_of(oid, column).map(|i| (i.to_vec(), value.clone())))
        .collect()
}

/// One entry per interface, in ifIndex order, in the console's shape.
/// `supports_poe(name)` says whether a port can carry PoE at all, so ports
/// without it never report a PoE state.
pub fn port_status_from(rows: &PortStatusRows, supports_poe: impl Fn(&str) -> bool) -> Vec<PortStatusEntry> {
    let names = by_index(&rows.if_name, IF_NAME);
    let oper = by_index(&rows.if_oper_status, IF_OPER_STATUS);
    let speed = by_index(&rows.if_high_speed, IF_HIGH_SPEED);
    let duplex = by_index(&rows.duplex, DOT3_DUPLEX_STATUS);
    let peth_admin = by_index(&rows.peth_admin, PETH_PORT_ADMIN_ENABLE);
    let peth_detection = by_index(&rows.peth_detection, PETH_PORT_DETECTION_STATUS);

    let int_at = |table: &BTreeMap<Vec<u32>, SnmpValue>, index: &[u32]| table.get(index).and_then(SnmpValue::as_int);

    let mut entries: Vec<PortStatusEntry> = Vec::new();
    // BTreeMap iterates in index order (ifIndex is one arc, so numeric order).
    for (index, name_value) in &names {
        let Some(name) = name_value.as_text().filter(|n| !n.is_empty()) else { continue };
        let up = int_at(&oper, index) == Some(1);
        let mbps = int_at(&speed, index).filter(|&n| n > 0);

        let poe_status = if supports_poe(&name) {
            // The power table is indexed group.port; the port number is the digits of `portN`.
            let port_number = name.strip_prefix("port").and_then(|n| n.parse::<u32>().ok());
            port_number.and_then(|port| {
                let row: Vec<u32> = vec![1, port];
                if int_at(&peth_admin, &row) == Some(2) {
                    return Some("disabled".to_string());
                }
                int_at(&peth_detection, &row).map(|status| match status {
                    1 => "disabled",
                    2 => "searching",
                    3 => "delivering",
                    4 | 6 => "fault",
                    5 => "test",
                    _ => "unknown",
                }
                .to_string())
            })
        } else {
            None
        };

        let entry = PortStatusEntry {
            port_id: name.clone(),
            link_status: oper.get(index).map(|_| if up { "up" } else { "down" }.to_string()),
            speed: if up { mbps.map(|n| n.to_string()) } else { None },
            duplex: if up {
                match int_at(&duplex, index) {
                    Some(3) => Some("full".to_string()),
                    Some(2) => Some("half".to_string()),
                    _ => None,
                }
            } else {
                None
            },
            poe_status,
        };

        // The same name can be listed twice: keep the entry that has a speed.
        match entries.iter_mut().find(|e| e.port_id == name) {
            Some(existing) if existing.speed.is_none() && entry.speed.is_some() => *existing = entry,
            Some(_) => {}
            None => entries.push(entry),
        }
    }
    entries
}

/// The learned MAC table in the console's shape (port = bridge port number,
/// no VLAN: BRIDGE-MIB has none).
pub fn mac_table_from(rows: &MacTableRows) -> Vec<MacTableEntry> {
    let address = by_index(&rows.address, FDB_ADDRESS);
    let port = by_index(&rows.port, FDB_PORT);
    let status = by_index(&rows.status, FDB_STATUS);

    let mut entries = Vec::new();
    for (index, port_value) in &port {
        // other(1) and invalid(2) are not usable entries; learned(3), self(4), mgmt(5) are.
        if let Some(st) = status.get(index).and_then(SnmpValue::as_int) {
            if !(3..=5).contains(&st) {
                continue;
            }
        }
        let Some(port_number) = port_value.as_int() else { continue };

        // The address column holds the MAC; if it's absent, standard BRIDGE-MIB
        // indexing puts the six MAC bytes in the row index.
        let bytes: Option<Vec<u8>> = match address.get(index).and_then(SnmpValue::as_octets) {
            Some(octets) if octets.len() == 6 => Some(octets.to_vec()),
            Some(_) => None,
            None if index.len() == 6 && index.iter().all(|&a| a <= 255) => Some(index.iter().map(|&a| a as u8).collect()),
            None => None,
        };
        let Some(bytes) = bytes else { continue };

        entries.push(MacTableEntry {
            mac_address: bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":"),
            vlan_id: None,
            port_id: port_number.to_string(),
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(column: &[u32], index: &[u32]) -> Vec<u32> {
        column.iter().chain(index.iter()).copied().collect()
    }

    fn int(column: &[u32], index: &[u32], value: i64) -> Row {
        (oid(column, index), SnmpValue::Integer(value))
    }

    fn lab_rows() -> PortStatusRows {
        crate::snmp::testing::lab_port_rows()
    }

    fn poe_ports_1_to_8(name: &str) -> bool {
        name.strip_prefix("port").and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n <= 8)
    }

    fn entry<'a>(entries: &'a [PortStatusEntry], name: &str) -> &'a PortStatusEntry {
        entries.iter().find(|e| e.port_id == name).unwrap_or_else(|| panic!("{name} missing: {entries:?}"))
    }

    #[test]
    fn an_up_port_reports_link_speed_and_duplex_like_the_console() {
        let entries = port_status_from(&lab_rows(), poe_ports_1_to_8);
        let port1 = entry(&entries, "port1");
        assert_eq!(port1.link_status.as_deref(), Some("up"));
        assert_eq!(port1.speed.as_deref(), Some("1000"));
        assert_eq!(port1.duplex.as_deref(), Some("full"));
    }

    #[test]
    fn a_down_port_has_no_speed_or_duplex_even_though_snmp_reports_a_nominal_one() {
        let entries = port_status_from(&lab_rows(), poe_ports_1_to_8);
        let port2 = entry(&entries, "port2");
        assert_eq!(port2.link_status.as_deref(), Some("down"));
        assert_eq!(port2.speed, None);
        assert_eq!(port2.duplex, None);
    }

    #[test]
    fn the_duplicate_internal_interface_appears_once_with_its_speed() {
        let entries = port_status_from(&lab_rows(), poe_ports_1_to_8);
        let internals: Vec<_> = entries.iter().filter(|e| e.port_id == "internal").collect();
        assert_eq!(internals.len(), 1);
        assert_eq!(internals[0].speed.as_deref(), Some("1000"));
    }

    #[test]
    fn entries_follow_ifindex_order_and_cover_every_interface() {
        let entries = port_status_from(&lab_rows(), poe_ports_1_to_8);
        let names: Vec<&str> = entries.iter().map(|e| e.port_id.as_str()).collect();
        assert_eq!(
            names,
            vec!["port1", "port2", "port3", "port4", "port5", "port6", "port7", "port8", "port9", "port10", "internal"]
        );
    }

    #[test]
    fn poe_state_comes_from_the_power_table_for_poe_capable_ports_only() {
        let mut rows = lab_rows();
        // port3 is delivering power; port4 is administratively disabled; port5 faults.
        rows.peth_detection[2].1 = SnmpValue::Integer(3);
        rows.peth_admin[3].1 = SnmpValue::Integer(2);
        rows.peth_detection[4].1 = SnmpValue::Integer(4);

        let entries = port_status_from(&rows, poe_ports_1_to_8);

        assert_eq!(entry(&entries, "port1").poe_status.as_deref(), Some("searching"));
        assert_eq!(entry(&entries, "port3").poe_status.as_deref(), Some("delivering"));
        assert_eq!(entry(&entries, "port4").poe_status.as_deref(), Some("disabled"));
        assert_eq!(entry(&entries, "port5").poe_status.as_deref(), Some("fault"));
        assert_eq!(entry(&entries, "port9").poe_status, None, "an SFP port has no PoE");
        assert_eq!(entry(&entries, "internal").poe_status, None);
    }

    #[test]
    fn half_duplex_and_unknown_oper_states_are_mapped_sensibly() {
        let mut rows = lab_rows();
        rows.duplex[0].1 = SnmpValue::Integer(2); // port1 half duplex
        rows.if_oper_status[8].1 = SnmpValue::Integer(5); // port9 dormant: not up
        let entries = port_status_from(&rows, poe_ports_1_to_8);
        assert_eq!(entry(&entries, "port1").duplex.as_deref(), Some("half"));
        assert_eq!(entry(&entries, "port9").link_status.as_deref(), Some("down"));
    }

    #[test]
    fn missing_columns_leave_those_fields_empty_instead_of_failing() {
        let mut rows = lab_rows();
        rows.duplex.clear();
        rows.peth_admin.clear();
        rows.peth_detection.clear();
        let entries = port_status_from(&rows, poe_ports_1_to_8);
        let port1 = entry(&entries, "port1");
        assert_eq!((port1.duplex.clone(), port1.poe_status.clone()), (None, None));
        assert_eq!(port1.speed.as_deref(), Some("1000"));
    }

    #[test]
    fn an_empty_walk_gives_an_empty_list() {
        assert!(port_status_from(&PortStatusRows::default(), poe_ports_1_to_8).is_empty());
    }

    // ---- MAC table ----

    fn lab_fdb() -> MacTableRows {
        crate::snmp::testing::lab_fdb()
    }

    #[test]
    fn the_mac_is_read_from_the_value_not_the_row_index() {
        let entries = mac_table_from(&lab_fdb());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].mac_address, "00:00:5e:00:53:01");
        assert_eq!(entries[0].port_id, "1", "bridge port number, like the console's port-id");
        assert_eq!(entries[2].port_id, "11");
        assert!(entries.iter().all(|e| e.vlan_id.is_none()), "BRIDGE-MIB has no VLAN");
    }

    #[test]
    fn invalid_and_other_entries_are_dropped_learned_self_and_mgmt_are_kept() {
        let mut rows = lab_fdb();
        rows.status[0].1 = SnmpValue::Integer(2); // invalid
        rows.status[1].1 = SnmpValue::Integer(1); // other
        rows.status[2].1 = SnmpValue::Integer(5); // mgmt
        let entries = mac_table_from(&rows);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mac_address, "00:00:5e:00:53:03");
    }

    #[test]
    fn the_standard_index_that_is_the_mac_itself_also_works() {
        // BRIDGE-MIB's own indexing: the row index IS the six MAC bytes.
        let idx = [0u32, 0, 0x5e, 0, 0x53, 0x09];
        let rows = MacTableRows {
            address: vec![],
            port: vec![int(FDB_PORT, &idx, 4)],
            status: vec![int(FDB_STATUS, &idx, 3)],
        };
        let entries = mac_table_from(&rows);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mac_address, "00:00:5e:00:53:09");
    }

    #[test]
    fn a_row_without_a_port_or_with_a_short_address_is_skipped() {
        let mut rows = lab_fdb();
        rows.port.remove(0);
        rows.address[1].1 = SnmpValue::Octets(vec![1, 2, 3]);
        let entries = mac_table_from(&rows);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mac_address, "00:00:5e:00:53:03");
    }

    #[test]
    fn an_empty_fdb_gives_an_empty_list() {
        assert!(mac_table_from(&MacTableRows::default()).is_empty());
    }
}
