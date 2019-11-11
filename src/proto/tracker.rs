use std::convert::TryInto;
use std::io;
use std::ops::Add;
use std::{convert::TryFrom, time::Duration};

use sha1::Sha1;
use wasm_timer::Instant;

use libp2p_bittorrent_tracker::util::ShaHash;

// TODO use a struct that is compliant with the tracker spec
use crate::peer::BttPeer;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum EventType {
    Started = 1,
    Completed = 2,
    Stopped = 3,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TrackerRequestMsg {
    pub info_hash: ShaHash,

    pub peer: BttPeer,

    pub uploaded: u64,

    pub downloaded: u64,

    pub left: u64,

    pub numwant: u32,

    pub event: Option<EventType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerResponseMsg {
    Failure {
        reason: String,
    },
    Success {
        /// amount of seeders
        complete: u32,
        /// amount of leechers
        incomplete: u32,
        /// time after when the downloader should send rerequest
        interval: Instant,
        /// Minimum announce interval
        min_interval: Option<Duration>,
        /// matching peers
        peers: Vec<BttPeer>,
    },
}

/// Return format of the peer info returned BitTorrent tracker.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PeerInfoFormat {
    NotCompact = 0,
    Compact = 1,
}
