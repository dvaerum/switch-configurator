//! `POST /switches/{id}/poe`: turn PoE on, off or power-cycle several ports in
//! one request, on one console session and one turn of the switch's queue.
//!
//! Every port gets its own events and its own outcome, and a port that fails
//! does not stop the others. A port is `completed` only once the PoE setting
//! has been read back over SNMP as asked (see `confirm_settings`); power state
//! never decides that, because a device that is off or unplugged is normal.
//! Design: `docs/decisions/0003-poe-batch-control-and-telemetry.md`.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, warn};

use super::handlers::{poe_commands_for, QUEUE_MAX_WAIT_ASYNC};
use crate::config::{ConfigStore, PoeBatchFailure, SseEvent};
use crate::models::{SwitchConfig, Vendor};
use crate::vendors::traits::COMMAND_REFUSED;

/// Bounds how long one request can hold the switch's turn.
pub const MAX_PORTS_PER_REQUEST: usize = 64;

/// Seconds a reset leaves the ports unpowered, as in the single-port reset.
const RESET_WAIT_SECONDS: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoeAction {
    On,
    Off,
    Reset,
}

impl PoeAction {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "reset" => Some(Self::Reset),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Reset => "reset",
        }
    }

    /// The PoE setting the port must have afterwards.
    fn enabled_afterwards(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Debug, Deserialize)]
pub struct PoeBatchRequest {
    pub action: String,
    pub ports: Vec<String>,
}

/// How one port of a batch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortOutcome {
    pub port: String,
    /// `completed` or `failed`.
    pub status: String,
    /// The reason for a failure, or the note that a completed port was not confirmed.
    pub detail: Option<String>,
}

/// How long the SNMP read-back keeps asking before it gives up.
#[derive(Debug, Clone, Copy)]
pub struct ConfirmPlan {
    pub attempts: u32,
    pub pause: Duration,
}

impl Default for ConfirmPlan {
    fn default() -> Self {
        Self { attempts: 10, pause: Duration::from_secs(1) }
    }
}

/// Trims, lower-cases, drops a `port` prefix and duplicates (order kept).
/// An empty list, a blank entry or more than `MAX_PORTS_PER_REQUEST` ports is a mistake.
pub fn normalize_ports(raw: &[String]) -> Result<Vec<String>, String> {
    let mut ports: Vec<String> = Vec::new();
    for entry in raw {
        let name = entry.trim().to_lowercase();
        if name.is_empty() {
            return Err("the ports list has a blank entry".to_string());
        }
        let name = name.strip_prefix("port").unwrap_or(&name).to_string();
        if !ports.contains(&name) {
            ports.push(name);
        }
    }
    if ports.is_empty() {
        return Err("the ports list is empty".to_string());
    }
    if ports.len() > MAX_PORTS_PER_REQUEST {
        return Err(format!("at most {} ports per request, got {}", MAX_PORTS_PER_REQUEST, ports.len()));
    }
    Ok(ports)
}

fn bad_request(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
}

