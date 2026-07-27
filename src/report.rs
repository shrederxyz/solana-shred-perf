use log::info;
use std::collections::HashMap;
use std::time::Duration;

use crate::raw_state::{finalize, Key, RawState};

/// Percentiles reported for the per-key delay distribution.
const PERCENTILES: [u32; 6] = [10, 30, 50, 70, 90, 99];

/// One rendered table row: key label + matched count + win% + delay percentiles.
pub struct Row {
    pub(crate) label: String,
    pub(crate) matched: usize,
    pub(crate) win_pct: f64,
    pub(crate) percentiles: [Option<Duration>; 6],
}

/// Build sorted per-key rows from settled state. Iterates `known_keys` so a key with zero
/// traffic still appears (matched = 0). Percentiles include 0µs win samples, so a key winning
/// N% of shreds shows p(100-N) = 0 and tail lag becomes visible past that point. A key present
/// in `labels` is shown under its friendly name instead of its raw address/port.
pub fn compute_rows<K: Key>(raw: &RawState<K>, labels: &HashMap<String, String>) -> Vec<Row> {
    let total = raw.total_finalized;
    let mut keys: Vec<K> = raw.known_keys.iter().copied().collect();
    keys.sort();

    keys.into_iter()
        .map(|key| {
            let matched = raw.matched_by_key.get(&key).copied().unwrap_or(0);
            let win_pct = if total > 0 {
                raw.wins_by_key.get(&key).copied().unwrap_or(0) as f64 / total as f64 * 100.0
            } else {
                0.0
            };

            let mut percentiles: [Option<Duration>; 6] = [None; 6];
            if let Some(delays) = raw.delays_by_key.get(&key) {
                if !delays.is_empty() {
                    let mut sorted = delays.clone();
                    sorted.sort();
                    let n = sorted.len();
                    for (i, pct) in PERCENTILES.iter().enumerate() {
                        let idx = (((n as u64 * *pct as u64) / 100) as usize).min(n - 1);
                        percentiles[i] = Some(sorted[idx]);
                    }
                }
            }

            let key_str = key.to_string();
            let label = labels.get(&key_str).cloned().unwrap_or(key_str);
            Row {
                label,
                matched,
                win_pct,
                percentiles,
            }
        })
        .collect()
}

/// Finalize, then print the report for one mode. Borders are built from the column widths, so
/// the only per-mode differences are the key column's header, width, and alignment.
pub fn report_stats<K: Key>(
    raw: &mut RawState<K>,
    period: Duration,
    noun: &str,
    key_header: &str,
    key_width: usize,
    left_align: bool,
    labels: &HashMap<String, String>,
) {
    finalize(raw);

    if raw.total_finalized == 0 {
        info!(
            "No shreds finalized — {:.1}s | pending={} settle={}ms",
            period.as_secs_f64(),
            raw.pending.len(),
            raw.settle_window_us / 1_000,
        );
        return;
    }

    let rows = compute_rows(raw, labels);
    let header_lines = build_header_lines(noun, period, raw);
    render_table(&header_lines, key_header, key_width, left_align, &rows);

    // Pairwise diagnostic for the common two-key A/B case.
    if rows.len() == 2 {
        let (a, b) = (&rows[0], &rows[1]);
        let (faster, slower) = if a.win_pct >= b.win_pct { (a, b) } else { (b, a) };
        println!(
            "  A/B summary: {noun} {} wins {:.1}% vs {noun} {} wins {:.1}%. Loser p50={}, p99={}.",
            faster.label,
            faster.win_pct,
            slower.label,
            slower.win_pct,
            slower.percentiles[2].map(format_duration).unwrap_or_else(|| "-".to_string()),
            slower.percentiles[5].map(format_duration).unwrap_or_else(|| "-".to_string()),
        );
    }

    info!("Memory: {} pending shreds, {} {}s tracked", raw.pending.len(), raw.known_keys.len(), noun);
}

fn build_header_lines<K: Key>(noun: &str, period: Duration, raw: &RawState<K>) -> Vec<String> {
    vec![
        format!("Shred Latency by {noun} — {:.1}s", period.as_secs_f64()),
        format!(
            "Finalized: {}  Pending: {}  Settle: {}ms",
            raw.total_finalized,
            raw.pending.len(),
            raw.settle_window_us / 1_000,
        ),
    ]
}

/// Render a bordered table. Columns: key, Matched, Win %, p10..p99.
fn render_table(header_lines: &[String], key_header: &str, key_width: usize, left_align: bool, rows: &[Row]) {
    // Inner content widths per column (cell width = inner + 2 padding spaces).
    let mut widths: Vec<usize> = vec![key_width, 9, 6];
    widths.extend([8usize; 6]);
    let mut headers: Vec<&str> = vec![key_header, "Matched", "Win %"];
    headers.extend(["p10", "p30", "p50", "p70", "p90", "p99"]);

    let seg = |sep: &str| widths.iter().map(|w| "═".repeat(w + 2)).collect::<Vec<_>>().join(sep);
    let inner_total: usize = widths.iter().map(|w| w + 2).sum::<usize>() + (widths.len() - 1);

    let center = |text: &str, w: usize| {
        let len = text.chars().count();
        if len >= w {
            return text.to_string();
        }
        let left = (w - len) / 2;
        format!("{}{}{}", " ".repeat(left), text, " ".repeat(w - len - left))
    };

    println!("\n╔{}╗", "═".repeat(inner_total));
    for line in header_lines {
        println!("║{}║", pad_box_line(line, inner_total));
    }
    println!("╠{}╣", seg("╦"));
    let header_cells: Vec<String> = headers
        .iter()
        .zip(&widths)
        .map(|(h, w)| format!(" {} ", center(h, *w)))
        .collect();
    println!("║{}║", header_cells.join("║"));
    println!("╠{}╣", seg("╬"));

    for row in rows {
        let p: Vec<String> = row
            .percentiles
            .iter()
            .map(|p| p.map(format_duration).unwrap_or_else(|| "-".to_string()))
            .collect();
        let key_cell = if left_align {
            format!(" {:<w$} ", row.label, w = key_width)
        } else {
            format!(" {:>w$} ", row.label, w = key_width)
        };
        println!(
            "║{}║ {:>9} ║ {:>5.1}% ║ {:>8} ║ {:>8} ║ {:>8} ║ {:>8} ║ {:>8} ║ {:>8} ║",
            key_cell, row.matched, row.win_pct, p[0], p[1], p[2], p[3], p[4], p[5],
        );
    }
    println!("╚{}╝\n", seg("╩"));
}

fn pad_box_line(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.chars().take(width).collect()
    } else {
        format!(" {}{}", text, " ".repeat(width.saturating_sub(len + 1)))
    }
}

pub fn format_duration(d: Duration) -> String {
    let micros = d.as_micros();
    if micros < 1_000 {
        format!("{}µs", micros)
    } else if micros < 1_000_000 {
        format!("{:.2}ms", micros as f64 / 1_000.0)
    } else {
        format!("{:.2}s", micros as f64 / 1_000_000.0)
    }
}
