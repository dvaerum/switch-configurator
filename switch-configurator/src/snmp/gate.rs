//! One SNMP table walk at a time per switch, shared by everyone who asks.
//!
//! A FortiSwitch's SNMP agent silently ends a walk early when other walks run
//! at the same time: it answers a normal end-of-table, so nothing in the
//! reply says rows are missing (measured on a bench 124E: one walk always
//! returns all 600 MAC entries, six at once return 147-229, from plain
//! `snmpbulkwalk` too). So lookups must never overlap on one switch, and a
//! caller that arrives while a walk is running shares that walk instead of
//! starting another. There is no cache: the shared result is dropped the
//! moment the walk completes, so the next caller walks again.
//!
//! The agent can still be disturbed by something that is not this service. A
//! result that is far smaller than the previous one is therefore walked again
//! (bounded) and the largest result wins, since a cut-short walk only ever
//! loses rows.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::sync::watch;
use tracing::{debug, warn};

/// A result below this share of the previous one is walked again.
const RETRY_BELOW: f64 = 0.7;
/// Smaller previous tables are too small to judge by.
const MIN_PREVIOUS_FOR_RETRY: usize = 20;
/// Walks per lookup, the first included.
const MAX_ATTEMPTS: u32 = 3;

/// What a gated lookup produced.
#[derive(Debug)]
pub struct Walked<T> {
    pub value: Arc<T>,
    /// Rows in the result (what a short table is judged by).
    pub entries: usize,
    /// Wall time of the walk(s), including any wait for another switch lookup.
    pub elapsed: Duration,
    /// Walks it took (more than 1 only after a suspiciously short result).
    pub attempts: u32,
    /// True when this caller arrived mid-walk and shares another caller's walk.
    pub shared: bool,
}

type Slot<T> = Option<std::result::Result<Walked<T>, String>>;

/// One kind of lookup (e.g. the MAC table). Lookups of different kinds on the
/// same switch also never overlap: see `switch_lock`.
pub struct Gate<T> {
    flights: Mutex<HashMap<String, watch::Receiver<Slot<T>>>>,
    last_entries: Mutex<HashMap<String, usize>>,
}

static SWITCH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn switch_lock(switch: &str) -> Arc<tokio::sync::Mutex<()>> {
    SWITCH_LOCKS.lock().unwrap().entry(switch.to_string()).or_default().clone()
}

impl<T: Send + Sync + 'static> Default for Gate<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + Sync + 'static> Gate<T> {
    pub fn new() -> Self {
        Gate { flights: Mutex::new(HashMap::new()), last_entries: Mutex::new(HashMap::new()) }
    }

    /// Run `walk` (returning the value and its row count) for `switch`, or
    /// share the walk already running for it.
    pub async fn run<F, Fut>(&'static self, switch: &str, walk: F) -> Result<Walked<T>>
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(T, usize)>> + Send + 'static,
    {
        let (mut receiver, shared) = {
            let mut flights = self.flights.lock().unwrap();
            if let Some(running) = flights.get(switch) {
                (running.clone(), true)
            } else {
                let (sender, receiver) = watch::channel(None);
                flights.insert(switch.to_string(), receiver.clone());
                let key = switch.to_string();
                // Spawned: a caller that gives up (its HTTP request dropped)
                // must not strand everyone sharing the walk.
                tokio::spawn(async move {
                    let result = self.lead(&key, walk).await;
                    // Out of the map first, so the next caller starts a new walk.
                    self.flights.lock().unwrap().remove(&key);
                    let _ = sender.send(Some(result));
                });
                (receiver, false)
            }
        };

        let slot = receiver
            .wait_for(|slot| slot.is_some())
            .await
            .map_err(|_| anyhow!("the SNMP walk for {} ended without a result", switch))?;
        match slot.as_ref().expect("waited for a result") {
            Ok(walked) => Ok(Walked {
                value: walked.value.clone(),
                entries: walked.entries,
                elapsed: walked.elapsed,
                attempts: walked.attempts,
                shared,
            }),
            Err(message) => Err(anyhow!("{}", message)),
        }
    }

    /// The walk itself: wait for any other lookup on this switch, walk, and
    /// walk again (bounded) while the result looks cut short.
    async fn lead<F, Fut>(&self, switch: &str, walk: F) -> std::result::Result<Walked<T>, String>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<(T, usize)>>,
    {
        let started = Instant::now();
        let lock = switch_lock(switch);
        let _one_at_a_time = lock.lock().await;
        let previous = self.last_entries.lock().unwrap().get(switch).copied();

        let mut best: Option<(T, usize)> = None;
        let mut attempts = 0;
        loop {
            attempts += 1;
            match walk().await {
                Ok((value, entries)) => {
                    if best.as_ref().is_none_or(|(_, n)| entries > *n) {
                        best = Some((value, entries));
                    }
                }
                // The first walk failing is the lookup failing; a retry
                // failing leaves the earlier result standing.
                Err(e) if best.is_none() => return Err(format!("{:#}", e)),
                Err(e) => {
                    warn!("SNMP re-walk for {} failed, keeping the earlier result: {:#}", switch, e);
                    break;
                }
            }
            let entries = best.as_ref().map_or(0, |(_, n)| *n);
            let short = previous.is_some_and(|p| p >= MIN_PREVIOUS_FOR_RETRY && (entries as f64) < RETRY_BELOW * p as f64);
            if !short || attempts >= MAX_ATTEMPTS {
                break;
            }
            warn!(
                "SNMP walk for {} returned {} rows after {} earlier; walking again (attempt {} of {})",
                switch, entries, previous.unwrap_or(0), attempts + 1, MAX_ATTEMPTS
            );
        }

        let (value, entries) = best.expect("a first successful walk, or an early return");
        self.last_entries.lock().unwrap().insert(switch.to_string(), entries);
        let elapsed = started.elapsed();
        debug!("SNMP walk for {}: {} rows in {:?}, {} attempt(s)", switch, entries, elapsed, attempts);
        Ok(Walked { value: Arc::new(value), entries, elapsed, attempts, shared: false })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    fn gate<T: Send + Sync + 'static>() -> &'static Gate<T> {
        Box::leak(Box::new(Gate::new()))
    }

    #[tokio::test]
    async fn callers_that_arrive_mid_walk_share_one_walk() {
        let g = gate::<Vec<u8>>();
        let walks = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..6 {
            let walks = walks.clone();
            tasks.push(tokio::spawn(async move {
                g.run("sw-share", move || {
                    let walks = walks.clone();
                    async move {
                        walks.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Ok((vec![1, 2, 3], 3))
                    }
                })
                .await
                .unwrap()
            }));
        }
        let results: Vec<_> = futures_join(tasks).await;

        assert_eq!(walks.load(Ordering::SeqCst), 1, "six overlapping callers must cost one walk");
        assert!(results.iter().all(|r| *r.value == vec![1, 2, 3]));
        assert_eq!(results.iter().filter(|r| !r.shared).count(), 1, "exactly one caller leads");
    }

    #[tokio::test]
    async fn a_caller_that_arrives_after_the_walk_finished_walks_again() {
        let g = gate::<u32>();
        let walks = Arc::new(AtomicU32::new(0));
        for expected in 1..=2u32 {
            let walks = walks.clone();
            let r = g
                .run("sw-fresh", move || {
                    let walks = walks.clone();
                    async move { Ok((walks.fetch_add(1, Ordering::SeqCst) + 1, 5)) }
                })
                .await
                .unwrap();
            assert_eq!(*r.value, expected, "no cache: every later caller reads live");
            assert!(!r.shared);
        }
    }

    #[tokio::test]
    async fn lookups_of_different_kinds_on_one_switch_never_overlap() {
        let a = gate::<u8>();
        let b = gate::<u16>();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let slow = |active: Arc<AtomicUsize>, peak: Arc<AtomicUsize>| async move {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(120)).await;
            active.fetch_sub(1, Ordering::SeqCst);
        };
        let (a1, p1, a2, p2) = (active.clone(), peak.clone(), active.clone(), peak.clone());
        let t1 = tokio::spawn(async move {
            a.run("sw-kinds", move || {
                let (a, p) = (a1.clone(), p1.clone());
                async move { slow(a, p).await; Ok((1u8, 1)) }
            })
            .await
            .unwrap();
        });
        let t2 = tokio::spawn(async move {
            b.run("sw-kinds", move || {
                let (a, p) = (a2.clone(), p2.clone());
                async move { slow(a, p).await; Ok((2u16, 1)) }
            })
            .await
            .unwrap();
        });
        t1.await.unwrap();
        t2.await.unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 1, "two walks ran on the same switch at the same time");
    }

