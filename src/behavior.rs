//! Implementation of the `BitTorrent` network behaviour.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::convert::TryInto;
use std::marker::PhantomData;
use std::path::PathBuf;

use fnv::{FnvHashMap, FnvHashSet};
use futures::{Async, Future};
use libp2p_core::{ConnectedPoint, Multiaddr, PeerId};
use libp2p_swarm::{NetworkBehaviour, NetworkBehaviourAction, PollParameters, ProtocolsHandler};
use smallvec::SmallVec;
use tokio_io::{AsyncRead, AsyncWrite};
use wasm_timer::Instant;

use crate::util::ShaHash;

use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata};
use crate::disk::message::DiskMessageOut;
use crate::peer::builder::TorrentConfig;
use crate::peer::piece::PieceBuffer;
use crate::peer::torrent::TorrentState;
use crate::peer::{BitTorrentPeer, ChokeType, InterestType};
use crate::proto::message::{Handshake, PeerMessage, PeerRequest};
use crate::{
    disk::error::TorrentError,
    disk::fs::FileSystem,
    disk::manager::DiskManager,
    disk::torrent::TorrentSeed,
    handler::BitTorrentHandler,
    handler::BitTorrentHandlerEvent,
    handler::BitTorrentHandlerIn,
    peer::torrent::{Torrent, TorrentId, TorrentPeer, TorrentPool, TorrentPoolState},
    piece::PieceSelection,
    torrent::MetaInfo,
};
use std::ptr::hash;

// TODO add dht support

/// Network behaviour that handles BitTorrent.
pub struct BitTorrent<TSubstream, TFileSystem>
where
    TFileSystem: FileSystem,
{
    /// The filesystem storage.
    disk_manager: DiskManager<TFileSystem>,
    /// The currently active piece operations.
    torrents: TorrentPool,
    /// The currently connected peers.
    connected_peers: FnvHashSet<PeerId>,
    /// Queued events to return when the behaviour is being polled.
    queued_events:
        VecDeque<NetworkBehaviourAction<BitTorrentHandlerIn<TorrentId>, BitTorrentEvent>>,
    /// Marker to pin the generics.
    marker: PhantomData<TSubstream>,
    /// Known peers with their torrents
    known_peers: FnvHashMap<PeerId, (Multiaddr, HashSet<ShaHash>)>,
    /// Pending handshakes
    pending_handshakes: FnvHashMap<PeerId, ShaHash>,
}

