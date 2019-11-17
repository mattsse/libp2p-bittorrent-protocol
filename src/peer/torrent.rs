use std::collections::VecDeque;
use std::convert::TryInto;
use std::path::PathBuf;
use std::time::Duration;

use bytes::BytesMut;
use fnv::FnvHashMap;
use libp2p_core::PeerId;
use wasm_timer::Instant;

use crate::util::ShaHash;

use crate::behavior::{
    BitTorrentConfig,
    BlockErr,
    HandshakeError,
    HandshakeOk,
    HandshakeResult,
    InterestOk,
    InterestResult,
    KeepAliveOk,
    KeepAliveResult,
    PeerError,
    SeedLeechConfig,
};
use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::disk::message::DiskMessageIn;
use crate::handler::BitTorrentRequestId;
use crate::peer::piece::{NextBlock, TorrentPieceHandler, TorrentPieceHandlerState};
use crate::peer::{BttPeer, ChokeType, InterestType};
use crate::piece::{Piece, PieceSelection};
use crate::proto::message::{Handshake, PeerRequest};

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
    initial_piece_selection_strategy: PieceSelection,
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
    PieceReady(Result<(TorrentId, Block), TorrentError>),
    /// A torrent is finished and remains in the pool for seeding.
    Finished(TorrentId),
    /// the peer we need to send a new KeepAlive msg
    KeepAlive(TorrentId, PeerId),
    /// A remote peer has timed out.
    Timeout(TorrentId, PeerId),
    /// A new block is ready to be downloaded
    NextBlock(NextBlock),
}

impl<TInner> TorrentPool<TInner> {
    /// Create a new pool for the `config`.
    pub fn new(local_peer_id: PeerId, config: BitTorrentConfig) -> Self {
        let local_peer_hash = config.peer_hash.unwrap_or_else(|| ShaHash::random());
        let max_simultaneous_downloads = config
            .max_simultaneous_downloads
            .unwrap_or(BitTorrentConfig::MAX_ACTIVE_TORRENTS);
        let max_active_torrents = config
            .max_active_torrents
            .unwrap_or(BitTorrentConfig::MAX_ACTIVE_TORRENTS);
        Self {
            local_peer_hash,
            local_peer_id,
            torrents: FnvHashMap::default(),
            peer_timeout: Duration::from_secs(120),
            max_simultaneous_downloads,
            max_active_torrents,
            move_completed_downloads: config.move_completed_downloads,
            initial_piece_selection_strategy: config.initial_piece_selection.unwrap_or_default(),
            next_unique_torrent: 0,
        }
    }

    fn add_torrent<T: Into<ShaHash>>(
        &mut self,
        info_hash: T,
        bitfield: BitField,
        piece_length: u64,
        inner: TInner,
        state: TorrentState,
        seed_leech: SeedLeechConfig,
    ) -> Result<(TorrentId, TorrentState), ShaHash> {
        let info_hash = info_hash.into();

        if self.torrents.values().any(|x| x.info_hash == info_hash) {
            return Err(info_hash);
        }

        let state = if state.is_active() {
            if self.iter_active().count() < self.max_active_torrents {
                TorrentState::Active
            } else {
                TorrentState::Paused
            }
        } else {
            state
        };

        let id = TorrentId(self.next_unique_torrent);
        self.next_unique_torrent += 1;
        let torrent = Torrent::new(
            id,
            info_hash,
            TorrentPieceHandler::new(
                id,
                self.initial_piece_selection_strategy,
                bitfield,
                piece_length,
            ),
            inner,
            seed_leech,
            state,
        );

        self.torrents.insert(id, torrent);
        Ok((id, state))
    }

    /// Creates a new `Torrent` with a new id in
    /// [`SeedLeechConfig::SeedAndLeech`] mode.
    pub fn add_leech<T: Into<ShaHash>>(
        &mut self,
        info_hash: T,
        bitfield: BitField,
        piece_length: u64,
        inner: TInner,
        state: TorrentState,
    ) -> Result<(TorrentId, TorrentState), ShaHash> {
        self.add_torrent(
            info_hash,
            bitfield,
            piece_length,
            inner,
            state,
            SeedLeechConfig::SeedAndLeech,
        )
    }