/// POST /switches/{id}/poe with `{"action": "on" | "off" | "reset", "ports": ["1", "2"]}`.
///
/// Everything is checked before anything runs; the work then waits for the
/// switch's turn in the background and reports on `/api/events`.
pub async fn poe_batch(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
    Json(request): Json<PoeBatchRequest>,
) -> Response {
    let Some(action) = PoeAction::parse(&request.action) else {
        return bad_request(format!("action must be on, off or reset, got '{}'", request.action));
    };
    let ports = match normalize_ports(&request.ports) {
        Ok(ports) => ports,
        Err(message) => return bad_request(message),
    };
    let switch_config = {
        let config = store.config.read().await;
        match config.switches.iter().find(|s| s.id == id) {
            Some(cfg) => cfg.clone(),
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": format!("Switch '{}' not found", id)})))
                    .into_response();
            }
        }
    };
    let model = switch_config.model();
    if !model.supports_poe() {
        return bad_request(format!("Switch model {:?} does not support PoE", model));
    }
    if !matches!(model.vendor(), Vendor::Aruba | Vendor::Fortiswitch) {
        return bad_request(format!("PoE control not yet supported for {:?} switches", model.vendor()));
    }
    let without_poe: Vec<&str> = ports.iter().filter(|p| !model.port_supports_poe(p)).map(String::as_str).collect();
    if !without_poe.is_empty() {
        return bad_request(format!("Ports without PoE on {:?}: {}", model, without_poe.join(", ")));
    }

    let (store_bg, id_bg, ports_bg) = (store.clone(), id.clone(), ports.clone());
    tokio::spawn(async move {
        // Taken here, not in the handler, so the request returns 202 at once
        // and the batch waits in the switch's queue like any other operation.
        let guard = match store_bg.status.acquire_configuring_queued(id_bg.clone(), QUEUE_MAX_WAIT_ASYNC).await {
            Some(guard) => guard,
            None => {
                let message = format!("Gave up after waiting {}s for the switch's turn", QUEUE_MAX_WAIT_ASYNC.as_secs());
                let outcomes: Vec<PortOutcome> = ports_bg
                    .iter()
                    .map(|p| PortOutcome { port: p.clone(), status: "failed".into(), detail: Some(message.clone()) })
                    .collect();
                finish(&store_bg, &id_bg, action, &outcomes).await;
                return;
            }
        };
        let outcomes = run_batch(&store_bg, &id_bg, &switch_config, action, &ports_bg, ConfirmPlan::default()).await;
        guard.release().await;
        info!("PoE {} batch on '{}' finished: {} ports", action.name(), id_bg, outcomes.len());
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": format!("PoE {} accepted for {} ports on switch '{}' (runs when the switch is free)", action.name(), ports.len(), id),
            "switch_id": id,
            "action": action.name(),
            "ports": ports,
            "hint": "Listen on /api/events for poe-set / poe-reset events per port and a poe-batch summary; GET /switches/{id}/poe shows each port's last_operation"
        })),
    )
        .into_response()
}

fn emit_port(store: &ConfigStore, id: &str, action: PoeAction, port: &str, stage: &str, detail: Option<String>) {
    let event = match action {
        PoeAction::Reset => SseEvent::PoeReset {
            switch_id: id.to_string(),
            port_id: port.to_string(),
            stage: stage.to_string(),
            detail,
            status: None,
        },
        _ => SseEvent::PoeSet {
            switch_id: id.to_string(),
            port_id: port.to_string(),
            action: action.name().to_string(),
            stage: stage.to_string(),
            detail,
            status: None,
        },
    };
    store.emit_event(event);
}

