use libp2p_core::PeerId;
use wasm_timer::Instant;

use crate::bitfield::BitField;
use crate::util::ShaHash;

pub mod piece;
pub mod torrent;

/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChokeType {
    /// remote has chocked the client
    /// When a peer chokes the client, it is a notification that no requests
    /// will be answered until the client is unchoked.
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
    //    /// Identifier of the peer.
    //    pub node_id: PeerId,
    /// The peer's bittorrent id
    pub peer_id: ShaHash,
    /// What pieces this peers owns or lacks
    pub piece_field: Option<BitField>,
    /// History of send/receive statistics
    pub stats: PeerStats,
    /// How the remote treats requests made by the client.
    pub choke_ty: ChokeType,
    /// Whether the remote is interested in the content
    pub interest_ty: InterestType,
    /// Timestamp for the last keepalive msg
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

    /// The peer currently choked the client
    pub fn is_choked(&self) -> bool {
        self.choke_ty == ChokeType::Choked
    }

    /// The peer currently has the client unchoked
    pub fn is_unchoked(&self) -> bool {
        self.choke_ty == ChokeType::UnChoked
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    pub received_good_blocks: usize,
    pub received_bad_blocks: usize,
    pub send_blocks: usize,
}

/// The state of a single `Peer`.
#[derive(Debug, Copy, Clone)]
enum PeerState {
    /// The peer has not yet been contacted.
    ///
    /// This is the starting state for every peer.
    NotContacted,
    /// The iterator is waiting for a result from the peer.
    Waiting(Instant),

    /// A result was not delivered for the peer within the configured timeout.
    ///
    /// The peer is not taken into account for the termination conditions
    /// of the iterator until and unless it responds.
    Unresponsive,

    /// Obtaining a result from the peer has failed.
    ///
    /// This is a final state, reached as a result of a call to `on_failure`.
    Failed,
    /// A successful result from the peer has been delivered.
    ///
    /// This is a final state, reached as a result of a call to `on_success`.
    Succeeded,
}
