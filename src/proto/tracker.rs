use crate::proto::BttPeer;
use sha1::Sha1;
use std::convert::TryInto;
use std::io;
use std::ops::Add;
use std::{convert::TryFrom, time::Duration};
use wasm_timer::Instant;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BttEventType {
    Started = 1,
    Completed = 2,
    Stopped = 3,
}

#[derive(Clone, PartialEq, Eq)]
pub struct BttTrackerRequestMsg {
    pub info_hash: Sha1,

    pub peer: BttPeer,

    pub uploaded: u64,

    pub downloaded: u64,

    pub left: u64,

    pub numwant: u32,

    pub event: Option<BttEventType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BttTrackerResponseMsg {
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
