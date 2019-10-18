use crate::bitfield::BitField;
use chrono::NaiveDate;
use sha1::Sha1;

#[derive(Clone, Eq, PartialEq)]
pub struct Torrent {
    /// urls of the trackers
    pub trackers: Vec<String>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub encoding: Option<String>,
    pub info: TorrentInfo,
    pub nodes: Vec<(String, u32)>,
    /// the hash that identifies this torrent
    pub info_hash: Sha1,
}
#[derive(Clone, Eq, PartialEq)]
pub enum TorrentInfo {
    MerkleTree,
    Dict(Info),
}

#[derive(Clone, Eq, PartialEq)]
pub struct Info {
    pub piece_hashes: Vec<Sha1>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<NaiveDate>,
}

/// TODO convert to bitflag
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum TorrentType {
    MultiFile,
    Private,
    I2P,
    SSL,
}

#[derive(Clone, Eq, PartialEq)]
pub struct MerkleTree {}