    pub fn try_start_new_seed<T: Into<ShaHash>>(
        &mut self,
        info_hash: T,
        bitfield: BitField,
        piece_length: u64,
        inner: TInner,
    ) -> Result<Result<TorrentId, TorrentId>, ShaHash> {
        let (id, state) = self.add_seed(
            info_hash,
            bitfield,
            piece_length,
            inner,
            TorrentState::Active,
        )?;
        if state.is_active() {
            Ok(Ok(id))
        } else {
            Ok(Err(id))
        }
    }

    /// Creates a new `Torrent` with a new id in [`SeedLeechConfig::SeedOnly`]
    /// mode.
    pub fn add_seed<T: Into<ShaHash>>(
        &mut self,
        info_hash: T,
        bitfield: BitField,
        piece_length: u64,
        inner: TInner,
        state: TorrentState,
    ) -> Result<(TorrentId, TorrentState), ShaHash> {
        self.add_torrent(
            info_hash,
            bitfield,
            piece_length,
            inner,
            state,
            SeedLeechConfig::SeedOnly,
        )
    }

    pub fn unchoke_remote(&mut self, peer_id: &PeerId) -> Result<TorrentId, ()> {
        for torrent in self.torrents.values_mut() {
            if torrent.unchoke_remote(peer_id).is_some() {
                debug!(
                    "Unchoked remote {:?} for torrent {:?}",
                    peer_id,
                    torrent.id()
                );
                return Ok(torrent.id());
            }
        }
        Err(())
    }

    pub fn choke_remote(&mut self, peer_id: &PeerId) -> Result<TorrentId, ()> {
        for torrent in self.torrents.values_mut() {
            if torrent.choke_remote(peer_id).is_some() {
                return Ok(torrent.id());
            }
        }
        Err(())
    }

    fn iter_active(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents.values().filter(|x| x.is_active())
    }

    fn iter_active_mut(&mut self) -> impl Iterator<Item = &mut Torrent<TInner>> {
        self.torrents.values_mut().filter(|x| x.is_active())
    }

    /// Whether the remote can contribute to the torrent's completion
    pub fn is_remote_interesting(&self, torrent_id: TorrentId, peer_id: &PeerId) -> bool {
        if let Some(torrent) = self.torrents.get(&torrent_id) {
            if torrent.seed_leech().is_leeching() {
                if let Some(peer) = torrent.piece_handler.get_peer(peer_id) {
                    return peer.is_having_missing_pieces_for(torrent.bitfield());
                }
            }
        }
        false
    }