/// The switch's or the connection's own words, without the error type's prefix.
fn reason(error: &crate::vendors::traits::VendorError) -> String {
    match error {
        crate::vendors::traits::VendorError::CommandError(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Sends every port's terminal event and the batch summary, and records the failures.
async fn finish(store: &ConfigStore, id: &str, action: PoeAction, outcomes: &[PortOutcome]) {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    for outcome in outcomes {
        if outcome.status == "completed" {
            emit_port(store, id, action, &outcome.port, "completed", outcome.detail.clone());
            completed.push(outcome.port.clone());
        } else {
            let detail = outcome.detail.clone().unwrap_or_default();
            emit_port(store, id, action, &outcome.port, "failed", Some(detail.clone()));
            error!("PoE {} failed for {}:{}: {}", action.name(), id, outcome.port, detail);
            store
                .status
                .record_error(
                    "PoeBatch".to_string(),
                    format!("port {}: {}", outcome.port, detail),
                    Some(id.to_string()),
                    format!("poe_{}", action.name()),
                )
                .await;
            failed.push(PoeBatchFailure { port_id: outcome.port.clone(), detail });
        }
    }
    store.emit_event(SseEvent::PoeBatch {
        switch_id: id.to_string(),
        action: action.name().to_string(),
        status: if failed.is_empty() { "completed" } else { "failed" }.to_string(),
        completed,
        failed,
    });
}

/// Runs a validated batch on one pooled session and returns one outcome per
/// port, in the order given. Never returns early: a port that cannot be done
/// is reported, not skipped.
pub async fn run_batch(
    store: &ConfigStore,
    id: &str,
    switch_config: &SwitchConfig,
    action: PoeAction,
    ports: &[String],
    confirm: ConfirmPlan,
) -> Vec<PortOutcome> {
    for port in ports {
        emit_port(store, id, action, port, "connecting", None);
    }
    let mut failure: Vec<Option<String>> = vec![None; ports.len()];

    match store.session_pool.checkout(switch_config).await {
        Err(e) => {
            let message = format!("Connection failed: {}", e);
            failure.iter_mut().for_each(|f| *f = Some(message.clone()));
        }
        Ok(mut session) => {
            let lost = drive_console(store, id, switch_config, action, ports, &mut failure, &mut session).await;
            if lost {
                session.discard().await;
            } else {
                session.release().await;
            }
        }
    }

    // Read the setting back for the ports the switch accepted.
    let accepted: Vec<usize> = (0..ports.len()).filter(|&i| failure[i].is_none()).collect();
    let mut notes: HashMap<usize, String> = HashMap::new();
    if !accepted.is_empty() {
        for &i in &accepted {
            emit_port(store, id, action, &ports[i], "verifying", None);
        }
        let wanted: Vec<&str> = accepted.iter().map(|&i| ports[i].as_str()).collect();
        let verdicts = confirm_settings(store, id, switch_config, &wanted, action.enabled_afterwards(), confirm).await;
        for &i in &accepted {
            match verdicts.get(ports[i].as_str()) {
                Some(Verdict::Confirmed) => {}
                Some(Verdict::Unconfirmed(note)) => {
                    notes.insert(i, note.clone());
                }
                Some(Verdict::Wrong(reason)) => failure[i] = Some(reason.clone()),
                None => {}
            }
        }
    }

    let outcomes: Vec<PortOutcome> = ports
        .iter()
        .enumerate()
        .map(|(i, port)| match &failure[i] {
            Some(reason) => PortOutcome { port: port.clone(), status: "failed".into(), detail: Some(reason.clone()) },
            None => PortOutcome { port: port.clone(), status: "completed".into(), detail: notes.get(&i).cloned() },
        })
        .collect();
    finish(store, id, action, &outcomes).await;
    outcomes
}

/// The console part: every port's commands, one list per port, so a refusal
/// is attributed to the right port. Returns true when the session is lost.
async fn drive_console(
    store: &ConfigStore,
    id: &str,
    switch_config: &SwitchConfig,
    action: PoeAction,
    ports: &[String],
    failure: &mut [Option<String>],
    session: &mut crate::session_pool::Session,
) -> bool {
    let reset = action == PoeAction::Reset;
    let first_stage = if reset { "disabling" } else { "setting" };
    let mut lost: Option<String> = None;

    for (i, port) in ports.iter().enumerate() {
        if let Some(reason) = &lost {
            failure[i] = Some(reason.clone());
            continue;
        }
        emit_port(store, id, action, port, first_stage, None);
        let (disable, enable) = poe_commands_for(switch_config, port);
        let commands = if action == PoeAction::On { &enable } else { &disable };
        if let Err(e) = session.vendor().execute_raw_commands(commands).await {
            let text = reason(&e);
            failure[i] = Some(text.clone());
            if !text.contains(COMMAND_REFUSED) {
                lost = Some(text);
            } else if reset {
                // The disable may have taken effect before it was refused: fail
                // towards power. Only logged: the refusal is what the caller needs.
                if let Err(e2) = session.vendor().execute_raw_commands(&enable).await {
                    warn!("PoE re-enable after a refused disable on port {} also failed: {}", port, e2);
                }
            }
        }
    }

    if !reset {
        return lost.is_some();
    }
    let disabled: Vec<usize> = (0..ports.len()).filter(|&i| failure[i].is_none()).collect();
    if let Some(reason) = &lost {
        for &i in &disabled {
            failure[i] = Some(format!(
                "PoE was DISABLED on port {} but the session was lost before re-enable: {}. Manual intervention required.",
                ports[i], reason
            ));
        }
        return true;
    }

    // One wait for all ports, with a per-port tick so a dashboard can count down.
    for n in (1..=RESET_WAIT_SECONDS).rev() {
        for &i in &disabled {
            emit_port(store, id, action, &ports[i], "waiting", Some(n.to_string()));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    for &i in &disabled {
        if let Some(reason) = &lost {
            failure[i] = Some(format!(
                "PoE was DISABLED on port {} but the session was lost before re-enable: {}. Manual intervention required.",
                ports[i], reason
            ));
            continue;
        }
        emit_port(store, id, action, &ports[i], "enabling", None);
        let (_, enable) = poe_commands_for(switch_config, &ports[i]);
        if let Err(e) = session.vendor().execute_raw_commands(&enable).await {
            warn!("First PoE re-enable attempt failed for port {}: {}. Retrying...", ports[i], e);
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Err(e2) = session.vendor().execute_raw_commands(&enable).await {
                let text = reason(&e2);
                failure[i] = Some(format!(
                    "PoE was DISABLED on port {} but re-enable FAILED after retry: {}. Manual intervention required.",
                    ports[i], text
                ));
                if !text.contains(COMMAND_REFUSED) {
                    lost = Some(text);
                }
            }
        }
    }
    lost.is_some()
}

/// What the read-back said about one port.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The setting is as asked.
    Confirmed,
    /// Nothing to check it with; the note says why.
    Unconfirmed(String),
    /// The switch accepted the command but the setting is not as asked.
    Wrong(String),
}

/// Reads the PoE setting back over SNMP (through the per-switch gate) until
/// every port shows what was asked or the attempts run out.
async fn confirm_settings(
    store: &ConfigStore,
    id: &str,
    switch_config: &SwitchConfig,
    ports: &[&str],
    enabled: bool,
    plan: ConfirmPlan,
) -> HashMap<String, Verdict> {
    let mut verdicts: HashMap<String, Verdict> = HashMap::new();
    let Some(target) = crate::snmp::lookup::target_for(switch_config) else {
        for port in ports {
            verdicts.insert(
                port.to_string(),
                Verdict::Unconfirmed("not confirmed: SNMP reads are not enabled for this switch".to_string()),
            );
        }
        return verdicts;
    };

    let model = switch_config.model();
    let poe = |name: &str| model.port_supports_poe(name.strip_prefix("port").unwrap_or(name));
    let wanted = if enabled { "enabled" } else { "disabled" };
    let mut pending: Vec<&str> = ports.to_vec();
    let mut last_seen: HashMap<String, Option<String>> = HashMap::new();
    let mut answered = false;
    let mut last_error = String::new();

    for attempt in 0..plan.attempts.max(1) {
        if attempt > 0 {
            tokio::time::sleep(plan.pause).await;
        }
        match crate::snmp::lookup::fetch_port_status(&target, poe).await {
            Ok((entries, raw)) => {
                answered = true;
                for port in &pending {
                    let entry = entries.iter().find(|e| e.port_id.strip_prefix("port") == Some(*port));
                    last_seen.insert(port.to_string(), entry.and_then(|e| e.poe_admin.clone()));
                }
                pending.retain(|port| {
                    if last_seen.get(*port).and_then(|v| v.as_deref()) == Some(wanted) {
                        verdicts.insert(port.to_string(), Verdict::Confirmed);
                        false
                    } else {
                        true
                    }
                });
                store.port_status_cache.write().await.insert(
                    id.to_string(),
                    crate::config::CachedPortStatus {
                        entries,
                        raw_output: raw,
                        fetched_at: chrono::Utc::now(),
                        generation_id: uuid::Uuid::new_v4(),
                    },
                );
            }
            Err(e) => last_error = format!("{:#}", e),
        }
        if pending.is_empty() {
            break;
        }
    }

    for port in pending {
        let verdict = match last_seen.get(port).cloned().flatten() {
            Some(seen) => Verdict::Wrong(format!(
                "the switch accepted the command but PoE is still {} on port {}",
                seen, port
            )),
            None if answered => {
                Verdict::Unconfirmed("not confirmed: the switch does not report the PoE setting of this port".to_string())
            }
            None => Verdict::Unconfirmed(format!("not confirmed: SNMP did not answer ({})", last_error)),
        };
        verdicts.insert(port.to_string(), verdict);
    }
    verdicts
}
