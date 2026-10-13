//! The SNMP protocol side: GET a scalar and WALK a subtree over SNMPv2c, with
//! a hard deadline on every call so a silent or slow switch can't hold the
//! caller (the lookup runs inside an HTTP request).

use std::time::Duration;

use anyhow::{anyhow, Result};

use super::{Row, SnmpValue};

/// Where and how to ask. The community is a secret: never logged or put in
/// an error message.
#[derive(Clone)]
pub struct SnmpTarget {
    pub host: String,
    pub port: u16,
    pub community: String,
    /// Per-request timeout (and the unit of the whole-call deadline).
    pub timeout: Duration,
}

impl std::fmt::Debug for SnmpTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnmpTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("community", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// One connection to one switch.
pub struct Session {
    client: async_snmp::Client,
    host: String,
    port: u16,
    /// Upper bound for a whole get/walk, however many round trips it takes.
    deadline: Duration,
}

/// A whole call may take this many request timeouts (a walk is many round trips).
const DEADLINE_IN_TIMEOUTS: u32 = 10;

fn convert(value: &async_snmp::Value) -> Option<SnmpValue> {
    use async_snmp::Value;
    match value {
        Value::Integer(n) => Some(SnmpValue::Integer(i64::from(*n))),
        Value::Counter32(n) | Value::Gauge32(n) | Value::TimeTicks(n) => Some(SnmpValue::Integer(i64::from(*n))),
        Value::OctetString(bytes) => Some(SnmpValue::Octets(bytes.to_vec())),
        // noSuchObject / endOfMibView / null / types the mappers never use
        _ => None,
    }
}

impl Session {
    /// Open a UDP "connection" (nothing is sent until the first request).
    pub async fn connect(target: &SnmpTarget) -> Result<Session> {
        let client = async_snmp::Client::builder((target.host.as_str(), target.port), async_snmp::Auth::v2c(target.community.as_str()))
            .request_timeout(target.timeout)
            .retry(async_snmp::Retry::none())
            .connect()
            .await
            .map_err(|e| anyhow!("cannot open SNMP session to {}:{}: {}", target.host, target.port, e))?;
        Ok(Session {
            client,
            host: target.host.clone(),
            port: target.port,
            deadline: target.timeout * DEADLINE_IN_TIMEOUTS,
        })
    }

    /// The value of one scalar (`None` for noSuchObject/noSuchInstance).
    #[cfg(test)]
    pub async fn get(&self, oid: &[u32]) -> Result<Option<SnmpValue>> {
        let oid = async_snmp::Oid::from_slice(oid);
        let response = tokio::time::timeout(self.deadline, self.client.get(&oid))
            .await
            .map_err(|_| anyhow!("SNMP get from {}:{} timed out", self.host, self.port))?
            .map_err(|e| anyhow!("SNMP get from {}:{} failed: {}", self.host, self.port, e))?;
        Ok(response.varbinds.first().and_then(|vb| convert(&vb.value)))
    }

    /// Every row under `oid`, in order, as numbers + value.
    pub async fn walk(&self, oid: &[u32]) -> Result<Vec<Row>> {
        let root = async_snmp::Oid::from_slice(oid);
        let rows = tokio::time::timeout(self.deadline, async {
            self.client.walk(root)?.collect().await
        })
        .await
        .map_err(|_| anyhow!("SNMP walk of {} on {}:{} timed out", fmt_oid(oid), self.host, self.port))?
        .map_err(|e| anyhow!("SNMP walk of {} on {}:{} failed: {}", fmt_oid(oid), self.host, self.port, e))?;

        Ok(rows
            .into_iter()
            .filter_map(|vb| convert(&vb.value).map(|v| (vb.oid.arcs().to_vec(), v)))
            .collect())
    }
}

fn fmt_oid(oid: &[u32]) -> String {
    oid.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snmp::testing::agent_serving;
    use async_snmp::Value;
    use std::time::Instant;

    fn target(addr: std::net::SocketAddr, community: &str, timeout_ms: u64) -> SnmpTarget {
        SnmpTarget {
            host: addr.ip().to_string(),
            port: addr.port(),
            community: community.to_string(),
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    #[tokio::test]
    async fn get_returns_a_scalar_and_none_for_a_missing_object() {
        let (addr, _agent) = agent_serving(vec![(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], Value::OctetString("sw1".into()))]).await;
        let session = Session::connect(&target(addr, "public", 500)).await.unwrap();

        assert_eq!(
            session.get(&[1, 3, 6, 1, 2, 1, 1, 5, 0]).await.unwrap(),
            Some(SnmpValue::Octets(b"sw1".to_vec()))
        );
        assert_eq!(session.get(&[1, 3, 6, 1, 2, 1, 99, 0]).await.unwrap(), None);
    }

    #[tokio::test]
    async fn walk_returns_integers_text_and_binary_values_in_order() {
        let (addr, _agent) = agent_serving(vec![
            (vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 8, 1], Value::Integer(1)),
            (vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 8, 2], Value::Integer(2)),
            (vec![1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 1, 1], Value::OctetString(vec![0x00, 0x00, 0x5e, 0x00, 0x53, 0x01].into())),
            (vec![1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 1, 1], Value::OctetString("port1".into())),
        ])
        .await;
        let session = Session::connect(&target(addr, "public", 500)).await.unwrap();

        let if_status = session.walk(&[1, 3, 6, 1, 2, 1, 2, 2, 1, 8]).await.unwrap();
        assert_eq!(
            if_status,
            vec![
                (vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 8, 1], SnmpValue::Integer(1)),
                (vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 8, 2], SnmpValue::Integer(2)),
            ],
            "a walk stays inside the asked subtree"
        );
        let fdb = session.walk(&[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 1]).await.unwrap();
        assert_eq!(fdb[0].1, SnmpValue::Octets(vec![0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]));
        let names = session.walk(&[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 1]).await.unwrap();
        assert_eq!(names[0].1, SnmpValue::Octets(b"port1".to_vec()));
    }

    #[tokio::test]
    async fn a_walk_of_two_thousand_rows_finishes_well_under_two_seconds() {
        let rows: Vec<(Vec<u32>, Value)> = (1..=2000u32)
            .map(|i| (vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 8, i], Value::Integer(1)))
            .collect();
        let (addr, _agent) = agent_serving(rows).await;
        let session = Session::connect(&target(addr, "public", 1000)).await.unwrap();

        let started = Instant::now();
        let walked = session.walk(&[1, 3, 6, 1, 2, 1, 2, 2, 1, 8]).await.unwrap();

        assert_eq!(walked.len(), 2000);
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn a_silent_switch_times_out_without_hanging_and_the_next_call_still_works() {
        // A UDP socket that never answers.
        let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap();
        let session = Session::connect(&target(silent_addr, "public", 200)).await.unwrap();

        let started = Instant::now();
        let err = session.get(&[1, 3, 6, 1, 2, 1, 1, 5, 0]).await.unwrap_err().to_string();
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert!(err.contains("timed out") || err.contains("timeout"), "unhelpful error: {err}");
        assert!(session.walk(&[1, 3, 6, 1, 2, 1, 2]).await.is_err());

        // The same process can talk to a healthy switch right afterwards.
        let (addr, _agent) = agent_serving(vec![(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], Value::OctetString("ok".into()))]).await;
        let healthy = Session::connect(&target(addr, "public", 500)).await.unwrap();
        assert_eq!(healthy.get(&[1, 3, 6, 1, 2, 1, 1, 5, 0]).await.unwrap(), Some(SnmpValue::Octets(b"ok".to_vec())));
        drop(silent);
    }

    #[tokio::test]
    async fn a_wrong_community_is_an_error_that_does_not_echo_the_community() {
        let (addr, _agent) = agent_serving(vec![(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], Value::OctetString("x".into()))]).await;
        let session = Session::connect(&target(addr, "wrong-community-secret", 200)).await.unwrap();

        let err = format!("{:#}", session.get(&[1, 3, 6, 1, 2, 1, 1, 5, 0]).await.unwrap_err());
        assert!(!err.contains("wrong-community-secret"), "secret echoed: {err}");
    }

    #[tokio::test]
    async fn garbage_datagrams_reach_the_agent_and_the_client_without_panicking() {
        let (addr, _agent) = agent_serving(vec![(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], Value::OctetString("alive".into()))]).await;
        let junk = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for payload in [&b""[..], &b"\x00"[..], &b"\x30\xff\xff\xff"[..], &[0xffu8; 300][..]] {
            junk.send_to(payload, addr).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        let session = Session::connect(&target(addr, "public", 500)).await.unwrap();
        assert_eq!(session.get(&[1, 3, 6, 1, 2, 1, 1, 5, 0]).await.unwrap(), Some(SnmpValue::Octets(b"alive".to_vec())));
    }

    #[test]
    fn the_target_debug_output_hides_the_community() {
        let t = SnmpTarget { host: "h".to_string(), port: 161, community: "very-secret".to_string(), timeout: Duration::from_secs(1) };
        assert!(!format!("{t:?}").contains("very-secret"));
    }
}
