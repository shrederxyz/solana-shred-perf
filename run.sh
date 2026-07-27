#!/bin/bash
# Solana Shred Latency Benchmark.
# Captures shred packets with libpcap (no port binding, so it runs alongside your
# proxy/validator) and compares per-shred arrival latency. Requires sudo for capture.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="${SCRIPT_DIR}/target/release/solana-shred-perf"

# raw-ips: compare how fast the SAME shred arrives from different source IPs on ONE port.
# Use --label IP=NAME (repeatable) to show a friendly name instead of the raw address.
PORT="20001"
# Note: env vars must follow `sudo` so they aren't stripped from the child env.
#sudo RUST_LOG=info "${BINARY}" \
#    --interface any \
#    --ports "${PORT}" \
#    --report-interval-secs 300 \
#    --mode raw-ips \
#    --settle-window-ms 1000 \
#    --label 198.13.137.171=shreder.xyz \
#    --label 198.13.137.172=provider-2

# raw-ports: compare how fast the SAME shred arrives on different destination ports
# (e.g. two proxies writing to ports 20000 and 20005).
# sudo RUST_LOG=info "${BINARY}" \
#     --interface any \
#     --ports 20000,20001 \
#     --report-interval-secs 20 \
#     --mode raw-ports \
#     --settle-window-ms 1000 

# --shred-filter clean: count only shreds whose signature verifies against the slot leader
# (junk/unverified dropped). Requires RPC access for the leader schedule. Combines with
# either --mode. Default is --shred-filter all (no verification, no network).
# sudo RUST_LOG=info "${BINARY}" \
#     --interface any \
#     --ports "${PORT}" \
#     --report-interval-secs 20 \
#     --mode raw-ips \
#     --settle-window-ms 1000 \
#     --shred-filter clean \
#     --rpc-url https://api.mainnet-beta.solana.com \
#     --label 198.13.137.171=shreder.xyz \
#     --label 198.13.137.172=provider-2