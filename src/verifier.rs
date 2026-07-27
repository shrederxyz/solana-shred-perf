use log::{debug, warn};
use solana_hash::Hash;
use solana_ledger::blockstore::MAX_DATA_SHREDS_PER_SLOT;
use solana_ledger::shred::{wire, ShredType};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::{sync::Arc, time::Instant};

use crate::constants::{NO_LEADER_WARN_INTERVAL, SIGNATURE_BYTES};
use crate::leader_schedule::LeaderScheduleProvider;

/// Minimum bytes needed to read the shred header fields sanitize inspects.
const MIN_SHRED_HEADER_SIZE: usize = 83;
/// Byte offset of the FEC set index (after signature:64 + variant:1 + slot:8 + index:4 + version:2).
const OFFSET_FEC_SET_INDEX: usize = 79;

/// Result of [`sanitize_shred_bytes`]. `index`/`fec_set_index` are kept for parity with the
/// shreder-proxy source and possible future use; this crate currently only needs the pass/fail.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct SanitizedShredInfo {
    pub slot: u64,
    pub index: usize,
    pub fec_set_index: u32,
}

/// Lightweight, allocation-free validation of raw shred bytes before verification/deserialization.
/// Catches garbage packets (wild slots, bad indices, wrong payload sizes) cheaply so junk never
/// reaches the verifier (and its leader-schedule RPC) or the shred parser.
pub fn sanitize_shred_bytes(data: &[u8]) -> Option<SanitizedShredInfo> {
    if data.len() < MIN_SHRED_HEADER_SIZE {
        return None;
    }

    let shred_type = wire::get_shred_id(data).map(|id| id.shred_type())?;

    let slot = wire::get_slot(data)?;
    if slot > u64::MAX / 2 {
        return None;
    }

    let index = wire::get_index(data)?;
    if index as usize >= MAX_DATA_SHREDS_PER_SLOT {
        return None;
    }

    let fec_set_index = u32::from_le_bytes(
        data[OFFSET_FEC_SET_INDEX..OFFSET_FEC_SET_INDEX + 4]
            .try_into()
            .ok()?,
    );
    if fec_set_index as usize >= MAX_DATA_SHREDS_PER_SLOT {
        return None;
    }

    // Merkle shred payload sizes differ by type:
    //   ShredCode: PACKET_DATA_SIZE (1232) - SIZE_OF_NONCE (4) = 1228
    //   ShredData: 1228 - SIZE_OF_CODING_SHRED_HEADERS (89) + SIZE_OF_SIGNATURE (64) = 1203
    const MERKLE_CODE_PAYLOAD_SIZE: usize = 1228;
    const MERKLE_DATA_PAYLOAD_SIZE: usize = 1203;
    let payload_size_valid = match shred_type {
        ShredType::Data => data.len() == MERKLE_DATA_PAYLOAD_SIZE,
        ShredType::Code => data.len() == MERKLE_CODE_PAYLOAD_SIZE,
    };
    if !payload_size_valid {
        return None;
    }

    Some(SanitizedShredInfo {
        slot,
        index: index as usize,
        fec_set_index,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShredAcceptance {
    Verified,
    Unverified,
    Rejected,
}

pub struct ShredVerifier {
    leader_schedule: Arc<LeaderScheduleProvider>,
    last_no_leader_warn: Option<Instant>,
}

impl ShredVerifier {
    pub fn new(leader_schedule: Arc<LeaderScheduleProvider>) -> Self {
        Self {
            leader_schedule,
            last_no_leader_warn: None,
        }
    }

    pub fn observe_slot(&self, slot: u64) {
        self.leader_schedule.observe_slot(slot);
    }

    pub fn should_accept_with_merkle_root(
        &mut self,
        packet: &[u8],
        slot: u64,
        merkle_root: Option<Hash>,
    ) -> ShredAcceptance {
        let Some(signature) = get_signature(packet) else {
            return ShredAcceptance::Rejected;
        };

        // Cheap check before touching the leader schedule (and any fallback RPC): a shred with
        // no merkle root can never be signature-verified, so reject junk here.
        let Some(merkle_root) = merkle_root else {
            return ShredAcceptance::Unverified;
        };

        let pubkey_bytes = match self.leader_schedule.get_leader(slot) {
            Some(pubkey) => pubkey,
            None => {
                if let Err(e) = self.leader_schedule.refresh_if_needed(slot) {
                    warn!("Failed to refresh leader schedule for slot={slot}: {e}");
                }
                match self.leader_schedule.get_leader(slot) {
                    Some(pubkey) => pubkey,
                    None => {
                        let now = Instant::now();
                        let should_warn = self
                            .last_no_leader_warn
                            .is_none_or(|last| now.duration_since(last) >= NO_LEADER_WARN_INTERVAL);
                        if should_warn {
                            self.last_no_leader_warn = Some(now);
                            warn!(
                                "leader schedule has no entry for slot={slot}; accepting shred as unverified"
                            );
                        }
                        return ShredAcceptance::Unverified;
                    }
                }
            }
        };
        let pubkey = Pubkey::from(pubkey_bytes);

        let verified = signature.verify(pubkey.as_ref(), merkle_root.as_ref());
        if verified {
            ShredAcceptance::Verified
        } else {
            debug!("Shred verification FAILED: slot={slot}, sig={signature}");
            ShredAcceptance::Rejected
        }
    }
}

pub fn get_signature(data: &[u8]) -> Option<Signature> {
    let bytes = <[u8; SIGNATURE_BYTES]>::try_from(data.get(..SIGNATURE_BYTES)?).ok()?;
    Some(Signature::from(bytes))
}
