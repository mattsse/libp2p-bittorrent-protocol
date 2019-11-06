pub mod piece;
use crate::bitfield::BitField;
use crate::util::ShaHash;
use libp2p_core::PeerId;
use wasm_timer::Instant;

/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChokeType {
    /// remote has chocked the client
    /// When a peer chokes the client, it is a notification that no requests will be answered until the client is unchoked.
    Choked = 0,
    /// client currently accepts request
    UnChoked = 1,
}

impl Default for ChokeType {
    fn default() -> Self {
        ChokeType::Choked
    }
}

/// Status of our interest to download a target by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InterestType {
    /// remote has no interest to download.
    NotInterested = 0,
    /// remote is currently interested to download.
    Interested = 1,
}

impl Default for InterestType {
    fn default() -> Self {
        InterestType::NotInterested
    }
}

/// connection start out as chocked and not interested
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeer {
    /// the peer's bittorrent id
    pub peer_id: ShaHash,
    /// what pieces this peers owns or lacks
    pub piece_field: Option<BitField>,
    /// history of send/receive statistics
    pub stats: PeerStats,
    /// How the remote treats requests made by the client.
    pub choke_ty: ChokeType,
    /// whether the remote is interested in the content
    pub interest_ty: InterestType,
    /// timestamp for the last keepalive msg
    pub keep_alive: Instant,
}

impl BttPeer {
    pub fn new<T: Into<ShaHash>>(peer_id: T, piece_field: BitField) -> Self {
        Self {
            piece_field: Some(piece_field),
            peer_id: peer_id.into(),
            stats: Default::default(),
            choke_ty: Default::default(),
            interest_ty: Default::default(),
            keep_alive: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    pub received_good_blocks: usize,
    pub received_bad_blocks: usize,
    pub send_blocks: usize,
}

/// peer's IP address either IPv6 (hexed) or IPv4 (dotted quad) or DNS name (string)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIp {
    Ip4,
    Ip6,
    DNS(String),
}
