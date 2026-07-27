mod capture;
mod cli;
mod constants;
mod event;
mod leader_schedule;
mod processor;
mod raw_state;
mod report;
mod timestamp;
mod verifier;

#[cfg(test)]
mod tests;

use clap::Parser;
use log::{error, info};
use solana_ledger::shred::{wire, Shred};
use solana_rpc_client::rpc_client::RpcClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::capture::{list_devices, run_capture};
use crate::cli::{parse_labels, Args};
use crate::event::Event;
use crate::leader_schedule::LeaderScheduleProvider;
use crate::processor::Processor;
use crate::verifier::{ShredAcceptance, ShredVerifier};

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();
    let args = Args::parse();

    info!(
        "Solana shred latency benchmark — mode {:?}, interface {}, ports {:?}",
        args.mode, args.interface, args.ports
    );
    info!(
        "Settle window: {} ms | report interval: {} s",
        args.settle_window_ms, args.report_interval_secs
    );
    info!("Available interfaces:");
    for dev in list_devices()? {
        info!("  - {} {:?}", dev.name, dev.desc);
    }

    // In the clean filter, spin up the leader schedule (RPC-backed) and a verifier. Verification
    // runs in the consumer loop below (off the capture thread), so any fallback RPC stall cannot
    // drop packets at the NIC. The `all` filter needs no network.
    let mut leader_schedule_exit = None;
    let mut leader_schedule_thread = None;
    let mut verifier = if args.shred_filter.clean_only() {
        info!("RPC URL for leader schedule: {}", args.rpc_url);
        let provider = Arc::new(LeaderScheduleProvider::new(Arc::new(RpcClient::new(
            args.rpc_url.clone(),
        ))));
        provider.refresh_initial()?;
        let exit = Arc::new(AtomicBool::new(false));
        let refresh_thread = provider.start_refresh_thread(exit.clone());
        leader_schedule_exit = Some(exit);
        leader_schedule_thread = Some(refresh_thread);
        Some(ShredVerifier::new(provider))
    } else {
        None
    };

    let (tx, rx) = mpsc::channel();

    let interface = args.interface.clone();
    let ports = args.ports.clone();
    let tx_capture = tx.clone();
    let capture_thread = thread::spawn(move || {
        if let Err(e) = run_capture(&interface, &ports, tx_capture) {
            error!("Capture error: {}", e);
        }
    });

    let interval = args.report_interval_secs;
    let tx_tick = tx.clone();
    let tick_thread = thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(interval));
        if tx_tick.send(Event::Tick).is_err() {
            break;
        }
    });

    let labels = parse_labels(&args.labels);
    for (key, name) in &labels {
        info!("Label: {} = {}", key, name);
    }

    let mut processor = Processor::new(args.mode, args.settle_window_ms, &args.ports, labels);
    while let Ok(event) = rx.recv() {
        match event {
            Event::Shred {
                port,
                source_ip,
                payload,
                timestamp,
            } => {
                // Clean filter: verify against the slot leader before parsing/ingesting.
                if let Some(v) = verifier.as_mut() {
                    let Some(sid) = wire::get_shred_id(&payload) else {
                        continue;
                    };
                    v.observe_slot(sid.slot());
                    let merkle_root = wire::get_merkle_root(&payload);
                    if v.should_accept_with_merkle_root(&payload, sid.slot(), merkle_root)
                        != ShredAcceptance::Verified
                    {
                        continue;
                    }
                }
                if let Ok(shred) = Shred::new_from_serialized_shred(payload) {
                    processor.ingest(port, source_ip, shred.id(), timestamp);
                }
            }
            Event::Tick => processor.report(),
        }
    }

    if let Some(exit) = leader_schedule_exit {
        exit.store(true, Ordering::Relaxed);
    }

    capture_thread.join().ok();
    tick_thread.join().ok();
    if let Some(refresh_thread) = leader_schedule_thread {
        refresh_thread.join().ok();
    }
    Ok(())
}
