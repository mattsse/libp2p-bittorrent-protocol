//! Implementation of the `Bittorrent` network behaviour.

use crate::peer::BttPeer;
use crate::{
    disk::error::TorrentError,
    disk::fs::FileSystem,
    disk::manager::DiskManager,
    disk::torrent::TorrentSeed,
    handler::BittorrentHandler,
    handler::BittorrentHandlerEvent,
    handler::BittorrentHandlerIn,
    peer::piece::{BlockBuffer, Torrent, TorrentId, TorrentPeer, TorrentPool, TorrentPoolState},
    piece::PieceSelection,
    torrent::MetaInfo,
    util::ShaHash,
};
use bitflags::_core::marker::PhantomData;
use fnv::{FnvHashMap, FnvHashSet};
use futures::{Async, Future};
use libp2p_core::{ConnectedPoint, Multiaddr, PeerId};
use libp2p_swarm::{NetworkBehaviour, NetworkBehaviourAction, PollParameters, ProtocolsHandler};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::convert::TryInto;
use std::path::PathBuf;
use tokio_io::{AsyncRead, AsyncWrite};
use wasm_timer::Instant;

/// Network behaviour that handles Bittorrent.
pub struct Bittorrent<TSubstream, TFileSystem>
where
    TFileSystem: FileSystem,
{
    /// The filesystem storage.
    disk_manager: DiskManager<TFileSystem>,
    /// The currently active piece operations.
    torrents: TorrentPool<TorrentInner>,
    /// The currently connected peers.
    connected_peers: FnvHashSet<PeerId>,
    /// Queued events to return when the behaviour is being polled.
    queued_events:
        VecDeque<NetworkBehaviourAction<BittorrentHandlerIn<TorrentId>, BittorrentEvent>>,
    /// cannot be higher than the active torrents
    max_simultaneous_downloads: usize,
    /// new torrents won't start if more are seeded/leeched
    max_active_torrents: usize,
    /// move finished downloads
    move_completed_downloads: Option<PathBuf>,
    /// how the initial pieces to download are selected
    initial_piece_selection: PieceSelection,
    /// Marker to pin the generics.
    marker: PhantomData<TSubstream>,
}

impl<TSubstream, TFileSystem> Bittorrent<TSubstream, TFileSystem>
where
    TFileSystem: FileSystem,
{
    /// Creates a new `Bittorrent` network behaviour with the given configuration.
    pub fn with_config(id: PeerId, filesystem: TFileSystem, config: BittorrentConfig) -> Self {
        unimplemented!()
    }

    /// start handshaking with peers for specific hash
    pub fn handshakes(&mut self, info_hash: ShaHash) {
        let local_peer_hash = self.torrents.local_peer_hash.clone();
        // TODO candidate identification should be a DHT lookup
        // for now consider all connected peers potential candidates
        for candidate_peer_id in self.torrents.iter_candidate_peer_ids(&info_hash) {
            self.queued_events
                .push_back(NetworkBehaviourAction::SendEvent {
                    peer_id: candidate_peer_id.clone(),
                    event: BittorrentHandlerIn::HandshakeOut {
                        info_hash: info_hash.clone(),
                    },
                });
        }
    }

    pub fn choke_peer(&mut self, peer_id: &PeerId) {
        unimplemented!()
    }

    pub fn unchoke_peer(&mut self, peer_id: &PeerId) {
        unimplemented!()
    }

    /// Add a new torrent to leech from peers
    pub fn add_leech<T: AsRef<ShaHash>>(&mut self, metainfo: MetaInfo, peers: PeerId) {
        unimplemented!()
    }

    /// Add a new torrent as seed.
    pub fn add_seed(&mut self, seed: TorrentSeed) {
        unimplemented!()
    }

    pub fn remove_torrent(&mut self, info_hash: &ShaHash) {
        unimplemented!()
    }

    pub fn pause_torrent(&mut self, info_hash: &ShaHash) {
        unimplemented!()
    }

    pub fn restart_torrent(&mut self, info_hash: &ShaHash) {
        unimplemented!()
    }

    /// Handles a finished (i.e. successful) torrent.
    fn torrent_finished(
        &mut self,
        torrent: Torrent<TorrentInner>,
        params: &mut impl PollParameters,
    ) -> Option<BittorrentEvent> {
        unimplemented!()
    }

    /// Handles a peer that failed to send a `KeepAlive`.
    fn remote_timeout(&self, peer_id: PeerId) -> Option<BittorrentEvent> {
        unimplemented!()
    }

    /// Handles a peer that failed to send a `KeepAlive`.
    fn send_keepalive(&mut self, peer_id: PeerId, peer: &mut BttPeer) {
        self.queued_events
            .push_back(NetworkBehaviourAction::SendEvent {
                peer_id,
                event: BittorrentHandlerIn::KeepAlive,
            });
    }

    /// Handles a block for a specific torrent
    fn block_ready(
        &mut self,
        torrent_id: TorrentId,
        buffer: BlockBuffer,
    ) -> Option<BittorrentEvent> {
        match buffer.try_into() {
            Ok(block) => {
                self.disk_manager.write_block(torrent_id, block);
                None
            }
            Err(err) => Some(BittorrentEvent::BlockResult(Err(err))),
        }
    }

    /// Handles a finished (i.e. successful) piece.
    fn piece_finished(
        &mut self,
        torrent: Torrent<TorrentInner>,
        params: &mut impl PollParameters,
    ) -> Option<BittorrentEvent> {
        unimplemented!()
    }

    /// Gets a mutable reference to the disk manager.
    pub fn disk_manager_mut(&mut self) -> &mut DiskManager<TFileSystem> {
        &mut self.disk_manager
    }
}