impl<TSubstream, TFileSystem> BitTorrent<TSubstream, TFileSystem>
where
    TFileSystem: FileSystem,
{
    /// Creates a new `BitTorrent` network behaviour with the given
    /// configuration.
    pub fn with_config(peer_id: PeerId, filesystem: TFileSystem, config: BitTorrentConfig) -> Self {
        Self {
            disk_manager: DiskManager::with_capacity(filesystem.into(), 100),
            torrents: TorrentPool::new(peer_id, config),
            connected_peers: Default::default(),
            queued_events: Default::default(),
            marker: PhantomData,
            known_peers: Default::default(),
            pending_handshakes: Default::default(),
        }
    }

    pub fn add_address_torrents(
        &mut self,
        peer: PeerId,
        address: Multiaddr,
        torrents: &[ShaHash],
    ) -> bool {
        if let Some((know_address, know_torrents)) = self.known_peers.get_mut(&peer) {
            *know_address = address;
            know_torrents.extend(torrents.into_iter());
            true
        } else {
            self.known_peers
                .insert(peer, (address, torrents.iter().cloned().collect()));
            false
        }
    }

    pub fn handshake_known_peer<T: Into<ShaHash>>(
        &mut self,
        peer: PeerId,
        torrent: T,
    ) -> Result<(), (PeerId, ShaHash)> {
        let torrent = torrent.into();
        if self.pending_handshakes.contains_key(&peer) || self.connected_peers.contains(&peer) {
            // already connected or handshake still pending
            return Err((peer, torrent));
        }

        if self.known_peers.contains_key(&peer) {
            self.queued_events
                .push_back(NetworkBehaviourAction::DialPeer {
                    peer_id: peer.clone(),
                });
            self.pending_handshakes.insert(peer, torrent);
            Ok(())
        } else {
            Err((peer, torrent))
        }
    }

    /// start handshaking with a peer for specific torrent
    fn send_handshake<T: Into<ShaHash>>(&mut self, info_hash: T, peer_id: PeerId) {
        // TODO candidate identification should be a DHT lookup
        let info_hash = info_hash.into();
        if let Some(torrent) = self.torrents.get_for_info_hash(&info_hash) {
            self.queued_events
                .push_back(NetworkBehaviourAction::SendEvent {
                    peer_id,
                    event: BitTorrentHandlerIn::HandshakeReq {
                        handshake: Handshake::new(info_hash, self.torrents.local_peer_hash.clone()),
                        user_data: torrent.id(),
                    },
                });
        } else {
            self.queued_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    BitTorrentEvent::HandshakeResult(Err(HandshakeError::InfoHashNotFound(
                        peer_id, info_hash,
                    ))),
                ))
        }
    }

    pub fn choke_peer(&mut self, peer_id: &PeerId) {
        if let Ok(id) = self.torrents.choke_remote(peer_id) {
            self.queued_events
                .push_back(NetworkBehaviourAction::SendEvent {
                    peer_id: peer_id.clone(),
                    event: BitTorrentHandlerIn::Choke {
                        inner: ChokeType::Choked,
                    },
                });
        } else {
            self.queued_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    BitTorrentEvent::ChokeResult(Err(PeerError::NotFound(peer_id.clone()))),
                ))
        }
    }

    pub fn unchoke_peer(&mut self, peer_id: &PeerId) {
        if let Ok(id) = self.torrents.unchoke_remote(peer_id) {
            self.queued_events
                .push_back(NetworkBehaviourAction::SendEvent {
                    peer_id: peer_id.clone(),
                    event: BitTorrentHandlerIn::Choke {
                        inner: ChokeType::UnChoked,
                    },
                });
        } else {
            self.queued_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    BitTorrentEvent::ChokeResult(Err(PeerError::NotFound(peer_id.clone()))),
                ))
        }
    }

    /// Add a new torrent to leech from peers
    pub fn add_leech(&mut self, torrent: MetaInfo, state: TorrentState) {
        let bitfield = BitField::new_all_clear(torrent.info.pieces.len());
        match self
            .torrents
            .add_torrent(TorrentConfig::new_leech(&torrent, state))
        {
            Ok((id, actual_state)) => {
                self.queued_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        BitTorrentEvent::TorrentAddedResult(Ok(TorrentAddedOk::NewLeech {
                            torrent: id,
                            state: actual_state,
                        })),
                    ));
                self.disk_manager.add_torrent(id, torrent)
            }
            Err(info_hash) => self
                .queued_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    BitTorrentEvent::TorrentAddedResult(Err(TorrentAddedErr::AlreadyExist {
                        info_hash,
                        meta_info: torrent,
                    })),
                )),
        }
    }

    /// Add a new torrent as seed .
    pub fn add_seed(&mut self, seed: TorrentSeed, state: TorrentState) {
        match self
            .torrents
            .add_torrent(TorrentConfig::new_seed(&seed.torrent, state))
        {
            Ok((id, actual_state)) => {
                self.queued_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        BitTorrentEvent::TorrentAddedResult(Ok(TorrentAddedOk::NewSeed {
                            torrent: id,
                            state: actual_state,
                        })),
                    ));
                self.disk_manager.add_torrent(id, seed.torrent)
            }
            Err(info_hash) => self
                .queued_events
                .push_back(NetworkBehaviourAction::GenerateEvent(
                    BitTorrentEvent::TorrentAddedResult(Err(TorrentAddedErr::AlreadyExist {
                        info_hash,
                        meta_info: seed.torrent,
                    })),
                )),
        }
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

    /// Handles a peer that failed to send a `KeepAlive`.
    fn on_remote_timeout(&self, peer_id: PeerId) -> Option<BitTorrentEvent> {
        // TODO
        None
    }
}

/// The configuration for the `BitTorrent` behaviour.
///
/// The configuration is consumed by [`BitTorrent::new`].
#[derive(Debug, Clone)]
pub struct BitTorrentConfig {
    /// cannot be higher than the active torrents
    pub max_simultaneous_downloads: Option<usize>,
    /// new torrents won't start if more are seeded/leeched
    pub max_active_torrents: Option<usize>,
    /// move finished downloads
    pub move_completed_downloads: Option<PathBuf>,
    /// torrents that are not done yet
    pub resume_leech: Option<Vec<PathBuf>>,
    /// files to seed
    pub files_to_seed: Option<Vec<TorrentSeed>>,
    /// how the initial pieces to download are selected
    pub initial_piece_selection: Option<PieceSelection>,
    /// the bittorrent peer hash used for the client
    pub peer_hash: Option<ShaHash>,
}

