use log::{info, warn};
use solana_pubkey::Pubkey;
use solana_rpc_client::rpc_client::RpcClient;
use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
};

use crate::constants::{PRE_FETCH_SLOTS_BEFORE_EPOCH_END, REFRESH_INTERVAL};

pub struct LeaderScheduleProvider {
    rpc_client: Arc<RpcClient>,
    schedule: RwLock<HashMap<u64, [u8; 32]>>,
    next_schedule: RwLock<HashMap<u64, [u8; 32]>>,
    current_epoch: AtomicU64,
    epoch_start_slot: AtomicU64,
    slots_in_epoch: AtomicU64,
    observed_slot: AtomicU64,
    /// Serializes refreshes so the background thread and the consumer's synchronous fallback
    /// never fire duplicate RPCs at the same time. Guards nothing beyond the critical section.
    refresh_lock: Mutex<()>,
}

impl LeaderScheduleProvider {
    pub fn new(rpc_client: Arc<RpcClient>) -> Self {
        Self {
            rpc_client,
            schedule: RwLock::new(HashMap::new()),
            next_schedule: RwLock::new(HashMap::new()),
            current_epoch: AtomicU64::new(u64::MAX),
            epoch_start_slot: AtomicU64::new(0),
            slots_in_epoch: AtomicU64::new(432_000),
            observed_slot: AtomicU64::new(0),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn get_leader(&self, slot: u64) -> Option<[u8; 32]> {
        if let Some(pubkey) = self.schedule.read().ok()?.get(&slot) {
            return Some(*pubkey);
        }
        self.next_schedule.read().ok()?.get(&slot).copied()
    }

    pub fn observe_slot(&self, slot: u64) {
        self.observed_slot.fetch_max(slot, Ordering::Relaxed);
    }

    pub fn refresh_initial(&self) -> anyhow::Result<()> {
        let initial_slot = self.rpc_client.get_slot()?;
        self.refresh_if_needed(initial_slot)
    }

    pub fn refresh_if_needed(&self, current_slot: u64) -> anyhow::Result<()> {
        let slots_in_epoch = self.slots_in_epoch.load(Ordering::Relaxed);
        if slots_in_epoch == 0 {
            return Ok(());
        }

        // Only one refresh may run at a time. The background thread and the consumer's synchronous
        // fallback can both call this concurrently; without coordination they would each fire
        // duplicate get_epoch_info/get_leader_schedule RPCs (the epoch guard below is
        // check-then-act, and current_epoch is committed only after the fetch completes). Block on
        // the lock so a caller that needs the schedule waits for the in-flight refresh instead of
        // giving up — then the double-checked epoch read below lets it reuse that result without a
        // second RPC. The lock guards only the refresh critical section, not get_leader reads, so
        // verification keeps serving from the current schedule while a refresh runs.
        let _guard = self
            .refresh_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Re-read the epoch inside the lock (double-checked): a refresh that just released the
        // lock may have already loaded the schedule for this epoch, so we skip a redundant fetch.
        let epoch = current_slot / slots_in_epoch;
        let stored_epoch = self.current_epoch.load(Ordering::Relaxed);

        if epoch != stored_epoch {
            // `get_epoch_info` ignores `current_slot` and returns the node's actual current epoch,
            // so the schedule we load below is always that epoch's. Record the epoch we actually
            // loaded — not the one computed from a possibly bogus `current_slot`. Otherwise a junk
            // slot would desync `current_epoch` from the loaded schedule and make the next
            // legitimate shred trigger a redundant refresh.
            let loaded_epoch = self.fetch_and_update_epoch_info()?;

            let epoch_start = self.epoch_start_slot.load(Ordering::Relaxed);
            let schedule = self.fetch_leader_schedule(Some(epoch_start))?;
            info!(
                "Refreshed leader schedule for epoch {loaded_epoch}: {} slots mapped",
                schedule.len()
            );
            *self.schedule.write().unwrap() = schedule;
            self.current_epoch.store(loaded_epoch, Ordering::Relaxed);
        }

        let epoch_start = self.epoch_start_slot.load(Ordering::Relaxed);
        let slots_remaining = (epoch_start + slots_in_epoch).saturating_sub(current_slot);
        if slots_remaining < PRE_FETCH_SLOTS_BEFORE_EPOCH_END {
            let next_epoch_start = epoch_start + slots_in_epoch;
            let next_schedule = self.next_schedule.read().unwrap();
            if next_schedule.is_empty() || !next_schedule.contains_key(&next_epoch_start) {
                drop(next_schedule);
                match self.fetch_leader_schedule(Some(next_epoch_start)) {
                    Ok(schedule) => {
                        info!(
                            "Pre-fetched leader schedule for next epoch: {} slots mapped",
                            schedule.len()
                        );
                        *self.next_schedule.write().unwrap() = schedule;
                    }
                    Err(e) => warn!("Failed to pre-fetch next epoch leader schedule: {e}"),
                }
            }
        }

        Ok(())
    }

    /// Fetch the node's current epoch info and update `epoch_start_slot`/`slots_in_epoch`.
    /// Returns the authoritative current epoch (the one whose schedule gets loaded), which may
    /// differ from an epoch computed from an untrusted incoming slot.
    fn fetch_and_update_epoch_info(&self) -> anyhow::Result<u64> {
        let epoch_info = self.rpc_client.get_epoch_info()?;
        let epoch_start = epoch_info.absolute_slot - epoch_info.slot_index;
        self.epoch_start_slot.store(epoch_start, Ordering::Relaxed);
        self.slots_in_epoch
            .store(epoch_info.slots_in_epoch, Ordering::Relaxed);
        Ok(epoch_info.epoch)
    }

    fn fetch_leader_schedule(&self, slot: Option<u64>) -> anyhow::Result<HashMap<u64, [u8; 32]>> {
        let rpc_schedule = self
            .rpc_client
            .get_leader_schedule(slot)?
            .ok_or_else(|| anyhow::anyhow!("leader schedule not available"))?;

        let epoch_start = self.epoch_start_slot.load(Ordering::Relaxed);
        let mut schedule = HashMap::new();

        for (pubkey_str, slot_offsets) in &rpc_schedule {
            let pubkey = Pubkey::from_str(pubkey_str)?;
            for &offset in slot_offsets {
                schedule.insert(epoch_start + offset as u64, pubkey.to_bytes());
            }
        }

        Ok(schedule)
    }

    pub fn start_refresh_thread(self: &Arc<Self>, exit: Arc<AtomicBool>) -> JoinHandle<()> {
        let provider = self.clone();
        thread::Builder::new()
            .name("leader_schedule_refresh".to_string())
            .spawn(move || {
                while !exit.load(Ordering::Relaxed) {
                    thread::sleep(REFRESH_INTERVAL);
                    let slot = match provider.observed_slot.load(Ordering::Relaxed) {
                        0 => provider.rpc_client.get_slot().unwrap_or(0),
                        slot => slot,
                    };
                    if slot > 0 {
                        if let Err(e) = provider.refresh_if_needed(slot) {
                            warn!("Failed to refresh leader schedule: {e}");
                        }
                    }
                }
            })
            .expect("failed to spawn leader schedule refresh thread")
    }
}