/// The configuration for the `Bittorrent` behaviour.
///
/// The configuration is consumed by [`Bittorrent::new`].
#[derive(Debug, Clone)]
pub struct BittorrentConfig {
    /// cannot be higher than the active torrents
    max_simultaneous_downloads: Option<usize>,
    /// new torrents won't start if more are seeded/leeched
    max_active_torrents: Option<usize>,
    /// move finished downloads
    move_completed_downloads: Option<PathBuf>,
    /// torrents that are not done yet
    resume_leech: Option<Vec<PathBuf>>,
    /// files to seed
    files_to_seed: Option<Vec<TorrentSeed>>,
    /// how the initial pieces to download are selected
    initial_piece_selection: Option<PieceSelection>,
}

impl BittorrentConfig {
    const MAX_ACTIVE_TORRENTS: usize = 5;
}

impl Default for BittorrentConfig {
    fn default() -> Self {
        Self {
            max_simultaneous_downloads: Some(Self::MAX_ACTIVE_TORRENTS),
            max_active_torrents: Some(Self::MAX_ACTIVE_TORRENTS),
            initial_piece_selection: Some(Default::default()),
            move_completed_downloads: None,
            resume_leech: None,
            files_to_seed: None,
        }
    }
}
impl<TSubstream, TFileSystem> NetworkBehaviour for Bittorrent<TSubstream, TFileSystem>
where
    TSubstream: AsyncRead + AsyncWrite,
    TFileSystem: FileSystem,
{
    type ProtocolsHandler = BittorrentHandler<TSubstream, TorrentId>;
    type OutEvent = BittorrentEvent;

    fn new_handler(&mut self) -> Self::ProtocolsHandler {
        BittorrentHandler::seed_and_leech()
    }

    fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
        Vec::new()
    }

    fn inject_connected(&mut self, peer_id: PeerId, endpoint: ConnectedPoint) {
        // TODO lookup peer in DHT for torrents

        // handshake for every torrent we currently leeching
        for torrent in self.torrents.iter_leeching() {
            self.queued_events
                .push_back(NetworkBehaviourAction::SendEvent {
                    peer_id: peer_id.clone(),
                    event: BittorrentHandlerIn::HandshakeOut {
                        info_hash: torrent.info_hash.clone(),
                    },
                });
        }

        self.connected_peers.insert(peer_id);
    }

    fn inject_disconnected(&mut self, peer_id: &PeerId, endpoint: ConnectedPoint) {
        unimplemented!()
    }

    fn inject_node_event(&mut self, peer_id: PeerId, event: BittorrentHandlerEvent<TorrentId>) {
        // torrents are identified by peer + active torrent id as user data in the handler event
        unimplemented!()
    }

    fn poll(
        &mut self,
        parameters: &mut impl PollParameters,
    ) -> Async<
        NetworkBehaviourAction<
            <Self::ProtocolsHandler as ProtocolsHandler>::InEvent,
            Self::OutEvent,
        >,
    > {
        let now = Instant::now();

        loop {
            // drain pending disk io
            match self.disk_manager.poll() {
                Ok(Async::Ready(event)) => {
                    // TODO handle disk event to update torrent stats, etc.
                    return Async::Ready(NetworkBehaviourAction::GenerateEvent(
                        BittorrentEvent::DiskResult,
                    ));
                }
                Err(err) => {
                    //
                }
                _ => {
                    return Async::NotReady;
                }
            };

            // Drain queued events.
            if let Some(event) = self.queued_events.pop_front() {
                return Async::Ready(event);
            }

            // Look for a finished bittorrent.
            loop {
                match self.torrents.poll(now) {
                    TorrentPoolState::Finished(q) => {
                        if let Some(event) = self.torrent_finished(q, parameters) {
                            return Async::Ready(NetworkBehaviourAction::GenerateEvent(event));
                        }
                    }
                    TorrentPoolState::Timeout(id) => {
                        if let Some(event) = self.remote_timeout(id) {
                            return Async::Ready(NetworkBehaviourAction::GenerateEvent(event));
                        }
                    }
                    TorrentPoolState::BlockReady((torrent_id, buffer)) => {
                        // TODO handle error
                        self.block_ready(torrent_id, buffer);
                    }
                    TorrentPoolState::KeepAlive(peer_id) => {
                        return Async::Ready(NetworkBehaviourAction::SendEvent {
                            peer_id,
                            event: BittorrentHandlerIn::KeepAlive,
                        });
                    }
                    TorrentPoolState::Waiting(Some((torrent, peer_id))) => {
                        panic!();
                    }
                    TorrentPoolState::Waiting(None) | TorrentPoolState::Idle => break,
                }
            }

            // No immediate event was produced as a result of a finished bittorrent.
            // If no new events have been queued either, signal `NotReady` to
            // be polled again later.
            if self.queued_events.is_empty() {
                return Async::NotReady;
            }
        }
    }
}

