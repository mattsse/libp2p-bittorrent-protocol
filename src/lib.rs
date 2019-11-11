#![allow(unused)]

#[macro_use]
extern crate log;

pub mod behavior;
pub mod bitfield;
pub mod disk;
pub mod error;
pub mod handler;
pub mod peer;
mod piece;
mod proto;

pub mod torrent;
pub mod util;

pub use behavior::{Bittorrent, BittorrentConfig, BittorrentEvent};

pub use behavior::{
    ChokeResult,
    DiskResult,
    HandshakeResult,
    HaveResult,
    InterestResult,
    KeepAliveResult,
    LeechBlockResult,
    SeedBlockResult,
    TorrentAddedResult,
};

pub use torrent::builder::TorrentBuilder;
pub use torrent::MetaInfo;

pub use disk::torrent::TorrentSeed;
