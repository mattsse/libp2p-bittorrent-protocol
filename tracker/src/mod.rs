use libp2p::PeerId;

pub const SHA_HASH_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerResponse {
    Success {
        /// Number of seconds the downloader should wait between regular rerequests, and peers.
        interval: usize,
        /// The known peers for torrent.
        peers: Vec<PeerResponse>

    },
    Failure {
        /// Why the query failed.
        reason: String
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerResponse {
    /// A string of length 20 which this downloader uses as its id.
    pub identifier: [u8; SHA_HASH_LEN],
    /// The libp2p identifier of the peer.
    pub peer_id: PeerId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerRequest {
    /// The 20 byte sha1 hash of the bencoded form of the info value from the
    /// metainfo file.
    pub info_hash: [u8; SHA_HASH_LEN],
    /// A string of length 20 which this downloader uses as its id.
    pub identifier: [u8; SHA_HASH_LEN],
    /// The libp2p identifier of the peer.
    pub peer_id: PeerId,
    /// The total amount uploaded so far.
    pub uploaded: usize,
    /// The total amount downloaded so far.
    pub downloaded: usize,
    /// The number of bytes this peer still has to download
    pub left: usize,
    /// An announcement using started is sent when a download first begins, and
    /// one using completed is sent when the download is complete. No completed
    /// is sent if the file was complete when started. Downloaders send an
    /// announcement using stopped when they cease downloading.
    pub event: Option<PeerEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Started,
    Completed,
    Stopped,
}