    /// Sets the client's interest in a peer for a torrent
    pub fn set_client_interest(
        &mut self,
        torrent_id: TorrentId,
        peer_id: &PeerId,
        interest: InterestType,
    ) -> Option<InterestType> {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            return torrent.set_client_interest(peer_id, interest);
        }
        None
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
    /// If the `Handshakes`' info_hash could not be found the in pool or the
    /// peer is already associated with a torrent a `None` value is returned.
    pub fn on_handshake_request(
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
            torrent.piece_handler.insert_peer(peer_id, peer);
            return Ok(Handshake::new(handshake.info_hash, self.local_peer_hash));
        }
        Err(HandshakeError::InfoHashMismatch(peer_id, None))
    }

    ///  Event handling for a [`BitTorrentHandlerEvent::HandshakeRes`] sent by
    /// the remote.
    ///
    /// If we still the torrent identified with the `torrent_id`, we begin to
    /// track the peer
    pub fn on_handshake_response(
        &mut self,
        peer_id: PeerId,
        torrent_id: TorrentId,
        handshake: &Handshake,
    ) -> Result<(HandshakeOk, BitField), HandshakeError> {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            if torrent.info_hash == handshake.info_hash {
                let peer = BttPeer::new(handshake.peer_id);
                torrent.piece_handler.insert_peer(peer_id.clone(), peer);
                return Ok((HandshakeOk(peer_id), torrent.bitfield().clone()));
            }
            return Err(HandshakeError::InfoHashMismatch(peer_id, Some(torrent_id)));
        }
        Err(HandshakeError::InvalidPeer(peer_id, Some(torrent_id)))
    }

    pub fn on_choke_by_remote(
        &mut self,
        peer_id: &PeerId,
        choke: ChokeType,
    ) -> Option<(TorrentId, ChokeType)> {
        for torrent in self.torrents.values_mut() {
            if let Some(choke) = torrent.on_choke_by_remote(peer_id, choke) {
                return Some((torrent.id(), choke));
            }
        }
        None
    }

    /// Update the peer's latest heartbeat timestamp.
    pub fn on_keep_alive_by_remote(
        &mut self,
        peer: PeerId,
        new_heartbeat: Instant,
    ) -> KeepAliveResult {
        for torrent in self.torrents.values_mut() {
            if let Some(old_heartbeat) = torrent.on_keep_alive_by_remote(&peer, &new_heartbeat) {
                return Ok(KeepAliveOk::Remote {
                    torrent: torrent.id(),
                    peer,
                    old_heartbeat,
                    new_heartbeat,
                });
            }
        }
        Err(PeerError::NotFound(peer))
    }

    /// Event handling for a [`PeerMessage::Interested`] or
    /// [`PeerMessage::NotInterested`] sent by the remote.
    ///
    /// Sets the state of the corresponding `BttPeer` accordingly.
    pub fn on_interest_by_remote(
        &mut self,
        peer_id: PeerId,
        interest: InterestType,
    ) -> InterestResult {
        for torrent in self.torrents.values_mut() {
            if let Some(_) = match &interest {
                InterestType::NotInterested => torrent.on_remote_not_interested(&peer_id),
                InterestType::Interested => torrent.on_remote_interested(&peer_id),
            } {
                return match interest {
                    InterestType::Interested => Ok(InterestOk::Interested(peer_id)),
                    InterestType::NotInterested => Ok(InterestOk::NotInterested(peer_id)),
                };
            }
        }
        Err(PeerError::NotFound(peer_id))
    }

    /// Event handling for a [`BitTorrentHandlerEvent::GetPieceReq`].
    ///
    /// Remote want's to leech a block.
    /// We check if the remote is currently tracked and the remote is interested
    /// and not choked and that we own the requested block. If Not a `None`
    /// value is returned.
    pub fn on_piece_request(
        &mut self,
        peer_id: PeerId,
        metadata: BlockMetadata,
        request_id: BitTorrentRequestId,
    ) -> Result<(TorrentId, BlockMetadata), (PeerId, BlockMetadata, BitTorrentRequestId)> {
        for torrent in self.torrents.values_mut() {
            if torrent.can_seed_piece_to(&peer_id, metadata.piece_index as usize) {
                torrent
                    .piece_handler
                    .insert_block_seed(peer_id, metadata.clone(), request_id);
                return Ok((torrent.id, metadata));
            }
        }
        Err((peer_id, metadata, request_id))
    }

    /// Event handling for a [`BitTorrentHandlerEvent::GetPieceRes`].
    pub fn on_block_response(
        &mut self,
        torrent_id: TorrentId,
        peer_id: &PeerId,
        block: Block,
    ) -> Result<BlockMetadata, BlockErr> {
        if let Some(torrent) = self.get_mut(&torrent_id) {
            return torrent.add_block(peer_id, block);
        }
        Err(BlockErr::NotRequested {
            peer: peer_id.clone(),
            block,
        })
    }

    pub fn on_cancel_request(
        &mut self,
        id: BitTorrentRequestId,
    ) -> Option<(PeerId, BlockMetadata)> {
        for torrent in self.torrents.values_mut() {
            if let Some((peer, block)) = torrent.piece_handler.remove_pending_seed_by_request(&id) {
                return Some((peer, block));
            }
        }
        None
    }

    pub fn on_block_read(
        &mut self,
        torrent_id: TorrentId,
        block: &BlockMetadata,
    ) -> Option<(PeerId, BitTorrentRequestId)> {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            if let Some((peer, id)) = torrent.piece_handler.remove_pending_seed(block) {
                return Some((peer, id));
            }
        }
        None
    }

    /// Event handling for a [`BitTorrentHandlerEvent::Have`].
    ///
    /// Update the peer's bitfield.
    /// If the peer was not found a `None` value is returned, otherwise if the
    /// `piece_index` was in bounds.
    pub fn on_remote_have(&mut self, peer_id: &PeerId, piece_index: usize) -> Option<bool> {
        for torrent in self.torrents.values_mut() {
            if let Some(bit) = torrent.on_remote_have(peer_id, piece_index) {
                return Some(bit);
            }
        }
        None
    }

    /// Set the `Bitfield` for the peer associated with the torrent
    ///
    /// Returns `true` if the associated peer was found, `false` otherwise
    pub fn set_peer_bitfield(
        &mut self,
        peer_id: &PeerId,
        torrent_id: TorrentId,
        bitfield: BitField,
    ) -> bool {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            if let Some(peer) = torrent.piece_handler.get_peer_mut(peer_id) {
                peer.set_bitfield(bitfield);
                return true;
            }
        }
        return false;
    }

    /// Returns the bitfield for the torrent the peer is currently tracked on
    ///
    /// A `None` should result in dropping the connection
    pub fn on_bitfield_request(
        &mut self,
        peer_id: &PeerId,
        bitfield: BitField,
    ) -> Option<BitField> {
        for torrent in self.torrents.values_mut() {
            if let Some(peer) = torrent.piece_handler.get_peer_mut(peer_id) {
                peer.set_bitfield(bitfield);
                return Some(torrent.piece_handler.bitfield().clone());
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

    /// returns all peers that are currently not related to the info hash
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
            .filter(|torrent| torrent.seed_leech != SeedLeechConfig::SeedOnly)
    }

    pub fn iter_leeching_from(&self) -> impl Iterator<Item = &PeerId> {
        self.iter_leeching().flat_map(Torrent::iter_peer_ids)
    }

    /// Returns all torrents we are seeding.
    pub fn iter_seeding(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|torrent| torrent.seed_leech != SeedLeechConfig::LeechOnly)
    }

    pub fn iter_seeding_to(&self) -> impl Iterator<Item = &PeerId> {
        self.iter_seeding().flat_map(Torrent::iter_peer_ids)
    }

    /// Returns all torrents we own completely.
    pub fn iter_complete_seeds(&self) -> impl Iterator<Item = &Torrent<TInner>> {
        self.torrents
            .values()
            .filter(|torrent| torrent.seed_leech == SeedLeechConfig::SeedOnly)
    }

    /// Returns a mutablereference to a torrent with the given ID, if it is in
    /// the pool.
    pub fn get_mut(&mut self, id: &TorrentId) -> Option<&mut Torrent<TInner>> {
        self.torrents.get_mut(id)
    }

    pub fn poll(&mut self, now: Instant) -> TorrentPoolState<TInner> {
        for torrent in self.iter_active_mut() {
            match torrent.poll(now) {
                TorrentPoolState::Idle => continue,
                TorrentPoolState::Waiting(_) => continue,
                ready => return ready,
            }
        }
        TorrentPoolState::Idle
    }
}

