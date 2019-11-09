use std::collections::VecDeque;
use std::convert::TryInto;
use std::path::PathBuf;
use std::time::Duration;

use bytes::BytesMut;
use fnv::FnvHashMap;
use libp2p_core::PeerId;
use wasm_timer::Instant;

use crate::behavior::{
    BittorrentConfig, HandshakeError, HandshakeOk, HandshakeResult, SeedLeechConfig,
};
use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::peer::piece::TorrentPieceHandler;
use crate::peer::BttPeer;
use crate::piece::PieceSelection;
use crate::proto::message::Handshake;
use crate::util::ShaHash;

/// A `TorrentPool` provides an aggregate state machine for driving the
/// Torrent#s `Piece`s to completion.
///
/// Internally, a `TorrentInner` is in turn driven by an underlying
/// `TorrentPeerIter` that determines the peer selection strategy.
pub struct TorrentPool<TInner> {
    pub local_peer_hash: ShaHash,
    pub local_peer_id: PeerId,
    torrents: FnvHashMap<TorrentId, Torrent<TInner>>,
    /// The timeout after we choke a peer
    peer_timeout: Duration,
    /// Cannot be higher than the active torrents
    max_simultaneous_downloads: usize,
    /// New torrents won't start if more are seeded/leeched
    max_active_torrents: usize,
    /// Move finished downloads to another folder
    move_completed_downloads: Option<PathBuf>,
    /// How the initial pieces to download are selected
    initial_piece_selection: PieceSelection,
    /// Next unique Identifier of a torrent.
    next_unique_torrent: usize,
}

/// The observable states emitted by [`TorrentPool::poll`].
pub enum TorrentPoolState<'a, TInner> {
    /// The pool is idle, i.e. there are no torrents to process.
    Idle,
    /// At least one torrent is waiting to generate requests.
    Waiting(&'a mut Torrent<TInner>),
    /// A leeched piece is ready to process
    PieceReady(TorrentId, Block),
    /// A torrent is finished and remains in the pool for seeding.
    Finished(TorrentId),
    /// A torrent has finished.
    Removed(Torrent<TInner>),
    /// A torrent was added.
    Added(TorrentId),
    /// the peer we need to send a new KeepAlive msg
    KeepAlive(PeerId),
    /// A remote peer has timed out.
    Timeout(PeerId),
}

impl<TInner> TorrentPool<TInner> {
    pub fn new(local_peer_id: PeerId, config: BittorrentConfig) -> Self {
        let local_peer_hash = config.peer_hash.unwrap_or_else(|| ShaHash::random());
        let max_simultaneous_downloads = config
            .max_simultaneous_downloads
            .unwrap_or(BittorrentConfig::MAX_ACTIVE_TORRENTS);
        let max_active_torrents = config
            .max_active_torrents
            .unwrap_or(BittorrentConfig::MAX_ACTIVE_TORRENTS);
        Self {
            local_peer_hash,
            local_peer_id,
            torrents: FnvHashMap::default(),
            peer_timeout: Duration::from_secs(120),
            max_simultaneous_downloads,
            max_active_torrents,
            move_completed_downloads: config.move_completed_downloads,
            initial_piece_selection: config.initial_piece_selection.unwrap_or_default(),
            next_unique_torrent: 0,
        }
    }

