use solana_ledger::shred::{ShredId, ShredType};
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::cli::parse_labels;
use crate::raw_state::{finalize, record_arrival, Key, RawState};
use crate::report::{compute_rows, Row};
use crate::timestamp::PcapTimestamp;
use crate::verifier::sanitize_shred_bytes;

/// A buffer shorter than the shred header must be rejected outright (no panic, no read OOB).
#[test]
fn sanitize_rejects_short_buffer() {
    assert!(sanitize_shred_bytes(&[]).is_none());
    assert!(sanitize_shred_bytes(&[0u8; 10]).is_none());
    assert!(sanitize_shred_bytes(&[0u8; 82]).is_none());
}

fn sid(slot: u64, index: u32) -> ShredId {
    ShredId::new(slot, index, ShredType::Data)
}

fn ts(micros: u64) -> PcapTimestamp {
    PcapTimestamp { micros }
}

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
}

/// Finalize everything pending by pushing "now" far into the future.
fn finalize_all<K: Key>(raw: &mut RawState<K>) {
    raw.latest_ts = Some(ts(u64::MAX / 2));
    finalize(raw);
}

fn row<'a>(rows: &'a [Row], label: &str) -> &'a Row {
    rows.iter().find(|r| r.label == label).expect("row present")
}

fn us(d: Option<Duration>) -> u64 {
    d.expect("percentile present").as_micros() as u64
}

/// Core comparison test (raw-ips): three IPs deliver an overlapping stream; every win,
/// delay, matched, and percentile value is hand-computed and asserted.
#[test]
fn ip_comparison_is_correct() {
    let (a, b, c) = (ip(1), ip(2), ip(3));
    let mut raw: RawState<IpAddr> = RawState::new(0, HashSet::new());

    // S1: A first (1000), B +500, C +1000 — all three see it.
    record_arrival(&mut raw, a, sid(1, 0), ts(1000));
    record_arrival(&mut raw, b, sid(1, 0), ts(1500));
    record_arrival(&mut raw, c, sid(1, 0), ts(2000));
    // S2: A first (1000), B +200. C never sees it -> C drop.
    record_arrival(&mut raw, a, sid(2, 0), ts(1000));
    record_arrival(&mut raw, b, sid(2, 0), ts(1200));
    // S3: B first (1000), A +300. C never sees it -> C drop.
    record_arrival(&mut raw, b, sid(3, 0), ts(1000));
    record_arrival(&mut raw, a, sid(3, 0), ts(1300));

    finalize_all(&mut raw);
    let total = raw.total_finalized;
    assert_eq!(total, 3, "three shreds finalized");

    let rows = compute_rows(&raw, &HashMap::new());
    assert_eq!(rows.len(), 3, "one row per known IP");
    let (ra, rb, rc) = (row(&rows, "10.0.0.1"), row(&rows, "10.0.0.2"), row(&rows, "10.0.0.3"));

    // Wins: A won S1 & S2, B won S3, C won nothing.
    assert_eq!((ra.win_pct * 3.0).round(), 200.0, "A wins 2/3");
    assert_eq!((rb.win_pct * 3.0).round(), 100.0, "B wins 1/3");
    assert_eq!(rc.win_pct, 0.0, "C wins none");

    // Matched (and derived drops = total - matched). A&B saw all 3; C saw only S1.
    assert_eq!((ra.matched, total - ra.matched), (3, 0));
    assert_eq!((rb.matched, total - rb.matched), (3, 0));
    assert_eq!((rc.matched, total - rc.matched), (1, 2));

    // A delays sorted: [0, 0, 300].
    assert_eq!(us(ra.percentiles[0]), 0); // p10
    assert_eq!(us(ra.percentiles[2]), 0); // p50
    assert_eq!(us(ra.percentiles[3]), 300); // p70
    assert_eq!(us(ra.percentiles[5]), 300); // p99
    // B delays sorted: [0, 200, 500].
    assert_eq!(us(rb.percentiles[2]), 200); // p50
    assert_eq!(us(rb.percentiles[3]), 500); // p70
    // C delay: single sample [1000] -> every percentile is 1000.
    for p in 0..6 {
        assert_eq!(us(rc.percentiles[p]), 1000, "C p-index {}", p);
    }
}