    #[tokio::test]
    async fn a_suspiciously_short_result_is_walked_again_and_the_larger_one_wins() {
        let g = gate::<Vec<u32>>();
        // First lookup establishes 100 entries.
        let r = g.run("sw-retry", || async { Ok((vec![0; 100], 100)) }).await.unwrap();
        assert_eq!(r.attempts, 1);
        // Second: the agent cuts the first walk short (20), the retry is complete (100).
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let r = g
            .run("sw-retry", move || {
                let c = c.clone();
                async move {
                    let n = if c.fetch_add(1, Ordering::SeqCst) == 0 { 20 } else { 100 };
                    Ok((vec![0; n], n))
                }
            })
            .await
            .unwrap();
        assert_eq!((r.entries, r.attempts), (100, 2));
        assert_eq!(r.value.len(), 100);
    }

    #[tokio::test]
    async fn a_table_that_really_shrank_is_accepted_after_a_bounded_number_of_walks() {
        let g = gate::<usize>();
        g.run("sw-shrink", || async { Ok((100, 100)) }).await.unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let r = g
            .run("sw-shrink", move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok((30, 30))
                }
            })
            .await
            .unwrap();
        assert_eq!(r.entries, 30, "a genuine shrink is what the switch says");
        assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS, "retries are bounded");
        // The new level is the baseline: the next 30 is not suspicious.
        let r = g.run("sw-shrink", || async { Ok((30, 30)) }).await.unwrap();
        assert_eq!(r.attempts, 1);
    }

    #[tokio::test]
    async fn a_small_table_is_never_retried() {
        let g = gate::<usize>();
        g.run("sw-small", || async { Ok((10, 10)) }).await.unwrap();
        let r = g.run("sw-small", || async { Ok((2, 2)) }).await.unwrap();
        assert_eq!(r.attempts, 1, "too few rows to call a result short");
    }

    #[tokio::test]
    async fn a_failed_walk_fails_everyone_who_shared_it_and_the_next_caller_starts_fresh() {
        let g = gate::<u8>();
        let mut tasks = Vec::new();
        for _ in 0..3 {
            tasks.push(tokio::spawn(async move {
                g.run("sw-fail", || async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Err::<(u8, usize), _>(anyhow!("SNMP walk timed out"))
                })
                .await
            }));
        }
        for t in tasks {
            let err = t.await.unwrap().unwrap_err().to_string();
            assert!(err.contains("timed out"), "{err}");
        }
        let ok = g.run("sw-fail", || async { Ok((7u8, 1)) }).await.unwrap();
        assert_eq!(*ok.value, 7);
    }

    async fn futures_join<T>(tasks: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
        let mut out = Vec::new();
        for t in tasks {
            out.push(t.await.unwrap());
        }
        out
    }
}
