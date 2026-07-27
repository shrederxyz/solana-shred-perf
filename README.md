# Solana Shred Latency Benchmark

**Packet-level Solana shred latency benchmarking for validators, relayers, proxies, and data teams.**

Use this tool to compare raw Solana shred delivery sources by destination port or source IP,
measure which source delivers each shred first, and quantify the delay distribution of the slower
paths.

Built by [Shreder](https://shreder.xyz) — ultra-low-latency Solana data infrastructure for teams
that need faster access to Solana shreds, transactions, and block data.

A lightweight tool for measuring **which shred source is fastest**.

It uses `libpcap` to passively capture Solana shred packets off the wire — it does **not**  
bind to the port, so it runs safely alongside your app, validator, proxy, or relayer without  
interfering with them. For every shred it sees, it records who delivered it first and how far  
behind everyone else was, then prints a periodic latency breakdown.

The same shred (identified by its network-wide `ShredId` of slot + index + type) often reaches
you from more than one place — two proxies writing to two ports, or several relayers sending to
the same port. This tool races those sources against each other, shred by shred.

With `--shred-filter clean`, the tool also filters out junk shreds. We’ve noticed that shred providers often try to manipulate latency test results. To win the race, they may send shreds that cannot be recovered or reconstructed. For some providers, the proportion of junk shreds can exceed 20%.

In that mode the tool drops these unreconstructable packets and shows a clean win rate based only on shreds that actually matter.

## Why use this

Latency claims are only useful when you can test them on your own machine, with your own network
path, using the same shreds. This benchmark helps you answer practical questions like:

- Which raw shred source reaches my host first most often?
- How large is the tail gap when a source loses?
- Does adding a second provider, proxy, relay, or region improve my arrival times?
- Are two feeds actually meaningfully different, or are they effectively tied at my location?

It is designed for latency-sensitive Solana teams that want a simple, reproducible way to compare
shred delivery paths without modifying their validator, proxy, or relayer setup.

## What it measures

For every matching Solana shred observed from multiple keys, the tool records:

- the first source to deliver that shred;
- each source's delay behind the first arrival;
- win rate per source;
- p10, p30, p50, p70, p90, and p99 delay distribution.

This makes it useful for A/B testing shred providers, comparing direct vs. proxied delivery,  
validating regional routing, or checking whether a failover path is also competitive on latency.

## The two available modes

You pick what to group and compare with `--mode`:

### `raw-ports` — compare destination ports

Compares how fast the **same shred** arrives on different **destination ports**. Use this when  
you have multiple deliverers each writing to its own  
port, and you want to know which port is faster.

Pass every port you want to compare in `--ports`.

### `raw-ips` — compare source IPs (recommended)

Compares how fast the **same shred** arrives from different **source IPs**, all on a **single
port**. Use this when several upstreams (e.g. shred providers) all send to the *same* port and you
want to know which IP is the fastest source.

Point `--ports` at the one combined port; sources are discovered automatically as they appear.

### Which one do I want?


| Your setup                                    | Mode        |
| --------------------------------------------- | ----------- |
| Multiple deliverers, each on its **own port** | `raw-ports` |
| Multiple sources, all on **one shared port**  | `raw-ips`   |


Both modes compute identical statistics — they only differ in what the rows are grouped by
(port vs. source IP).

## Reading the output

```
╔═════════════════════════════════════════════════════════════════════════════════════════════════╗
║ Shred Latency by port — 20.0s                                                                   ║
║ Finalized: 296134  Pending: 812  Settle: 1000ms                                                 ║
╠══════════╦═══════════╦════════╦══════════╦══════════╦══════════╦══════════╦══════════╦══════════╣
║   Port   ║  Matched  ║ Win %  ║   p10    ║   p30    ║   p50    ║   p70    ║   p90    ║   p99    ║
╠══════════╬═══════════╬════════╬══════════╬══════════╬══════════╬══════════╬══════════╬══════════╣
║    20000 ║    148233 ║  61.4% ║      0µs ║      0µs ║      0µs ║    180µs ║    640µs ║   2.10ms ║
║    20005 ║    147901 ║  38.6% ║      0µs ║     90µs ║    310µs ║    720µs ║   1.50ms ║   4.20ms ║
╚══════════╩═══════════╩════════╩══════════╩══════════╩══════════╩══════════╩══════════╩══════════╝
  A/B summary: port 20000 wins 61.4% vs port 20005 wins 38.6%. Loser p50=310µs, p99=4.20ms.
```

- **Matched** — how many finalized shreds this key received.
- **Win %** — share of shreds this key delivered **first**. Higher is faster.
- **p10–p99** — the delay distribution behind the winner. A key that won a shred contributes a
`0µs` sample, so if a key wins N% of shreds its percentiles read `0µs` up to about the
`p(100−N)` mark, and the tail past that shows how far behind it falls when it loses.
- **Finalized** — total shreds settled and scored this period. **Pending** — shreds still inside
the settle window, not yet scored.

When exactly two keys are present, an **A/B summary** line highlights the head-to-head.

## Building

Requires a Rust toolchain and `libpcap` headers.

```bash
# Debian/Ubuntu: sudo apt-get install libpcap-dev
cargo build --release
```

The binary is written to `target/release/solana-shred-perf`.

## Running

Packet capture needs elevated privileges, so run with `sudo` (or grant the binary
`cap_net_raw`).

**Compare two ports (`raw-ports`):**

```bash
sudo RUST_LOG=info ./target/release/solana-shred-perf \
    --interface any \
    --ports 20000,20005 \
    --mode raw-ports \
    --report-interval-secs 20 \
    --settle-window-ms 1000
```

**Compare source IPs on one port (`raw-ips`), naming a known source:**

```bash
sudo RUST_LOG=info ./target/release/solana-shred-perf \
    --interface any \
    --ports 20000 \
    --mode raw-ips \
    --report-interval-secs 20 \
    --settle-window-ms 1000 \
    --label 198.13.137.171=shreder.xyz
```

A ready-to-edit `run.sh` wraps the first command; uncomment the second block to switch modes.

## Options


| Flag                        | Default                               | Description                                                 |
| --------------------------- | ------------------------------------- | ----------------------------------------------------------- |
| `--interface`               | `any`                                 | Capture interface. `any` captures all interfaces (Linux).   |
| `--ports`                   | `20000`                               | Comma-separated UDP destination ports to capture.           |
| `--mode`                    | `raw-ports`                           | `raw-ports` or `raw-ips` (see above).                       |
| `--shred-filter`            | `all`                                 | `all` or `clean` (see below). `clean` requires `--rpc-url`. |
| `--rpc-url`                 | `https://api.mainnet-beta.solana.com` | RPC endpoint for the leader schedule (clean filter only).   |
| `--report-interval-secs`    | `10`                                  | Seconds between reports.                                    |
| `--settle-window-ms`        | `1000`                                | How long each shred is held before scoring.                 |
| `--label`                   | —                                     | `KEY=NAME` friendly name for a key; repeatable (see below). |


### Naming sources with `--label`

By default each row is labeled with its raw key — a source IP in `raw-ips`, a destination port
in `raw-ports`. Pass `--label KEY=NAME` to show a friendly name instead. The flag is repeatable,
and `KEY` must exactly match what the mode groups by:

```bash
# raw-ips: name source IPs
--label 198.13.137.171=shreder.xyz --label 198.13.137.172=provider-fra

# raw-ports: name destination ports
--label 20000=proxy-a --label 20005=proxy-b
```

The chosen row would then print as `shreder.xyz` instead of `198.13.137.171`. Unlabeled keys are
shown as-is. Keep names short (the IP column fits ~21 characters) so the table stays aligned.

### Filtering shreds with `--shred-filter`

Orthogonal to `--mode` (works with both `raw-ports` and `raw-ips`), this controls **which
shreds are counted**:

- `all` (default) — every parsed shred is counted.
- `clean` — each shred's signature is verified against the slot leader, and only verified
shreds are counted; junk and unverified packets are dropped before scoring. This requires
RPC access (`--rpc-url`) to fetch the leader schedule, which is refreshed automatically
across epoch boundaries.

Verification runs off the capture thread (in the consumer loop). Each shred is verified
individually with Ed25519 against the slot leader's pubkey and the shred's Merkle root.

```bash
# raw-ips over clean shreds only
sudo RUST_LOG=info ./target/release/solana-shred-perf \
    --interface any \
    --ports 20000 \
    --mode raw-ips \
    --shred-filter clean \
    --rpc-url https://api.mainnet-beta.solana.com
```

### About `--settle-window-ms`

A shred isn't scored the instant it's first seen — it's held for the settle window so a slower
source's copy has time to arrive and be counted as a (delayed) match rather than a false drop.
Set it **larger than the maximum arrival skew** you expect between sources. `1000ms` is a safe
default that stays well under a typical report interval; lower it only once you've confirmed
your real inter-source skew is much smaller. The window is measured on the packet capture clock,
not wall-clock time.

## How it works

1. `libpcap` captures UDP packets on the requested ports and stamps each with the kernel
  capture time.
2. Each payload is parsed as a Solana shred to extract its `ShredId`. With `--shred-filter
  clean`, the shred's signature is also verified against the slot leader and dropped unless it
  passes.
3. The first arrival per key is recorded per shred (duplicate copies from the same key are
  ignored, so retransmits don't skew the timing).
4. Once a shred's oldest arrival is older than the settle window it's finalized: the earliest
  key wins, and every key's delay from that first arrival is recorded.
5. Every `--report-interval-secs`, the collected delays are turned into the per-key table above.

Memory is bounded: in-flight shreds are capped and the oldest slots are evicted under pressure.

## Benchmarking Shreder

Want to compare Shreder against your current shred source?

1. Send Shreder and the other source to the same host.
2. Use `raw-ips` if both feeds land on one shared port, or `raw-ports` if each source writes to a
  separate port.
3. Add `--label` values so the output is easy to read.
4. Compare `Win %`, p50, p90, and p99 over several reporting periods.

Shreder provides ultra-low-latency Solana data feeds including raw shreds, decoded shreds, Binary, Preconfs,  
and Geyser/Fastlane.

Learn more at [shreder.xyz](https://shreder.xyz).

## Contributing

Issues and PRs are welcome.