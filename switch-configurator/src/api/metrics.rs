//! `GET /metrics`: the cached per-port facts in Prometheus text format.
//!
//! Served from the port-status cache that the background refreshers keep warm,
//! so a scrape never waits for (or loads) a switch. Raw facts only: no
//! "healthy" verdicts, and a switch with no cached data has no series at all
//! rather than zeros. `switch_port_status_age_seconds` says how old the data is.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use prometheus_client::encoding::prometheus_text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

use crate::config::ConfigStore;

type Labels = Vec<(String, String)>;

/// The `poe_status` values the mapping can produce; each port reports all of
/// them (1 for the current one) so queries need no knowledge of which exist.
const POE_STATES: [&str; 6] = ["disabled", "searching", "delivering", "fault", "test", "unknown"];

fn labels(pairs: &[(&str, &str)]) -> Labels {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub async fn get_metrics(State(store): State<ConfigStore>) -> Response {
    let age = Family::<Labels, Gauge<f64, std::sync::atomic::AtomicU64>>::default();
    let link_up = Family::<Labels, Gauge>::default();
    let poe_admin = Family::<Labels, Gauge>::default();
    let poe_state = Family::<Labels, Gauge>::default();
    let poe_class = Family::<Labels, Gauge>::default();
    let counters: [Family<Labels, Counter>; 5] = Default::default();
    let unit_gauges: [Family<Labels, Gauge>; 3] = Default::default();
    let rewalks = Family::<Labels, Counter>::default();
    let link_traps = Family::<Labels, Counter>::default();

    for (switch, cached) in store.poe_unit_cache.read().await.iter() {
        let at = labels(&[("switch", switch)]);
        let values = [cached.unit.budget_watts, cached.unit.consumption_watts, cached.unit.threshold_percent];
        for (family, value) in unit_gauges.iter().zip(values) {
            if let Some(value) = value {
                family.get_or_create(&at).set(i64::from(value));
            }
        }
    }
    for switch in store.config.read().await.switches.iter() {
        if let Some(target) = crate::snmp::lookup::target_for(switch) {
            rewalks.get_or_create(&labels(&[("switch", &switch.id)])).inc_by(crate::snmp::lookup::rewalks_for(&target));
        }
    }

    for ((switch, _), counts) in store.link_traps.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter() {
        for (state, count) in [("down", counts.down), ("up", counts.up)] {
            link_traps
                .get_or_create(&labels(&[("switch", switch), ("port", &counts.port_id), ("state", state)]))
                .inc_by(count);
        }
    }

    let now = chrono::Utc::now();
    for (switch, cached) in store.port_status_cache.read().await.iter() {
        let seconds = (now - cached.fetched_at).num_milliseconds().max(0) as f64 / 1000.0;
        age.get_or_create(&labels(&[("switch", switch)])).set(seconds);
        for entry in &cached.entries {
            let port = entry.port_id.as_str();
            if let Some(link) = &entry.link_status {
                link_up.get_or_create(&labels(&[("switch", switch), ("port", port)])).set((link == "up") as i64);
            }
            if let Some(admin) = &entry.poe_admin {
                poe_admin
                    .get_or_create(&labels(&[("switch", switch), ("port", port)]))
                    .set((admin == "enabled") as i64);
            }
            if let Some(class) = entry.poe_class {
                poe_class.get_or_create(&labels(&[("switch", switch), ("port", port)])).set(class as i64);
            }
            if let Some(c) = &entry.poe_counters {
                let values = [c.mps_absent, c.invalid_signature, c.power_denied, c.overload, c.short];
                for (family, value) in counters.iter().zip(values) {
                    family.get_or_create(&labels(&[("switch", switch), ("port", port)])).inc_by(value);
                }
            }
            if let Some(state) = &entry.poe_status {
                for known in POE_STATES {
                    poe_state
                        .get_or_create(&labels(&[("switch", switch), ("port", port), ("state", known)]))
                        .set((state == known) as i64);
                }
            }
        }
    }

    let mut registry = Registry::default();
    registry.register("switch_port_status_age_seconds", "Seconds since the port status of the switch was read", age);
    registry.register("switch_port_link_up", "1 if the port has link", link_up);
    registry.register("switch_port_poe_admin_enabled", "1 if PoE is enabled on the port (the setting)", poe_admin);
    registry.register(
        "switch_port_poe_state",
        "PoE power state of the port as a state set: 1 for the current state, 0 for the others",
        poe_state,
    );

    registry.register(
        "switch_port_poe_class",
        "IEEE class (0-4) of the device last detected on the port; meaningful only while one is detected",
        poe_class,
    );
    let counter_help = [
        ("switch_port_poe_mps_absent", "Times the device on the port stopped drawing the minimum current"),
        ("switch_port_poe_invalid_signature", "Invalid PoE signatures detected on the port"),
        ("switch_port_poe_power_denied", "Times power was denied to the port"),
        ("switch_port_poe_overload", "Overload conditions on the port"),
        ("switch_port_poe_short", "Short circuits detected on the port"),
    ];
    for ((name, help), family) in counter_help.into_iter().zip(counters) {
        registry.register(name, help, family);
    }

    for ((name, help), family) in [
        ("switch_poe_budget_watts", "Power the switch's PoE unit can supply"),
        ("switch_poe_consumption_watts", "Power the switch's PoE unit supplies now"),
        ("switch_poe_threshold_percent", "Consumption, as percent of the budget, at which the switch alarms"),
    ]
    .into_iter()
    .zip(unit_gauges)
    {
        registry.register(name, help, family);
    }
    registry.register(
        "switch_snmp_rewalks",
        "Extra SNMP walks needed because a result was short, since the service started",
        rewalks,
    );

    registry.register(
        "switch_port_link_traps",
        "Link down and up traps received from the switch for the port, since the service started",
        link_traps,
    );

    let mut body = String::new();
    match encode(&mut body, &registry) {
        Ok(()) => (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("could not encode metrics: {}", e)).into_response(),
    }
}
