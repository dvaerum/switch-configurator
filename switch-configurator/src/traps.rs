//! SNMP traps from the switches, received by the service itself.
//!
//! What a FortiSwitch 108F-POE sends (docs/learnings/snmp-fortiswitch.md): the
//! standard `linkDown` / `linkUp` traps about a second after a link changes, and a
//! config-change trap on every change. It sends **no** PoE trap, so PoE state stays
//! polled; a link trap only makes the link state fresh at once and is counted, as a
//! raw fact (how often a port went down or up), never judged.
//!
//! The sender is recognised by its source address (the switch's `management_ip`).
//! SNMP does not authenticate that on v1/v2c, which is acceptable on the closed
//! management network this service runs on.

use std::net::IpAddr;

use async_snmp::{Notification, NotificationReceiver, Oid, Value, VarBind};
use tracing::{debug, info, warn};

use crate::config::{ConfigStore, SseEvent};

/// IF-MIB `linkDown` and `linkUp`.
const LINK_DOWN: &[u32] = &[1, 3, 6, 1, 6, 3, 1, 1, 5, 3];
const LINK_UP: &[u32] = &[1, 3, 6, 1, 6, 3, 1, 1, 5, 4];
/// IF-MIB `ifIndex` and `ifDescr` columns (the row index is the trap's interface).
const IF_INDEX: &[u32] = &[1, 3, 6, 1, 2, 1, 2, 2, 1, 1];
const IF_DESCR: &[u32] = &[1, 3, 6, 1, 2, 1, 2, 2, 1, 2];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Up,
    Down,
}

impl LinkState {
    fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// A link change read from a trap.
#[derive(Debug, PartialEq, Eq)]
pub struct LinkTrap {
    pub state: LinkState,
    /// The interface name from `ifDescr` (`port9`), or its `ifIndex` when the trap has no name.
    pub port: String,
}

/// `Some` for a `linkDown` / `linkUp` trap, `None` for any other trap.
pub fn link_trap(trap_oid: &Oid, varbinds: &[VarBind]) -> Option<LinkTrap> {
    let state = match trap_oid.arcs() {
        LINK_DOWN => LinkState::Down,
        LINK_UP => LinkState::Up,
        _ => return None,
    };
    let in_column = |column: &[u32]| varbinds.iter().find(|vb| vb.oid.arcs().starts_with(column));
    let name = in_column(IF_DESCR).and_then(|vb| match &vb.value {
        Value::OctetString(bytes) => Some(String::from_utf8_lossy(bytes).trim().to_string()).filter(|n| !n.is_empty()),
        _ => None,
    });
    let index = in_column(IF_INDEX).and_then(|vb| match &vb.value {
        Value::Integer(n) => Some(n.to_string()),
        _ => None,
    });
    Some(LinkTrap { state, port: name.or(index)? })
}

/// Counts and publishes a link trap from `source`, and refreshes that switch's
/// port status. A trap from an address that is no configured switch is dropped.
pub async fn handle_link_trap(store: &ConfigStore, source: IpAddr, trap_oid: &Oid, varbinds: &[VarBind]) {
    let Some(trap) = link_trap(trap_oid, varbinds) else {
        return;
    };
    let switch_config = {
        let config = store.config.read().await;
        config.switches.iter().find(|s| s.management_ip().parse::<IpAddr>().is_ok_and(|ip| ip == source)).cloned()
    };
    let Some(switch_config) = switch_config else {
        debug!("link trap from {} ignored: not the management address of a configured switch", source);
        return;
    };
    info!("link {} on {} port {} (trap from {})", trap.state.name(), switch_config.id, trap.port, source);

    store.record_link_trap(&switch_config.id, &trap.port, trap.state.name());
    store.emit_event(SseEvent::LinkChange {
        switch_id: switch_config.id.clone(),
        port_id: trap.port.clone(),
        state: trap.state.name().to_string(),
        source: "trap".to_string(),
    });
    // The poll would catch this within 10 s; a trap makes the link state current at once.
    crate::api::handlers::refresh_snmp_port_status_of(store, &switch_config).await;
}

/// Receives traps until the socket fails.
pub async fn serve(store: ConfigStore, receiver: NotificationReceiver) {
    loop {
        let received = match receiver.recv().await {
            Ok(received) => received,
            Err(e) => {
                warn!("trap receiver: {}", e);
                continue;
            }
        };
        let (trap_oid, varbinds) = match &received.notification {
            Notification::TrapV2c { trap_oid, varbinds, .. }
            | Notification::TrapV3 { trap_oid, varbinds, .. }
            | Notification::InformV2c { trap_oid, varbinds, .. }
            | Notification::InformV3 { trap_oid, varbinds, .. } => (trap_oid, varbinds),
            Notification::TrapV1 { .. } => {
                debug!("SNMPv1 trap from {} ignored", received.source);
                continue;
            }
        };
        handle_link_trap(&store, received.source.ip(), trap_oid, varbinds).await;
    }
}

/// Binds `listen` (`address:port`, e.g. `0.0.0.0:1162`) and serves traps on it.
pub async fn run(store: ConfigStore, listen: String) -> anyhow::Result<()> {
    let receiver = NotificationReceiver::bind(&listen).await?;
    info!("receiving SNMP traps on {}", receiver.local_addr());
    serve(store, receiver).await;
    Ok(())
}