impl BitTorrentConfig {
    pub const MAX_ACTIVE_TORRENTS: usize = 5;
}

impl Default for BitTorrentConfig {
    fn default() -> Self {
        Self {
            max_simultaneous_downloads: Some(Self::MAX_ACTIVE_TORRENTS),
            max_active_torrents: Some(Self::MAX_ACTIVE_TORRENTS),
            initial_piece_selection: Some(Default::default()),
            move_completed_downloads: None,
            resume_leech: None,
            files_to_seed: None,
            peer_hash: None,
        }
    }
}

impl<TSubstream, TFileSystem> NetworkBehaviour for BitTorrent<TSubstream, TFileSystem>
where
    TSubstream: AsyncRead + AsyncWrite,
    TFileSystem: FileSystem,
{
    type ProtocolsHandler = BitTorrentHandler<TSubstream, TorrentId>;
    type OutEvent = BitTorrentEvent;

    fn new_handler(&mut self) -> Self::ProtocolsHandler {
        BitTorrentHandler::seed_and_leech()
    }

    fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
        if let Some((addr, _)) = self.known_peers.get(peer_id) {
            vec![addr.clone()]
        } else {
            Vec::new()
        }
    }

    fn inject_connected(&mut self, peer_id: PeerId, endpoint: ConnectedPoint) {
        // TODO lookup peer in DHT for torrents

        match endpoint {
            ConnectedPoint::Dialer { .. } => {
                if let Some((id, hash)) = self.pending_handshakes.remove_entry(&peer_id) {
                    self.send_handshake(hash, id);
                }
            }
            ConnectedPoint::Listener { send_back_addr, .. } => {
                self.add_address_torrents(peer_id.clone(), send_back_addr, &[]);
            }
        }

        self.connected_peers.insert(peer_id);
    }

    fn inject_disconnected(&mut self, peer_id: &PeerId, endpoint: ConnectedPoint) {
        // remove torrent from the pool
        self.torrents.remove_peer(peer_id);
        self.connected_peers.remove(peer_id);
    }

    /// send from the handler
    fn inject_node_event(&mut self, peer_id: PeerId, event: BitTorrentHandlerEvent<TorrentId>) {
        match event {
            BitTorrentHandlerEvent::HandshakeReq {
                handshake,
                request_id,
            } => {
                // a peer initiated a handshake
                match self
                    .torrents
                    .on_handshake_request(peer_id.clone(), handshake)
                {
                    Ok(handshake) => {
                        debug!("answering good handshake request.");
                        self.queued_events
                            .push_back(NetworkBehaviourAction::SendEvent {
                                peer_id,
                                event: BitTorrentHandlerIn::HandshakeRes {
                                    handshake,
                                    request_id,
                                },
                            });
                    }
                    Err(err) => {
                        debug!("denying bad handshake request.");
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::HandshakeResult(Err(err)),
                            ));
                        // drop the connection
                        self.queued_events
                            .push_back(NetworkBehaviourAction::SendEvent {
                                peer_id,
                                event: BitTorrentHandlerIn::Disconnect(None),
                            });
                    }
                }
            }
            BitTorrentHandlerEvent::HandshakeRes {
                handshake,
                user_data,
            } => {
                debug!("received handshake response");
                // a peer responded to our handshake request
                match self
                    .torrents
                    .on_handshake_response(peer_id.clone(), user_data, &handshake)
                {
                    Ok((handshake, index_field)) => {
                        // initiate bitfield exchange with the remote
                        self.queued_events
                            .push_back(NetworkBehaviourAction::SendEvent {
                                peer_id,
                                event: BitTorrentHandlerIn::BitfieldReq {
                                    index_field,
                                    user_data,
                                },
                            });

                        // notify user
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::HandshakeResult(Ok(handshake)),
                            ));
                    }
                    Err(e) => {
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::HandshakeResult(Err(e)),
                            ));
                    }
                }
            }
            BitTorrentHandlerEvent::BitfieldReq {
                index_field,
                request_id,
            } => {
                // a peer requested the bitfield
                if let Some(index_field) = self.torrents.on_bitfield_request(&peer_id, index_field)
                {
                    debug!("received good bitfield request");
                    self.queued_events
                        .push_back(NetworkBehaviourAction::SendEvent {
                            peer_id,
                            event: BitTorrentHandlerIn::BitfieldRes {
                                index_field,
                                request_id,
                            },
                        });
                } else {
                    debug!("received bad bitfield request");
                    // peer is not associated with any torrent in the pool
                    self.queued_events
                        .push_back(NetworkBehaviourAction::SendEvent {
                            peer_id,
                            event: BitTorrentHandlerIn::Disconnect(None),
                        });
                }
            }
            BitTorrentHandlerEvent::BitfieldRes {
                index_field,
                user_data,
            } => {
                // we received a bitfield from a remote
                if self
                    .torrents
                    .set_peer_bitfield(&peer_id, user_data, index_field)
                {
                    // automatically send an interest message if remote has something we want
                    if self.torrents.is_remote_interesting(user_data, &peer_id) {
                        self.torrents.set_client_interest(
                            user_data,
                            &peer_id,
                            InterestType::Interested,
                        );
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::BitfieldResult(Ok(BitfieldOk {
                                    torrent: user_data,
                                    peer: peer_id.clone(),
                                })),
                            ));
                        self.queued_events
                            .push_back(NetworkBehaviourAction::SendEvent {
                                peer_id,
                                event: BitTorrentHandlerIn::Interest {
                                    inner: InterestType::Interested,
                                },
                            });
                    }
                } else {
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::BitfieldResult(Err(PeerError::NotFound(peer_id))),
                        ));
                }
            }
            BitTorrentHandlerEvent::GetPieceReq {
                request,
                request_id,
            } => {
                debug!("received new block request {:?}", request);
                // remote wants a new block
                // TODO check config for seeding
                // validate that we can send a block first
                match self
                    .torrents
                    .on_piece_request(peer_id, request.into(), request_id)
                {
                    Ok((id, block_metadata)) => {
                        debug!("queuing block request {:?}", block_metadata);
                        self.disk_manager.read_block(id, block_metadata);
                    }
                    Err((peer_id, block_metadata, request_id)) => {
                        debug!("can't serve block request {:?}", block_metadata);
                        self.queued_events
                            .push_back(NetworkBehaviourAction::SendEvent {
                                peer_id: peer_id.clone(),
                                event: BitTorrentHandlerIn::Reset(request_id),
                            });

                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::SeedBlockResult(Err(
                                    SeedBlockErr::RemoteNotAllowed {
                                        peer: peer_id,
                                        block_metadata,
                                    },
                                )),
                            ))
                    }
                }
            }
            BitTorrentHandlerEvent::GetPieceRes { piece, user_data } => {
                match self
                    .torrents
                    .on_block_response(user_data, &peer_id, piece.into())
                {
                    Ok(block_metadata) => {
                        debug!("received good block response {:?}", block_metadata);
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::LeechBlockResult(Ok(BlockOk {
                                    peer: peer_id,
                                    block_metadata,
                                    torrent: user_data,
                                })),
                            ))
                    }
                    Err(err) => {
                        debug!("received bad block response {:?}", err);
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::LeechBlockResult(Err(err)),
                            ))
                    }
                }
            }
            BitTorrentHandlerEvent::CancelPiece {
                request_id,
                request,
            } => {
                if let Some((peer, block)) = self.torrents.on_cancel_request(request_id) {
                    debug!("Cancelled read by remote {:?}", peer);
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::SeedBlockResult(Ok(SeedBlockOk::CancelledByRemote {
                                peer: peer_id,
                                block_metadata: request.into(),
                            })),
                        ));
                } else {
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::SeedBlockResult(Err(SeedBlockErr::NotFound(peer_id))),
                        ));
                }
            }
            BitTorrentHandlerEvent::Choke { inner } => {
                if let Some((torrent_id, old_choke_ty)) =
                    self.torrents.on_choke_by_remote(&peer_id, inner)
                {
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::ChokeResult(Ok(ChokeOk {
                                torrent: torrent_id,
                                peer: peer_id,
                                old_choke_ty,
                                new_choke_ty: inner,
                            })),
                        ))
                } else {
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::ChokeResult(Err(PeerError::NotFound(peer_id))),
                        ))
                }
            }
            BitTorrentHandlerEvent::Interest { inner } => {
                debug!("Received interest {:?} from remote {:?}", inner, peer_id);
                let interest = self.torrents.on_interest_by_remote(peer_id, inner);
                // TODO optimistic unchoking if remote is interested
                self.queued_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        BitTorrentEvent::InterestResult(interest),
                    ))
            }
            BitTorrentHandlerEvent::Have { index } => {
                if let Some(valid_piece) = self.torrents.on_remote_have(&peer_id, index as usize) {
                    if valid_piece {
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::HaveResult(Ok(HaveOk {
                                    peer: peer_id,
                                    piece: index,
                                })),
                            ))
                    } else {
                        self.queued_events
                            .push_back(NetworkBehaviourAction::GenerateEvent(
                                BitTorrentEvent::HaveResult(Err(HaveErr::InvalidIndex(index))),
                            ))
                    }
                } else {
                    self.queued_events
                        .push_back(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::HaveResult(Err(HaveErr::NotFound(peer_id))),
                        ))
                }
            }
            BitTorrentHandlerEvent::KeepAlive { timestamp } => {
                self.queued_events
                    .push_back(NetworkBehaviourAction::GenerateEvent(
                        BitTorrentEvent::KeepAliveResult(
                            self.torrents.on_keep_alive_by_remote(peer_id, timestamp),
                        ),
                    ))
            }
            BitTorrentHandlerEvent::TorrentErr { .. } => {}
        }
    }

    /// send to user or handler
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
            // Drain queued events.
            if let Some(event) = self.queued_events.pop_front() {
                return Async::Ready(event);
            }

            // advance the torrent pool state
            loop {
                match self.torrents.poll(now) {
                    TorrentPoolState::Idle => break,
                    TorrentPoolState::Timeout(torrent, peer) => {
                        if let Some(event) = self.on_remote_timeout(peer) {
                            return Async::Ready(NetworkBehaviourAction::GenerateEvent(event));
                        }
                    }
                    TorrentPoolState::PieceReady(res) => {
                        match res {
                            Ok((torrent_id, piece)) =>
                            // write complete piece to disk
                            {
                                debug!("Piece ready to write {:?}", piece.metadata());
                                self.disk_manager.write_block(torrent_id, piece)
                            }
                            Err(e) => {
                                // TODO reset torrent piece handler
                            }
                        }
                    }
                    TorrentPoolState::KeepAlive(torrent, peer_id) => {
                        debug!("sending keepalive to {:?}", peer_id);
                        // a new keepalive msg needs to be send
                        return Async::Ready(NetworkBehaviourAction::SendEvent {
                            peer_id,
                            event: BitTorrentHandlerIn::KeepAlive,
                        });
                    }
                    TorrentPoolState::Waiting(torrent) => {
                        debug!("Torrent {:?} waiting...", torrent.id());
                        break;
                    }
                    TorrentPoolState::Finished(torrent_id) => {
                        debug!("torrent complete {:?}", torrent_id);
                        return Async::Ready(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::TorrentFinished(torrent_id),
                        ));
                    }
                    TorrentPoolState::NextBlock(block) => {
                        debug!("requesting new block {:?}", block);
                        return Async::Ready(NetworkBehaviourAction::SendEvent {
                            peer_id: block.peer_id,
                            event: BitTorrentHandlerIn::GetPieceReq {
                                user_data: block.torrent_id,
                                request: block.block_metadata.into(),
                            },
                        });
                    }
                }
            }

            loop {
                // drain pending disk io
                match self.disk_manager.poll() {
                    Ok(Async::Ready(event)) => {
                        match event {
                            DiskMessageOut::TorrentAdded(_) => {
                                continue;
                            }
                            DiskMessageOut::TorrentRemoved(_, _) => {
                                continue;
                            }
                            DiskMessageOut::TorrentSynced(id) => {
                                self.queued_events.push_back(
                                    NetworkBehaviourAction::GenerateEvent(
                                        BitTorrentEvent::DiskResult(Ok(
                                            DiskMessageOut::TorrentSynced(id),
                                        )),
                                    ),
                                );
                            }
                            DiskMessageOut::BlockRead(torrent, block) => {
                                debug!("disk manager read: {:?}", block.metadata());

                                if let Some((peer_id, request_id)) =
                                    self.torrents.on_block_read(torrent, &block.metadata())
                                {
                                    self.queued_events.push_back(
                                        NetworkBehaviourAction::SendEvent {
                                            peer_id,
                                            event: BitTorrentHandlerIn::GetPieceRes {
                                                piece: block.into_piece(),
                                                request_id,
                                            },
                                        },
                                    );
                                } else {
                                    // TODO error handling
                                    debug!(
                                        "Failed to match read block to peer {:?}",
                                        block.metadata()
                                    );
                                }
                            }
                            DiskMessageOut::PieceWritten(torrent_id, block) => {
                                if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
                                    let index = block.metadata().piece_index as u32;

                                    // acknowledge that the piece is written
                                    torrent.set_piece(block.metadata().piece_index as usize);

                                    // send a new Have message to all peers of this torrent
                                    for peer in torrent.iter_peer_ids() {
                                        self.queued_events.push_back(
                                            NetworkBehaviourAction::SendEvent {
                                                peer_id: peer.clone(),
                                                event: BitTorrentHandlerIn::Have { index },
                                            },
                                        );
                                    }
                                }
                            }
                            DiskMessageOut::TorrentError(id, err) => {
                                // error occurred while adding removing the
                                // torrent
                            }
                            DiskMessageOut::ReadBlockError(id, err) => {
                                debug!("encountered error while reading block {:?}", err);
                                return Async::Ready(NetworkBehaviourAction::GenerateEvent(
                                    BitTorrentEvent::DiskResult(Ok(
                                        DiskMessageOut::ReadBlockError(id, err),
                                    )),
                                ));
                            }
                            DiskMessageOut::WriteBlockError(id, err) => {
                                debug!("encountered error while writing block {:?}", err);
                                // TODO err handling
                            }
                            DiskMessageOut::MovedTorrent(id, dest) => {
                                debug!("Moved torrent {:?} to {}", id, dest.display());
                            }
                        }
                    }
                    Err(err) => {
                        return Async::Ready(NetworkBehaviourAction::GenerateEvent(
                            BitTorrentEvent::DiskResult(Err(())),
                        ));
                    }
                    Ok(Async::NotReady) => {
                        break;
                    }
                };
            }

            // No immediate event was produced as a result of a finished bittorrent event.
            // If no new events have been queued either, signal `NotReady` to
            // be polled again later.
            if self.queued_events.is_empty() {
                return Async::NotReady;
            }
        }
    }
}