/// a Torrent in a `TorrentPool`
pub struct Torrent<TInner> {
    /// The unique ID of the Torrent.
    id: TorrentId,
    /// the info hash of the torrent file, this is how torrents are identified
    pub info_hash: ShaHash,
    /// The peer iterator that drives the torrent's piece state.
    piece_handler: TorrentPieceHandler,
    /// The instant when the torrent was created.
    created: Instant,
    /// The opaque inner state.
    pub inner: TInner,
    /// The state of how the torrent should send/receive blocks
    seed_leech: SeedLeechConfig,
    /// State of torrent's capability to download/upload
    state: TorrentState,
}

impl<TInner> Torrent<TInner> {
    // TODO add Builder
    pub fn new(
        id: TorrentId,
        info_hash: ShaHash,
        piece_handler: TorrentPieceHandler,
        inner: TInner,
        seed_leech: SeedLeechConfig,
        state: TorrentState,
    ) -> Self {
        Self {
            id,
            info_hash,
            piece_handler,
            created: Instant::now(),
            inner,
            seed_leech,
            state,
        }
    }

    /// Gets the unique ID of the torrent.
    pub fn id(&self) -> TorrentId {
        self.id
    }

    pub fn iter_peer_ids(&self) -> impl Iterator<Item = &PeerId> {
        self.piece_handler.peers.keys()
    }

