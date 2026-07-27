use std::time::Duration;

/// Kernel capture timestamp (microsecond resolution), more accurate than `Instant::now()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PcapTimestamp {
    pub(crate) micros: u64,
}

impl PcapTimestamp {
    pub fn from_pcap_header(ts_sec: i64, ts_usec: i64) -> Self {
        Self {
            micros: ts_sec as u64 * 1_000_000 + ts_usec as u64,
        }
    }

    pub fn duration_since(&self, earlier: PcapTimestamp) -> Duration {
        Duration::from_micros(self.micros.saturating_sub(earlier.micros))
    }
}