/// The events produced by the `BitTorrent` behaviour.
///
/// See [`BitTorrent::poll`].
pub enum BitTorrentEvent {
    HandshakeResult(HandshakeResult),
    LeechBlockResult(LeechBlockResult),
    BitfieldResult(BitfieldResult),
    SeedBlockResult(SeedBlockResult),
    DiskResult(DiskResult),
    KeepAliveResult(KeepAliveResult),
    ChokeResult(ChokeResult),
    InterestResult(InterestResult),
    HaveResult(HaveResult),
    TorrentFinished(TorrentId),
    TorrentAddedResult(TorrentAddedResult),
}

/// The result of [`BitTorrent::handshake`].
pub type HandshakeResult = Result<HandshakeOk, HandshakeError>;

/// The result of a diskmanager task
pub type DiskResult = Result<DiskMessageOut, ()>;

#[derive(Debug, Clone)]
pub enum DiskOk {
    ReadBlock {
        torrent: TorrentId,
        block: BlockMetadata,
    },
}

#[derive(Debug, Clone)]
pub enum DiskError {
    PieceHashMismatch {
        torrent: TorrentId,
        piece_index: u64,
        expected: ShaHash,
        got: ShaHash,
    },
}

/// The successful result of [`BitTorrent::handshake`].
#[derive(Debug, Clone)]
pub struct HandshakeOk(pub PeerId);