    pub fn iter_peers(&self) -> impl Iterator<Item = &BttPeer> {
        self.piece_handler.peers.values()
    }

    pub fn remove_peer(&mut self, peer_id: &PeerId) -> Option<BttPeer> {
        self.piece_handler.remove_peer(peer_id)
    }

    pub fn add_block(&mut self, peer_id: &PeerId, block: Block) -> Result<BlockMetadata, BlockErr> {
        self.piece_handler.add_block(peer_id, block)
    }

    /// The bitfield of the client.
    pub fn bitfield(&self) -> &BitField {
        self.piece_handler.bitfield()
    }

    pub fn poll(&mut self, now: Instant) -> TorrentPoolState<TInner> {
        for (id, peer) in &mut self.piece_handler.peers {
            // TODO only consider active peers
            if peer.is_client_timeout(now) {
                peer.client_heartbeat = Instant::now();
                return TorrentPoolState::KeepAlive(self.id, id.clone());
            }
            if peer.is_remote_timeout(now) {
                return TorrentPoolState::Timeout(self.id, id.clone());
            }
        }
        match self.piece_handler.poll() {
            TorrentPieceHandlerState::WaitingPeers => TorrentPoolState::Waiting(self),
            TorrentPieceHandlerState::LeechBlock(block) => {
                if let Some(block) = block {
                    TorrentPoolState::NextBlock(block)
                } else {
                    TorrentPoolState::Waiting(self)
                }
            }
            TorrentPieceHandlerState::PieceFinished(res) => {
                TorrentPoolState::PieceReady(res.map(|x| (self.id(), x)))
            }
            TorrentPieceHandlerState::CompleteSeed => {
                if self.seed_leech.is_leeching() {
                    self.seed_leech = SeedLeechConfig::SeedOnly;
                    TorrentPoolState::Finished(self.id())
                } else {
                    TorrentPoolState::Idle
                }
            }
        }
    }

    /// Update the peer's latest heartbeat timestamp.
    pub fn on_keep_alive_by_remote(
        &mut self,
        peer_id: &PeerId,
        heartbeat: &Instant,
    ) -> Option<Instant> {
        if let Some(peer) = self.piece_handler.get_peer_mut(peer_id) {
            Some(std::mem::replace(
                &mut peer.remote_heartbeat,
                heartbeat.to_owned(),
            ))
        } else {
            None
        }
    }

    /// Update the client's latest heartbeat timestamp.
    pub fn on_keep_alive_by_client(
        &mut self,
        peer_id: &PeerId,
        heartbeat: &Instant,
    ) -> Option<Instant> {
        if let Some(peer) = self.piece_handler.get_peer_mut(peer_id) {
            Some(std::mem::replace(
                &mut peer.client_heartbeat,
                heartbeat.to_owned(),
            ))
        } else {
            None
        }
    }

    pub fn on_choke_by_remote(&mut self, peer_id: &PeerId, choke: ChokeType) -> Option<ChokeType> {
        match choke {
            ChokeType::Choked => self.on_choked_by_remote(peer_id),
            ChokeType::UnChoked => self.on_unchoked_by_remote(peer_id),
        }
    }

    pub fn on_choked_by_remote(&mut self, peer_id: &PeerId) -> Option<ChokeType> {
        self.piece_handler
            .set_choke_on_remote(peer_id, ChokeType::Choked)
    }

