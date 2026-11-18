//! Side-by-side probe for the SNMP MAC-table lookup: N concurrent readers, each
//! reading the table R times, through either the raw walker (`raw`: overlapping
//! walks, what a switch's agent cuts short) or the gated lookup (`gated`: one
//! walk at a time per switch, shared by concurrent callers).
//!
//!   cargo run --release --example snmp_probe -- <host> <community> <raw|gated> [readers=6] [rounds=4]
//!
//! Prints how many entries each read returned. A complete table is the same
//! number every time; a cut-short one is smaller.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use switch_configurator::snmp::client::{Session, SnmpTarget};
use switch_configurator::snmp::{lookup, mapping};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: snmp_probe <host> <community> <raw|gated> [readers=6] [rounds=4]");
        std::process::exit(2);
    }
    let target = SnmpTarget { host: args[1].clone(), port: 161, community: args[2].clone(), timeout: Duration::from_secs(2) };
    let gated = args[3] == "gated";
    let readers: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(6);
    let rounds: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(4);

    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..readers {
        let target = target.clone();
        tasks.push(tokio::spawn(async move {
            let mut counts = Vec::new();
            for _ in 0..rounds {
                let n = if gated {
                    lookup::fetch_mac_table(&target).await.map(|(entries, _)| entries.len())
                } else {
                    // The ungated path: its own session and walk per reader.
                    async {
                        let session = Session::connect(&target).await?;
                        let rows = session.walk(mapping::FDB_TABLE).await?;
                        Ok::<_, anyhow::Error>(rows.iter().filter(|(oid, _)| oid.starts_with(mapping::FDB_PORT)).count())
                    }
                    .await
                };
                counts.push(n.map_err(|e| format!("{e:#}")));
            }
            counts
        }));
    }
    let mut histogram: BTreeMap<String, usize> = BTreeMap::new();
    let mut reads = 0;
    for task in tasks {
        for result in task.await.expect("reader task") {
            reads += 1;
            *histogram.entry(result.map_or_else(|e| format!("error: {e}"), |n| n.to_string())).or_default() += 1;
        }
    }
    println!("{} mode: {readers} readers x {rounds} rounds = {reads} reads in {:.1} s", args[3], started.elapsed().as_secs_f64());
    println!("entries returned (value: reads):");
    for (value, count) in &histogram {
        println!("  {value}: {count}");
    }
}
