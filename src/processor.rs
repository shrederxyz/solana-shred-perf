use solana_ledger::shred::ShredId;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Instant;

use crate::cli::Mode;
use crate::raw_state::{record_arrival, RawState};
use crate::report::report_stats;
use crate::timestamp::PcapTimestamp;

/// Owns the state for the active mode and drives ingest + reporting.
pub struct Processor {
    mode: Mode,
    ports: Option<RawState<u16>>,
    ips: Option<RawState<IpAddr>>,
    labels: HashMap<String, String>,
    period_start: Instant,
}

impl Processor {
    pub fn new(mode: Mode, settle_window_ms: u64, ports: &[u16], labels: HashMap<String, String>) -> Self {
        Self {
            mode,
            ports: matches!(mode, Mode::RawPorts)
                .then(|| RawState::new(settle_window_ms, ports.iter().copied().collect())),
            ips: matches!(mode, Mode::RawIps).then(|| RawState::new(settle_window_ms, HashSet::new())),
            labels,
            period_start: Instant::now(),
        }
    }

    pub fn ingest(&mut self, port: u16, source_ip: IpAddr, shred_id: ShredId, ts: PcapTimestamp) {
        if let Some(state) = self.ports.as_mut() {
            record_arrival(state, port, shred_id, ts);
        }
        if let Some(state) = self.ips.as_mut() {
            record_arrival(state, source_ip, shred_id, ts);
        }
    }

    pub fn report(&mut self) {
        let period = self.period_start.elapsed();
        match self.mode {
            Mode::RawPorts => {
                report_stats(self.ports.as_mut().unwrap(), period, "port", "Port", 8, false, &self.labels)
            }
            Mode::RawIps => {
                report_stats(self.ips.as_mut().unwrap(), period, "IP", "Source IP", 21, true, &self.labels)
            }
        }
        self.period_start = Instant::now();
        if let Some(state) = self.ports.as_mut() {
            state.reset_period();
        }
        if let Some(state) = self.ips.as_mut() {
            state.reset_period();
        }
    }
}
