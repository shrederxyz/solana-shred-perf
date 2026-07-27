use std::net::IpAddr;

use crate::timestamp::PcapTimestamp;

/// A captured shred payload (verified and parsed downstream), or a periodic report signal.
pub enum Event {
    Shred {
        port: u16,
        source_ip: IpAddr,
        payload: Vec<u8>,
        timestamp: PcapTimestamp,
    },
    Tick,
}