    /// The torrent that matches the hash
    pub fn get_for_info_hash(&self, info_hash: &ShaHash) -> Option<&Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|x| x.info_hash == *info_hash)
            .next()
    }

    /// The torrent that matches the hash
    pub fn get_for_info_hash_mut(&mut self, info_hash: &ShaHash) -> Option<&mut Torrent<TInner>> {
        self.torrents
            .values_mut()
            .filter(|x| x.info_hash == *info_hash)
            .next()
    }

    /// Whether the peer is currently associated with a torrent
    pub fn is_associated(&self, peer_id: &PeerId) -> bool {
        for torrent in self.torrents.values() {
            if torrent.contains_peer(peer_id) {
                return true;
            }
        }
        false
    }

    /// Creates the response `HandShake` for a specific torrent
    ///
    /// If the `HandShakes`' info_hash could not be found the in pool or the
    /// peer is already associated with a torrent a `None` value is returned.
    pub fn create_handshake_response(
        &mut self,
        peer_id: PeerId,
        handshake: Handshake,
    ) -> Result<Handshake, HandshakeError> {
        if self.is_associated(&peer_id) {
            // peer is already tracked with a torrent
            return Err(HandshakeError::InvalidPeer(peer_id, None));
        }
        if let Some(torrent) = self.get_for_info_hash_mut(&handshake.info_hash) {
            let peer = BttPeer::new(handshake.peer_id);
            // begin tracking the peer
            torrent.peer_iter.insert_peer(peer_id, peer);
            return Ok(Handshake::new(handshake.info_hash, self.local_peer_hash));
        }
        Err(HandshakeError::InfoHashMismatch(peer_id, None))
    }

    pub fn on_handshake(
        &mut self,
        peer_id: PeerId,
        torrent_id: TorrentId,
        handshake: &Handshake,
    ) -> HandshakeResult {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            if torrent.info_hash == handshake.info_hash {
                let peer = BttPeer::new(handshake.peer_id);
                torrent.peer_iter.insert_peer(peer_id.clone(), peer);
                return Ok(HandshakeOk(peer_id));
            }
            return Err(HandshakeError::InfoHashMismatch(peer_id, Some(torrent_id)));
        }
        Err(HandshakeError::InvalidPeer(peer_id, Some(torrent_id)))
    }

    pub fn on_keep_alive(&mut self, peer_id: &PeerId, instant: Instant) {}

    pub fn on_choked(&mut self, peer_id: &PeerId) {}

    pub fn on_unchoked(&mut self, peer_id: &PeerId) {}

    pub fn on_have(&mut self, peer_id: &PeerId, piece_index: u64) {}

    /// Set the `Bitfield` for the peer associated with the torrent
    ///
    /// Returns `true` if the associated peer was found, `false` otherwise
    pub fn set_bitfield(
        &mut self,
        peer_id: &PeerId,
        torrent_id: TorrentId,
        bitfield: BitField,
    ) -> bool {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            if let Some(peer) = torrent.peer_iter.get_peer_mut(peer_id) {
                peer.set_bitfield(bitfield);
                return true;
            }
        }
        return false;
    }

    /// Returns the bitfield for the torrent the peer is currently tracked on
    ///
    /// A `None` should result in dropping the connection
    pub fn get_bitfield(&self, peer_id: &PeerId) -> Option<BitField> {
        for torrent in self.torrents.values() {
            if torrent.peer_iter.peers.contains_key(peer_id) {
                return Some(torrent.peer_iter.bitfield().clone());
            }
        }
        None
    }

    /// Drops the first peer from the pool that matches the id
    ///
    /// This is valid since only a single connection per torrent and peer is
    /// allowed
    pub fn remove_peer(&mut self, peer_id: &PeerId) -> Option<BttPeer> {
        for torrent in self.torrents.values_mut() {
            if let Some(peer) = torrent.remove_peer(peer_id) {
                return Some(peer);
            }
        }
        None
    }

    /// Returns an iterator over the torrents in the pool.
    pub fn iter(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents.values()
    }

    /// Returns an iterator over mutable torrents in the pool.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Torrent<TInner>> {
        self.torrents.values_mut()
    }

    /// returns all peers that are currently related to the torrents hash
    pub fn iter_active_peers<'a>(
        &'a self,
        info_hash: &'a ShaHash,
    ) -> impl Iterator<Item = &'a PeerId> + 'a {
        self.torrents
            .values()
            .filter(move |torrent| torrent.info_hash == *info_hash)
            .flat_map(Torrent::iter_peer_ids)
    }

    /// returns all peers that are currently not related to the info hash
    // TODO should changed to a lookup in DHT
    pub fn iter_candidate_peers<'a>(
        &'a self,
        info_hash: &'a ShaHash,
    ) -> impl Iterator<Item = &'a BttPeer> + 'a {
        self.torrents
            .values()
            .filter(move |torrent| torrent.info_hash != *info_hash)
            .flat_map(Torrent::iter_peers)
    }

    pub fn iter_candidate_peer_ids<'a>(
        &'a self,
        info_hash: &'a ShaHash,
    ) -> impl Iterator<Item = &'a PeerId> + 'a {
        self.torrents
            .values()
            .filter(move |torrent| torrent.info_hash != *info_hash)
            .flat_map(Torrent::iter_peer_ids)
    }

    /// Returns a reference to a torrent with the given ID, if it is in the
    /// pool.
    pub fn get(&self, id: &TorrentId) -> Option<&Torrent<TInner>> {
        self.torrents.get(id)
    }

    /// Returns all torrents in leeching state
    pub fn iter_leeching(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|torrent| torrent.state != SeedLeechConfig::Seed)
    }

    pub fn iter_leeching_from(&self) -> impl Iterator<Item = &PeerId> {
        self.iter_leeching().flat_map(Torrent::iter_peer_ids)
    }

    /// Returns all torrents we are seeding.
    pub fn iter_seeding(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|torrent| torrent.state != SeedLeechConfig::Leech)
    }

    pub fn iter_seeding_to(&self) -> impl Iterator<Item = &PeerId> {
        self.iter_seeding().flat_map(Torrent::iter_peer_ids)
    }

    /// Returns all torrents we own completely.
    pub fn iter_complete_seeds(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|torrent| torrent.state == SeedLeechConfig::Seed)
    }

    /// Returns a mutablereference to a torrent with the given ID, if it is in
    /// the pool.
    pub fn get_mut(&mut self, id: &TorrentId) -> Option<&mut Torrent<TInner>> {
        self.torrents.get_mut(id)
    }

    pub fn poll(&mut self, now: Instant) -> TorrentPoolState<TInner> {
        unimplemented!()
    }
}

