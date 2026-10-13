//! Test-only helpers: an in-process SNMP agent and the tables of the lab
//! FortiSwitch 108F-POE (7.2.8) as rows, with MAC addresses replaced by
//! reserved documentation addresses.

use std::sync::Arc;

use async_snmp::agent::Agent;
use async_snmp::handler::{BoxFuture, GetNextResult, GetResult, HandlerResult, MibHandler, OidTable, RequestContext};
use async_snmp::{Oid, Value, VarBind};

use super::mapping::*;
use super::{Row, SnmpValue};

/// Tracks how many requests are being handled at once, and holds each for
/// `delay` so overlapping requests actually overlap. Real switch agents (the
/// FortiSwitch's, for one) can't take several walks of the FDB table at once.
#[derive(Default)]
pub struct InFlight {
    current: std::sync::atomic::AtomicUsize,
    pub max_seen: std::sync::atomic::AtomicUsize,
}

struct TableHandler {
    table: OidTable<Value>,
    in_flight: Arc<InFlight>,
    delay: std::time::Duration,
}

impl TableHandler {
    async fn track(&self) {
        use std::sync::atomic::Ordering;
        let now = self.in_flight.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.in_flight.max_seen.fetch_max(now, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.in_flight.current.fetch_sub(1, Ordering::SeqCst);
    }
}

impl MibHandler for TableHandler {
    fn get<'a>(&'a self, _ctx: &'a RequestContext, oid: &'a Oid) -> BoxFuture<'a, HandlerResult<GetResult>> {
        Box::pin(async move {
            self.track().await;
            Ok(self.table.get(oid).cloned().map(GetResult::Value).unwrap_or(GetResult::NoSuchObject))
        })
    }
    fn get_next<'a>(&'a self, _ctx: &'a RequestContext, oid: &'a Oid) -> BoxFuture<'a, HandlerResult<GetNextResult>> {
        Box::pin(async move {
            self.track().await;
            Ok(self
                .table
                .get_next(oid)
                .map(|(next, value)| GetNextResult::Value(VarBind::new(next.clone(), value.clone())))
                .unwrap_or(GetNextResult::EndOfMibView))
        })
    }
}

/// An in-process agent serving `rows` under 1.3.6.1.2.1 with community `public`.
pub async fn agent_serving(rows: Vec<(Vec<u32>, Value)>) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let (addr, task, _) = agent_serving_tracked(rows, std::time::Duration::ZERO).await;
    (addr, task)
}

/// Like `agent_serving`, but each request takes `delay` and the highest number
/// of requests in flight at once is recorded.
pub async fn agent_serving_tracked(
    rows: Vec<(Vec<u32>, Value)>,
    delay: std::time::Duration,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>, Arc<InFlight>) {
    let table: OidTable<Value> = rows.into_iter().map(|(o, v)| (Oid::from_slice(&o), v)).collect();
    let in_flight = Arc::new(InFlight::default());
    let handler = TableHandler { table, in_flight: in_flight.clone(), delay };
    let agent = Agent::builder()
        .bind("127.0.0.1:0")
        .community("public")
        .allow_all_access()
        .handler(Oid::from_slice(&[1, 3, 6, 1, 2, 1]), Arc::new(handler))
        .build()
        .await
        .expect("agent");
    let addr = agent.local_addr();
    let task = tokio::spawn(async move {
        let _ = agent.run().await;
    });
    (addr, task, in_flight)
}

fn oid(column: &[u32], index: &[u32]) -> Vec<u32> {
    column.iter().chain(index.iter()).copied().collect()
}

fn int(column: &[u32], index: &[u32], value: i64) -> Row {
    (oid(column, index), SnmpValue::Integer(value))
}

fn text(column: &[u32], index: &[u32], value: &str) -> Row {
    (oid(column, index), SnmpValue::Octets(value.as_bytes().to_vec()))
}

/// Ports 1-10 (8 PoE + 2 SFP) and `internal` twice. port1, port9, port10 and
/// internal are up at 1000 full; the rest are down but still report a nominal
/// 1000 and duplex "unknown" (1). The PoE table covers ports 1-11, all enabled
/// and nothing powered.
pub fn lab_port_rows() -> PortStatusRows {
    let names = ["port1", "port2", "port3", "port4", "port5", "port6", "port7", "port8", "port9", "port10", "internal", "internal"];
    let up = [1, 9, 10, 11];
    let mut rows = PortStatusRows::default();
    for (i, name) in names.iter().enumerate() {
        let idx = i as u32 + 1;
        rows.if_name.push(text(IF_NAME, &[idx], name));
        rows.if_oper_status.push(int(IF_OPER_STATUS, &[idx], if up.contains(&idx) { 1 } else { 2 }));
        rows.if_high_speed.push(int(IF_HIGH_SPEED, &[idx], if idx == 12 { 0 } else { 1000 }));
        if idx != 12 {
            rows.duplex.push(int(DOT3_DUPLEX_STATUS, &[idx], if up.contains(&idx) { 3 } else { 1 }));
        }
    }
    for port in 1..=11u32 {
        rows.peth_admin.push(int(PETH_PORT_ADMIN_ENABLE, &[1, port], 1));
        rows.peth_detection.push(int(PETH_PORT_DETECTION_STATUS, &[1, port], 2));
    }
    rows
}

/// FortiSwitch numbers FDB rows with a counter (1, 2, 3...), not the MAC.
pub fn lab_fdb() -> MacTableRows {
    let macs: [[u8; 6]; 3] = [
        [0x00, 0x00, 0x5e, 0x00, 0x53, 0x01],
        [0x00, 0x00, 0x5e, 0x00, 0x53, 0x02],
        [0x00, 0x00, 0x5e, 0x00, 0x53, 0x03],
    ];
    let ports = [1i64, 1, 11];
    let status = [3i64, 3, 4];
    let mut rows = MacTableRows::default();
    for i in 0..3usize {
        let idx = [i as u32 + 1];
        rows.address.push((oid(FDB_ADDRESS, &idx), SnmpValue::Octets(macs[i].to_vec())));
        rows.port.push(int(FDB_PORT, &idx, ports[i]));
        rows.status.push(int(FDB_STATUS, &idx, status[i]));
    }
    rows
}

/// Rows in the shape the in-process agent serves.
pub fn agent_rows(groups: &[&[Row]]) -> Vec<(Vec<u32>, Value)> {
    groups
        .iter()
        .flat_map(|g| g.iter())
        .map(|(oid, value)| {
            let v = match value {
                SnmpValue::Integer(n) => Value::Integer(*n as i32),
                SnmpValue::Octets(b) => Value::OctetString(b.clone().into()),
            };
            (oid.clone(), v)
        })
        .collect()
}

/// All the lab FortiSwitch tables, ready for `agent_serving`.
pub fn lab_agent_rows() -> Vec<(Vec<u32>, Value)> {
    let p = lab_port_rows();
    let f = lab_fdb();
    agent_rows(&[&p.if_name, &p.if_oper_status, &p.if_high_speed, &p.duplex, &p.peth_admin, &p.peth_detection, &f.address, &f.port, &f.status])
}
