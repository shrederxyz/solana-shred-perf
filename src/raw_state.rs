use solana_ledger::shred::ShredId;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::time::Duration;

use crate::timestamp::PcapTimestamp;

/// Upper bound on in-flight shreds; oldest slots are evicted past this.
const MAX_PENDING: usize = 500_000;

/// The set of trait bounds a grouping key must satisfy.
pub trait Key: Eq + Hash + Copy + Ord + fmt::Display {}
impl<T: Eq + Hash + Copy + Ord + fmt::Display> Key for T {}

/// Per-shred arrival tracking, generic over the grouping key `K` (destination port for
/// `raw-ports`, source IP for `raw-ips`).
///
/// Each shred is held until the settle window elapses, then finalized: the key that saw it
/// first "wins", and every other key's delay from that first arrival is recorded. A key that
/// never saw a finalized shred counts it as a drop. Timestamps come from the kernel capture
/// clock, and finalization uses the latest observed timestamp as "now" — so the settle window
/// is measured on the same clock as the delays, never on wall-clock time.
pub struct RawState<K: Key> {
    /// First arrival timestamp per (shred, key).
    pub(crate) pending: HashMap<ShredId, HashMap<K, PcapTimestamp>>,
    /// Earliest arrival across keys for each pending shred.
    pub(crate) pending_oldest: HashMap<ShredId, PcapTimestamp>,
    /// Delays from the winning key, per key (includes 0µs samples for wins).
    pub(crate) delays_by_key: HashMap<K, Vec<Duration>>,
    /// Finalized shreds each key saw.
    pub(crate) matched_by_key: HashMap<K, usize>,
    /// Shreds each key received first.
    pub(crate) wins_by_key: HashMap<K, usize>,
    /// Shreds finalized this period.
    pub(crate) total_finalized: usize,
    /// Keys to report. Ports are seeded up front so a silent port still shows as full drops;
    /// IPs are discovered as they appear.
    pub(crate) known_keys: HashSet<K>,
    /// Settle window in microseconds.
    pub(crate) settle_window_us: u64,
    /// Latest capture timestamp; used as "now" for the settle check.
    pub(crate) latest_ts: Option<PcapTimestamp>,
    /// Reject shreds at or below this slot after eviction.
    pub(crate) min_evicted_slot: u64,
}

impl<K: Key> RawState<K> {
    pub fn new(settle_window_ms: u64, known_keys: HashSet<K>) -> Self {
        Self {
            pending: HashMap::new(),
            pending_oldest: HashMap::new(),
            delays_by_key: HashMap::new(),
            matched_by_key: HashMap::new(),
            wins_by_key: HashMap::new(),
            total_finalized: 0,
            known_keys,
            settle_window_us: settle_window_ms.saturating_mul(1_000),
            latest_ts: None,
            min_evicted_slot: 0,
        }
    }

    /// Clear per-period counters. `pending`/`pending_oldest`/`latest_ts` persist so shreds
    /// straddling a report boundary still finalize.
    pub fn reset_period(&mut self) {
        self.delays_by_key.clear();
        self.matched_by_key.clear();
        self.wins_by_key.clear();
        self.total_finalized = 0;
    }
}

/// Record one shred arrival for a key, keeping only the first arrival per key so retransmits
/// don't perturb the delay math.
pub fn record_arrival<K: Key>(raw: &mut RawState<K>, key: K, shred_id: ShredId, timestamp: PcapTimestamp) {
    let slot = shred_id.slot();
    if raw.min_evicted_slot > 0 && slot <= raw.min_evicted_slot {
        return;
    }

    raw.known_keys.insert(key);
    raw.latest_ts = Some(match raw.latest_ts {
        Some(prev) if prev > timestamp => prev,
        _ => timestamp,
    });

    raw.pending.entry(shred_id).or_default().entry(key).or_insert(timestamp);
    match raw.pending_oldest.get_mut(&shred_id) {
        Some(prev) if *prev > timestamp => *prev = timestamp,
        Some(_) => {}
        None => {
            raw.pending_oldest.insert(shred_id, timestamp);
        }
    }

    if raw.pending.len() > MAX_PENDING {
        let oldest = raw.pending.keys().map(|s| s.slot()).min().unwrap_or(0);
        raw.pending.retain(|s, _| s.slot() > oldest);
        raw.pending_oldest.retain(|s, _| s.slot() > oldest);
        raw.min_evicted_slot = oldest;
    }
}

/// Finalize every shred whose oldest arrival is older than the settle window.
pub fn finalize<K: Key>(raw: &mut RawState<K>) {
    let now = match raw.latest_ts {
        Some(ts) => ts,
        None => return,
    };
    let settle = raw.settle_window_us;

    let ready: Vec<ShredId> = raw
        .pending_oldest
        .iter()
        .filter(|(_, oldest)| now.duration_since(**oldest).as_micros() as u64 > settle)
        .map(|(id, _)| *id)
        .collect();

    for shred_id in ready {
        let Some(arrivals) = raw.pending.remove(&shred_id) else { continue };
        raw.pending_oldest.remove(&shred_id);

        let Some((winner, t_ref)) = arrivals.iter().min_by_key(|(_, ts)| **ts).map(|(k, ts)| (*k, *ts))
        else {
            continue;
        };
        *raw.wins_by_key.entry(winner).or_insert(0) += 1;

        // Disjoint field borrows: read `known_keys` while mutating the delay/matched maps.
        for &key in &raw.known_keys {
            if let Some(&ts) = arrivals.get(&key) {
                raw.delays_by_key.entry(key).or_default().push(ts.duration_since(t_ref));
                *raw.matched_by_key.entry(key).or_insert(0) += 1;
            }
        }

        raw.total_finalized += 1;
    }
}