/// First-arrival-per-key dedup: a repeated copy from the same key must NOT move the
/// recorded arrival time (protects the delay math from retransmits).
#[test]
fn per_key_first_arrival_wins_dedup() {
    let (a, b) = (ip(1), ip(2));
    let mut raw: RawState<IpAddr> = RawState::new(0, HashSet::new());

    record_arrival(&mut raw, a, sid(10, 0), ts(1000)); // A first arrival = 1000
    record_arrival(&mut raw, a, sid(10, 0), ts(1200)); // duplicate, ignored
    record_arrival(&mut raw, b, sid(10, 0), ts(1400)); // B arrives at 1400

    finalize_all(&mut raw);
    let rows = compute_rows(&raw, &HashMap::new());
    // A won at 1000; B lags by 1400-1000 = 400 (NOT 1400-1200 = 200).
    assert_eq!(us(row(&rows, "10.0.0.2").percentiles[2]), 400);
    assert_eq!(row(&rows, "10.0.0.1").win_pct.round(), 100.0);
}

/// Same machinery keyed by port (raw-ports) must produce equivalent results.
#[test]
fn port_keying_matches() {
    let mut raw: RawState<u16> = RawState::new(0, [20000u16, 20005u16].into_iter().collect());
    record_arrival(&mut raw, 20000, sid(1, 0), ts(1000)); // port 20000 first
    record_arrival(&mut raw, 20005, sid(1, 0), ts(1250)); // +250
    finalize_all(&mut raw);

    let rows = compute_rows(&raw, &HashMap::new());
    assert_eq!(rows.len(), 2);
    assert_eq!(row(&rows, "20000").win_pct.round(), 100.0);
    assert_eq!(us(row(&rows, "20000").percentiles[2]), 0);
    assert_eq!(us(row(&rows, "20005").percentiles[2]), 250);
}

/// A seeded key with zero traffic must appear as a full-drop row (this is what makes
/// drop counts trustworthy in raw-ports).
#[test]
fn zero_traffic_seeded_port_shows_full_drops() {
    let mut raw: RawState<u16> = RawState::new(0, [20000u16, 20005u16].into_iter().collect());
    record_arrival(&mut raw, 20000, sid(1, 0), ts(1000)); // only 20000 sees anything
    finalize_all(&mut raw);
    let total = raw.total_finalized;

    let rows = compute_rows(&raw, &HashMap::new());
    let dead = row(&rows, "20005");
    assert_eq!((dead.matched, total - dead.matched), (0, 1));
}

/// A labeled key is shown under its friendly name; unlabeled keys keep their raw address.
#[test]
fn labels_rename_keys() {
    let (a, b) = (ip(1), ip(2));
    let mut raw: RawState<IpAddr> = RawState::new(0, HashSet::new());
    record_arrival(&mut raw, a, sid(1, 0), ts(1000));
    record_arrival(&mut raw, b, sid(1, 0), ts(1200));
    finalize_all(&mut raw);

    let mut labels = HashMap::new();
    labels.insert("10.0.0.1".to_string(), "shreder.xyz".to_string());
    let rows = compute_rows(&raw, &labels);

    assert!(rows.iter().any(|r| r.label == "shreder.xyz"), "labeled IP renamed");
    assert!(rows.iter().any(|r| r.label == "10.0.0.2"), "unlabeled IP unchanged");
}

#[test]
fn parse_labels_handles_valid_and_malformed() {
    let specs = vec![
        "198.13.137.171=shreder.xyz".to_string(),
        "  20000 = proxy A ".to_string(), // trimmed
        "no-equals".to_string(),          // ignored
        "=name".to_string(),              // ignored (empty key)
        "key=".to_string(),               // ignored (empty name)
    ];
    let labels = parse_labels(&specs);
    assert_eq!(labels.len(), 2);
    assert_eq!(labels.get("198.13.137.171").unwrap(), "shreder.xyz");
    assert_eq!(labels.get("20000").unwrap(), "proxy A");
}
