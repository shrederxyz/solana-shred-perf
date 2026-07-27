use clap::{Parser, ValueEnum};
use log::warn;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum Mode {
    /// Compare shred arrival latency across destination ports.
    RawPorts,
    /// Compare shred arrival latency across source IPs on a single port.
    RawIps,
}

/// Orthogonal to `Mode`: which shreds are counted. Applies to both raw-ports and raw-ips.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum ShredFilter {
    /// Count every parsed shred (no verification).
    All,
    /// Count only shreds whose signature verifies against the slot leader.
    Clean,
}

impl ShredFilter {
    /// Whether non-clean (junk/unverified) shreds should be dropped on ingest.
    pub fn clean_only(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about = "Benchmark Solana shred sources by per-shred arrival latency")]
pub struct Args {
    /// Network interface to capture on (e.g. "eth0", or "any").
    #[clap(long, default_value = "any")]
    pub interface: String,

    /// UDP destination ports to capture, comma-separated.
    #[clap(long, value_delimiter = ',', default_value = "20000")]
    pub ports: Vec<u16>,

    /// Seconds between reports.
    #[clap(long, default_value = "10")]
    pub report_interval_secs: u64,

    /// Comparison mode.
    #[clap(long, value_enum, default_value = "raw-ports")]
    pub mode: Mode,

    /// Which shreds to count: 'all' (every parsed shred) or 'clean' (only shreds whose
    /// signature verifies against the slot leader). 'clean' requires --rpc-url.
    #[clap(long, value_enum, default_value = "all")]
    pub shred_filter: ShredFilter,

    /// RPC endpoint used to fetch the leader schedule for clean-shred verification.
    #[clap(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub rpc_url: String,

    /// Hold each shred this long (ms) before finalizing, so late arrivals are not
    /// mis-counted as drops. Must exceed the largest expected arrival skew.
    #[clap(long, default_value = "1000")]
    pub settle_window_ms: u64,

    /// Give a key a friendly name in the table, as KEY=NAME (repeatable). KEY is a source IP
    /// in raw-ips or a destination port in raw-ports. E.g. --label 198.13.137.171=shreder.xyz
    #[clap(long = "label", value_name = "KEY=NAME")]
    pub labels: Vec<String>,
}

/// Parse `KEY=NAME` label specs into a lookup keyed by the key's display string.
pub fn parse_labels(specs: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for spec in specs {
        match spec.split_once('=') {
            Some((key, name)) if !key.trim().is_empty() && !name.trim().is_empty() => {
                map.insert(key.trim().to_string(), name.trim().to_string());
            }
            _ => warn!("Ignoring malformed --label '{}' (expected KEY=NAME)", spec),
        }
    }
    map
}