/// The error result of [`BitTorrent::handshake`].
#[derive(Debug, Clone)]
pub enum HandshakeError {
    InfoHashMismatch(PeerId, Option<TorrentId>),
    InfoHashNotFound(PeerId, ShaHash),
    InvalidPeer(PeerId, Option<TorrentId>),
    Timeout(PeerId),
}

pub type BitfieldResult = Result<BitfieldOk, PeerError>;

#[derive(Debug, Clone)]
pub struct BitfieldOk {
    pub torrent: TorrentId,
    pub peer: PeerId,
}

/// The result of a `KeepAlive`.
pub type ChokeResult = Result<ChokeOk, PeerError>;

#[derive(Debug, Clone)]
pub struct ChokeOk {
    pub torrent: TorrentId,
    pub peer: PeerId,
    pub old_choke_ty: ChokeType,
    pub new_choke_ty: ChokeType,
}

pub type KeepAliveResult = Result<KeepAliveOk, PeerError>;

#[derive(Debug, Clone)]
pub enum KeepAliveOk {
    Remote {
        torrent: TorrentId,
        peer: PeerId,
        old_heartbeat: Instant,
        new_heartbeat: Instant,
    },
    Client {
        torrent: TorrentId,
        peer: PeerId,
        old_heartbeat: Instant,
        new_heartbeat: Instant,
    },
}