    pub fn on_unchoked_by_remote(&mut self, peer_id: &PeerId) -> Option<ChokeType> {
        self.piece_handler
            .set_choke_on_remote(peer_id, ChokeType::UnChoked)
    }

    pub fn on_remote_interested(&mut self, peer_id: &PeerId) -> Option<InterestType> {
        self.piece_handler
            .set_interest_on_remote(peer_id, InterestType::Interested)
    }

    pub fn on_remote_not_interested(&mut self, peer_id: &PeerId) -> Option<InterestType> {
        self.piece_handler
            .set_interest_on_remote(peer_id, InterestType::NotInterested)
    }

    pub fn set_client_interest(
        &mut self,
        peer_id: &PeerId,
        interest: InterestType,
    ) -> Option<InterestType> {
        self.piece_handler.set_client_interest(peer_id, interest)
    }

    fn set_choke_for_remote(&mut self, peer: &PeerId, choke: ChokeType) -> Option<ChokeType> {
        if let Some(peer) = self.piece_handler.peers.get_mut(peer) {
            Some(std::mem::replace(&mut peer.client_choke, choke))
        } else {
            None
        }
    }

    pub fn unchoke_remote(&mut self, peer: &PeerId) -> Option<ChokeType> {
        self.set_choke_for_remote(peer, ChokeType::UnChoked)
    }

    pub fn choke_remote(&mut self, peer: &PeerId) -> Option<ChokeType> {
        self.set_choke_for_remote(peer, ChokeType::Choked)
    }

    /// Update the peer's bitfield.
    ///
    /// If the peer is not tracked, a `None` value is returned.
    /// Otherwise if the `piece_index` was in bounds.
    pub fn on_remote_have(&mut self, peer_id: &PeerId, piece_index: usize) -> Option<bool> {
        // TODO check current buffer for the new piece
        if let Some(peer) = self.piece_handler.get_peer_mut(peer_id) {
            peer.add_piece(piece_index)
        } else {
            None
        }
    }

    /// Whether the client can seed a piece the peer.
    ///
    /// Seeding a piece to the remote is possible, if the remote is interested
    /// and not choked by the client. Also the client has to own the piece in
    /// the first place and no outstanding requests
    pub fn can_seed_piece_to(&self, peer_id: &PeerId, piece_index: usize) -> bool {
        if let Some(peer) = self.piece_handler.get_peer(peer_id) {
            self.piece_handler.has_piece(piece_index)
                && peer.remote_can_leech()
                && self.piece_handler.is_ready_to_seed_block(peer_id)
        } else {
            false
        }
    }

    /// Whether the remote can seed a specific piece.
    ///
    /// Leeching the piece from the remote is possible if the remote owns that
    /// piece, the client is interested to download and currently not choked by
    /// the remote.
    pub fn can_leech_piece_from(&self, peer_id: &PeerId, piece_index: usize) -> bool {
        if let Some(peer) = self.piece_handler.get_peer(peer_id) {
            peer.remote_can_seed_piece(piece_index)
        } else {
            false
        }
    }

    /// If the peer is currently tracked by this torrent.
    pub fn contains_peer(&self, peer_id: &PeerId) -> bool {
        self.piece_handler.peers.contains_key(peer_id)
    }

    pub fn seed_leech(&self) -> &SeedLeechConfig {
        &self.seed_leech
    }

    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }
    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    pub fn is_stopped(&self) -> bool {
        self.state.is_stopped()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentState {
    Active,
    Paused,
    Stopped,
}

impl TorrentState {
    pub fn is_active(&self) -> bool {
        match self {
            TorrentState::Active => true,
            _ => false,
        }
    }
    pub fn is_paused(&self) -> bool {
        match self {
            TorrentState::Paused => true,
            _ => false,
        }
    }

    pub fn is_stopped(&self) -> bool {
        match self {
            TorrentState::Stopped => true,
            _ => false,
        }
    }
}

impl Default for TorrentState {
    fn default() -> Self {
        TorrentState::Paused
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