// Events

/// The events produced by the `Bittorrent` behaviour.
///
/// See [`Bittorrent::poll`].
#[derive(Debug)]
pub enum BittorrentEvent {
    HandshakeResult(HandshakeResult),
    BlockResult(BlockResult),
    DiskResult,
}

/// The result of [`Bittorrent::handshake`].
pub type HandshakeResult = Result<HandshakeOk, HandshakeError>;

/// The successful result of [`Bittorrent::handshake`].
#[derive(Debug, Clone)]
pub struct HandshakeOk(PeerId);

/// The error result of [`Bittorrent::handshake`].
#[derive(Debug, Clone)]
pub enum HandshakeError {
    Timeout { peer: PeerId },
}

#[derive(Debug, Clone)]
pub enum InterestedPeerResult {
    Interested,
    NotInterested,
    Timeout,
}

/// The error result of [`Kademlia::get_closest_peers`].
#[derive(Debug, Clone)]
pub enum InterestedPeerError {
    Timeout { key: Vec<u8>, peers: Vec<PeerId> },
}

pub type BlockResult = Result<GoodPiece, TorrentError>;

/// Message indicating that a good piece has been identified for
/// the given torrent (hash), as well as the piece index.
#[derive(Debug, Clone)]
pub struct GoodPiece((TorrentId, u64));

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SeedLeechConfig {
    /// only send blocks
    Seed,
    /// only receive blocks
    Leech,
    /// send blocks to remote and receive missing blocks
    Both,
}

impl Default for SeedLeechConfig {
    fn default() -> Self {
        SeedLeechConfig::Both
    }
}

/// Internal piece state
struct TorrentInner {
    /// The piece-specific state.
    info: TorrentInfo,
}

pub struct TorrentInfo {}