#[derive(Debug, Clone)]
pub enum PeerError {
    NotFound(PeerId),
}

/// The result of a `KeepAlive`.
pub type InterestResult = Result<InterestOk, PeerError>;

#[derive(Debug, Clone)]
pub enum InterestOk {
    Interested(PeerId),
    NotInterested(PeerId),
}

/// The result of a `KeepAlive`.
pub type HaveResult = Result<HaveOk, HaveErr>;

#[derive(Debug, Clone)]
pub struct HaveOk {
    pub peer: PeerId,
    pub piece: u32,
}

#[derive(Debug, Clone)]
pub enum HaveErr {
    NotFound(PeerId),
    /// Provided piece index was out of bounds.
    InvalidIndex(u32),
}

pub type LeechBlockResult = Result<BlockOk, BlockErr>;

#[derive(Debug, Clone)]
pub struct BlockOk {
    pub peer: PeerId,
    pub block_metadata: BlockMetadata,
    pub torrent: TorrentId,
}

#[derive(Debug)]
pub enum BlockErr {
    NotRequested {
        peer: PeerId,
        block: Block,
    },
    InvalidBlock {
        peer: PeerId,
        expected: BlockMetadata,
        block: Block,
    },
}

pub type SeedBlockResult = Result<SeedBlockOk, SeedBlockErr>;

