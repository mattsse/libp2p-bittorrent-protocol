use crate::behavior::SeedLeechConfig;
use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::peer::BttPeer;
use crate::piece::PieceSelection;
use crate::util::ShaHash;
use bytes::BytesMut;
use fnv::FnvHashMap;
use libp2p_core::PeerId;
use std::collections::VecDeque;
use std::convert::TryInto;
use std::time::Duration;
use wasm_timer::Instant;

/// A `TorrentPool` provides an aggregate state machine for driving the Torrent#s `Piece`s to completion.
///
/// Internally, a `TorrentInner` is in turn driven by an underlying `TorrentPeerIter`
/// that determines the peer selection strategy.
pub struct TorrentPool<TInner> {
    pub local_peer_hash: ShaHash,
    pub local_peer_id: PeerId,
    torrents: FnvHashMap<TorrentId, Torrent<TInner>>,
    timeout: Duration,
}

/// The observable states emitted by [`TorrentPool::poll`].
pub enum TorrentPoolState<'a, TInner> {
    /// The pool is idle, i.e. there are no torrents to process.
    Idle,
    /// At least one torrent is waiting for results. `Some(request)` indicates
    /// that a new request is now being waited on.
    Waiting(Option<(&'a mut Torrent<TInner>, PeerId)>),
    /// A block is ready to process
    BlockReady((TorrentId, BlockBuffer)),
    /// A torrent has finished.
    Finished(Torrent<TInner>),
    /// the peer we need to send a new KeepAlive msg
    KeepAlive(PeerId),
    /// A remote peer has timed out.
    Timeout(PeerId),
}

impl<TInner> TorrentPool<TInner> {
    /// Returns an iterator over the pieces in the pool.
    pub fn iter(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents.values()
    }

    pub fn new<T: Into<ShaHash>>(local_id: T, local_peer_id: PeerId) -> Self {
        Self {
            local_peer_id,
            local_peer_hash: local_id.into(),
            torrents: FnvHashMap::default(),
            timeout: Duration::from_secs(120),
        }
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

    /// Returns a reference to a torrent with the given ID, if it is in the pool.
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

    /// Returns a mutablereference to a torrent with the given ID, if it is in the pool.
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
    /// the downloaded blocks for a torrent
    block_buffer: BlockBuffer,
    /// the info hash of the torrent file, this is how torrents are identified
    pub info_hash: ShaHash,
    /// The peer iterator that drives the torrent's piece state.
    // TODO this should be piece handler
    peer_iter: TorrentPeerIter,
    /// The instant when the torrent started (i.e. began waiting for the first
    /// result from a peer).
    started: Instant,
    /// The opaque inner piece state.
    pub inner: TInner,
    /// the state of the torrent
    pub state: SeedLeechConfig,
}

impl<TInner> Torrent<TInner> {
    pub fn iter_peer_ids(&self) -> impl Iterator<Item = &PeerId> {
        self.peer_iter.peers.keys()
    }

    pub fn iter_peers(&self) -> impl Iterator<Item = &BttPeer> {
        self.peer_iter.peers.values()
    }

    pub fn poll(&mut self, now: Instant) -> TorrentPoolState<TInner> {
        unimplemented!()
    }
}

/// tracks the state of a single torrent with all their remotes
pub struct TorrentPeerIter {
    /// how the next piece is selected
    strategy: PieceSelection,
    /// which pieces the client owns and lacks
    have: BitField,
    /// The ownership state of peers.
    // TODO only peer-id is necessary to identify the peer
    peers: FnvHashMap<PeerId, BttPeer>,
    /// whether client is in endgame mode
    endgame: bool,
}

impl TorrentPeerIter {
    /// Advances the state of the torrent's pieces.
    pub fn poll(&mut self, now: Instant) {
        unimplemented!()
    }
}

/// Unique identifier for an active Torrent operation.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct TorrentId(usize);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TorrentPeer {
    /// the libp2p identifier
    pub peer_id: PeerId,
    /// the bittorrent id, necessary to for handshaking
    pub peer_hash: ShaHash,
}

pub struct BlockBuffer {
    /// torrent this belongs to
    pub torrent_id: TorrentId,
    /// the index of the piece this block buffers
    pub piece_index: u64,
    /// all the data, not necessarily in correct order
    blocks: Vec<BlockMut>,
    /// whether all blocks consecutively
    aligned: bool,
    /// the length of the piece all blocks belong to
    piece_length: usize,
}

impl BlockBuffer {
    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.aligned
    }

    pub fn align(&mut self) -> bool {
        if self.aligned {
            true
        } else {
            // TODO order the blocks by offset
            false
        }
    }

    #[inline]
    pub fn is_full_piece(&self) -> bool {
        // if we own all pieces in that block
        unimplemented!()
    }
}

impl TryInto<Block> for BlockBuffer {
    type Error = TorrentError;

    fn try_into(mut self) -> Result<Block, Self::Error> {
        if self.align() {
            panic!("Not yet implemented")
        } else {
            Err(TorrentError::BadPiece {
                index: self.piece_index,
            })
        }
    }
}