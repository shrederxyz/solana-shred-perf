use etherparse::{NetSlice, SlicedPacket, TransportSlice};
use log::{info, warn};
use pcap::{Capture, Device};
use std::net::IpAddr;
use std::sync::mpsc;
use std::time::Instant;

use crate::constants::{CAPTURE_BUFFER_BYTES, STATS_LOG_INTERVAL};
use crate::event::Event;
use crate::timestamp::PcapTimestamp;
use crate::verifier::sanitize_shred_bytes;

pub fn list_devices() -> anyhow::Result<Vec<Device>> {
    Ok(Device::list()?)
}

/// Capture UDP packets on the given ports and forward the raw shred payloads.
///
/// This thread does only lightweight L2/L3/L4 parsing and hands the raw payload to the consumer.
/// Signature verification and shred parsing happen downstream, so nothing here can stall the
/// kernel capture ring and cause packet drops.
pub fn run_capture(interface: &str, ports: &[u16], tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
    let promisc = interface != "any";
    let mut cap = Capture::from_device(interface)?
        .promisc(promisc)
        .snaplen(2048)
        .buffer_size(CAPTURE_BUFFER_BYTES)
        .timeout(1000)
        .open()?;

    let filter = ports
        .iter()
        .map(|p| format!("udp dst port {}", p))
        .collect::<Vec<_>>()
        .join(" or ");
    cap.filter(&filter, true)?;
    info!("Capturing: interface={}, filter=({})", interface, filter);

    let mut last_stats_log = Instant::now();
    let mut last_dropped = 0u32;

    loop {
        // Log pcap drop stats periodically. Done at the top of the loop while `cap` is free of
        // the `next_packet` borrow. TimeoutExpired keeps this firing even with no traffic.
        if last_stats_log.elapsed() >= STATS_LOG_INTERVAL {
            if let Ok(stats) = cap.stats() {
                let delta = stats.dropped.saturating_sub(last_dropped);
                if delta > 0 {
                    warn!(
                        "pcap drops: +{} this window (received={}, dropped={}, if_dropped={})",
                        delta, stats.received, stats.dropped, stats.if_dropped
                    );
                } else {
                    info!(
                        "pcap stats: received={}, dropped={}, if_dropped={}",
                        stats.received, stats.dropped, stats.if_dropped
                    );
                }
                last_dropped = stats.dropped;
            }
            last_stats_log = Instant::now();
        }

        match cap.next_packet() {
            Ok(packet) => {
                let timestamp = PcapTimestamp::from_pcap_header(
                    packet.header.ts.tv_sec,
                    i64::from(packet.header.ts.tv_usec),
                );

                // The "any" pseudo-interface prepends a 16-byte Linux SLL header.
                let parsed = if interface == "any" {
                    if packet.data.len() > 16 {
                        SlicedPacket::from_ip(&packet.data[16..]).ok()
                    } else {
                        continue;
                    }
                } else {
                    SlicedPacket::from_ethernet(packet.data).ok()
                };

                let Some(parsed) = parsed else { continue };

                let source_ip = match parsed.net {
                    Some(NetSlice::Ipv4(v4)) => IpAddr::V4(v4.header().source_addr()),
                    Some(NetSlice::Ipv6(v6)) => IpAddr::V6(v6.header().source_addr()),
                    None => continue,
                };

                let (port, payload) = match parsed.transport {
                    Some(TransportSlice::Udp(udp)) => (udp.destination_port(), udp.payload()),
                    _ => continue,
                };

                // Drop garbage (wild slots, bad indices, wrong sizes) before allocating and
                // sending downstream. Allocation-free and constant-time, so it can't stall capture.
                if sanitize_shred_bytes(payload).is_none() {
                    continue;
                }

                let event = Event::Shred {
                    port,
                    source_ip,
                    payload: payload.to_vec(),
                    timestamp,
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
            Err(pcap::Error::TimeoutExpired) => continue,
            Err(e) => warn!("Capture error: {}", e),
        }
    }

    Ok(())
}