pub enum SeedBlockOk {
    Ok(BlockOk),
    CancelledByRemote {
        peer: PeerId,
        block_metadata: BlockMetadata,
    },
}

#[derive(Debug, Clone)]
pub enum SeedBlockErr {
    RemoteNotAllowed {
        peer: PeerId,
        block_metadata: BlockMetadata,
    },
    NotFound(PeerId),
}

pub type TorrentAddedResult = Result<TorrentAddedOk, TorrentAddedErr>;

#[derive(Debug, Clone)]
pub enum TorrentAddedOk {
    NewSeed {
        torrent: TorrentId,
        state: TorrentState,
    },
    NewLeech {
        torrent: TorrentId,
        state: TorrentState,
    },
}

#[derive(Debug, Clone)]
pub enum TorrentAddedErr {
    AlreadyExist {
        info_hash: ShaHash,
        meta_info: MetaInfo,
    },
}

/// Message indicating that a good piece has been identified for
/// the given torrent (hash), as well as the piece index.
#[derive(Debug, Clone)]
pub struct GoodPiece((TorrentId, u64));

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SeedLeechConfig {
    /// only send blocks
    SeedOnly,
    /// only receive blocks
    LeechOnly,
    /// send blocks to remote and receive missing blocks
    SeedAndLeech,
}

impl SeedLeechConfig {
    pub fn is_seed_only(&self) -> bool {
        match self {
            SeedLeechConfig::SeedOnly => true,
            _ => false,
        }
    }

    pub fn is_leech_only(&self) -> bool {
        match self {
            SeedLeechConfig::LeechOnly => true,
            _ => false,
        }
    }
    pub fn is_seed_and_leech(&self) -> bool {
        match self {
            SeedLeechConfig::SeedAndLeech => true,
            _ => false,
        }
    }

    pub fn is_seeding(&self) -> bool {
        !self.is_leech_only()
    }

    pub fn is_leeching(&self) -> bool {
        !self.is_seed_only()
    }
}

impl Default for SeedLeechConfig {
    fn default() -> Self {
        SeedLeechConfig::SeedAndLeech
    }
}
