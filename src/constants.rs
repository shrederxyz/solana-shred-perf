use std::time::Duration;

pub const SIGNATURE_BYTES: usize = 64;
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
pub const PRE_FETCH_SLOTS_BEFORE_EPOCH_END: u64 = 1000;
pub const NO_LEADER_WARN_INTERVAL: Duration = Duration::from_secs(5);

/// pcap kernel buffer size (~32 MiB) so bursts survive while the consumer catches up.
pub const CAPTURE_BUFFER_BYTES: i32 = 32 * 1024 * 1024;
/// How often the capture thread logs pcap drop stats.
pub const STATS_LOG_INTERVAL: Duration = Duration::from_secs(10);