/// a Torrent in a `TorrentPool`
pub struct Torrent<TInner> {
    /// The unique ID of the Torrent.
    id: TorrentId,
    /// the info hash of the torrent file, this is how torrents are identified
    pub info_hash: ShaHash,
    /// The peer iterator that drives the torrent's piece state.
    peer_iter: TorrentPieceHandler,
    /// The instant when the torrent started (i.e. began waiting for the first
    /// result from a peer).
    started: Instant,
    /// The opaque inner piece state.
    pub inner: TInner,
    /// the state of the torrent
    pub state: SeedLeechConfig,
}

impl<TInner> Torrent<TInner> {
    /// Gets the unique ID of the torrent.
    pub fn id(&self) -> TorrentId {
        self.id
    }

    pub fn iter_peer_ids(&self) -> impl Iterator<Item = &PeerId> {
        self.peer_iter.peers.keys()
    }

    pub fn iter_peers(&self) -> impl Iterator<Item = &BttPeer> {
        self.peer_iter.peers.values()
    }

    pub fn remove_peer(&mut self, peer_id: &PeerId) -> Option<BttPeer> {
        self.peer_iter.remove_peer(peer_id)
    }

    pub fn poll(&mut self, now: Instant) -> TorrentPoolState<TInner> {
        unimplemented!()
    }

    pub fn contains_peer(&self, peer_id: &PeerId) -> bool {
        self.peer_iter.peers.contains_key(peer_id)
    }
}

/// Unique identifier for an active Torrent operation.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct TorrentId(pub usize);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TorrentPeer {
    /// the libp2p identifier
    pub peer_id: PeerId,
    /// the bittorrent id, necessary to for handshaking
    pub torrent_id: TorrentId,
}
