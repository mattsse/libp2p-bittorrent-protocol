use std::collections::{BTreeMap, HashMap, HashSet};

use bytes::{BufMut, BytesMut};
use fnv::FnvHashMap;
use futures::Async;
use libp2p_core::PeerId;
use rand::{self, seq::SliceRandom, Rng};
use wasm_timer::Instant;

use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::peer::torrent::TorrentId;
use crate::peer::{BttPeer, ChokeType, InterestType};
use crate::piece::PieceSelection;

/// Tracks the progress of the handler
#[derive(Debug)]
pub enum TorrentPieceHandlerState {
    /// Wait until the state of a peer changes.
    WaitingPeers,
    /// Start a new block to leech.
    Ready(Option<NextBlock>),
    /// A new piece is ready.
    Finished(Result<Block, TorrentError>),
}

/// Tracks the state of a single torrent with all their remotes
#[derive(Debug)]
pub struct TorrentPieceHandler {
    id: TorrentId,
    /// How the next piece is selected
    selection_strategy: PieceSelection,
    /// Tracks the state of the currently downloaded piece
    piece_buffer: Option<PieceBuffer>,
    /// Which pieces the client owns and lacks
    have: BitField,
    /// length for a piece in the torrent
    piece_length: u64,
    /// All the peers related to the torrent.
    pub peers: FnvHashMap<PeerId, BttPeer>,
    /// Whether client is in endgame mode
    endgame: bool,
}

impl TorrentPieceHandler {
    /// Add a new peer for the torrent.
    pub fn insert_peer(&mut self, id: PeerId, peer: BttPeer) -> Option<BttPeer> {
        if let Some(buffer) = &mut self.piece_buffer {
            if peer.remote_can_seed_piece(buffer.piece_index as usize) {
                // consider adding to the current buffer
                if !self.peers.contains_key(&id) {
                    buffer.peers.push(id.clone());
                }
            }
        }

        self.peers.insert(id.clone(), peer)
    }

    pub fn bitfield(&self) -> &BitField {
        &self.have
    }

    pub fn set_client_choke_for_peer(
        &mut self,
        peer_id: &PeerId,
        choke: ChokeType,
    ) -> Option<ChokeType> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            Some(std::mem::replace(&mut peer.client_choke, choke))
        } else {
            None
        }
    }

    pub fn set_client_interest_for_peer(
        &mut self,
        peer_id: &PeerId,
        interest: InterestType,
    ) -> Option<InterestType> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            Some(std::mem::replace(&mut peer.client_interest, interest))
        } else {
            None
        }
    }

    pub fn set_choke_on_remote(&mut self, peer_id: &PeerId, choke: ChokeType) -> Option<ChokeType> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            let old = Some(std::mem::replace(&mut peer.remote_choke, choke));
            if let Some(buffer) = &mut self.piece_buffer {
                match &choke {
                    ChokeType::Choked => {
                        buffer.remove_peer(peer_id);
                    }
                    ChokeType::UnChoked => {
                        if peer.remote_can_seed_piece(buffer.piece_index as usize) {
                            buffer.peers.push(peer_id.clone());
                        }
                    }
                }
            }
            old
        } else {
            None
        }
    }

    pub fn set_interest_on_remote(
        &mut self,
        peer_id: &PeerId,
        interest: InterestType,
    ) -> Option<InterestType> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            let old = Some(std::mem::replace(&mut peer.remote_interest, interest));
            old
        } else {
            None
        }
    }

    /// Whether the piece was already downloaded
    pub fn has_piece(&self, piece_index: usize) -> bool {
        self.have.get(piece_index).unwrap_or_default()
    }

    pub fn remove_peer(&mut self, id: &PeerId) -> Option<BttPeer> {
        let peer = self.peers.remove(id);
        if peer.is_some() {
            if let Some(buffer) = &mut self.piece_buffer {
                buffer.remove_peer(id);
            }
        }
        peer
    }

    /// Returns the `BttPeer` that's tracked with the `PeerId`
    pub fn get_peer(&self, id: &PeerId) -> Option<&BttPeer> {
        self.peers.get(id)
    }

    pub fn get_peer_mut(&mut self, id: &PeerId) -> Option<&mut BttPeer> {
        self.peers.get_mut(id)
    }

    /// Untrack current piece
    pub fn timeout_buffer(&mut self) -> Option<PieceBuffer> {
        self.piece_buffer.take()
    }

    /// Returns the next piece to leech if the previous piece was finished and
    /// at least one peer has a missing piece.
    pub fn poll(&mut self) -> TorrentPieceHandlerState {
        if let Some(mut buffer) = self.piece_buffer.take() {
            if buffer.is_full_piece() {
                let block = match buffer.try_finalize_piece() {
                    Ok(block) => {
                        self.add_piece(block.metadata().piece_index as usize);
                        Ok(block)
                    }
                    Err(e) => Err(e),
                };
                return TorrentPieceHandlerState::Finished(block);
            } else {
                let next_block = buffer.next();
                self.piece_buffer = Some(buffer);
                return TorrentPieceHandlerState::Ready(next_block);
            }
        }

        if let Some(mut buffer) = self.next_piece_buffer() {
            let next_block = buffer.next();
            self.piece_buffer = Some(buffer);
            TorrentPieceHandlerState::Ready(next_block)
        } else {
            TorrentPieceHandlerState::WaitingPeers
        }
    }

    /// Set the index in the peer's bitfield
    ///
    /// If no `BttPeer` is currently tracked for the `peer_id` a `None` value is
    /// returned otherwise whether the `piece_index` was in bounds of the peer's
    /// bitfield.
    pub fn add_peer_piece(&mut self, peer_id: &PeerId, piece_index: usize) -> Option<bool> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            if peer.has_bitfield() {
                peer.add_piece(piece_index)
            } else {
                if piece_index < self.have.len() {
                    let mut field = self.have.clone();
                    field.clear();
                    field.set(piece_index, true);
                    peer.set_bitfield(field);
                    Some(true)
                } else {
                    Some(false)
                }
            }
        } else {
            None
        }
    }

    /// Set the piece at the index to owned.
    ///
    /// If the piece_index is out of bounds a error value is returned.
    pub fn add_piece(&mut self, piece_index: usize) -> Result<(), ()> {
        if piece_index < self.have.len() {
            self.have.set(piece_index, true);
            Ok(())
        } else {
            Err(())
        }
    }

    /// Set the piece at the index to missing.
    ///
    /// If the piece_index is out of bounds a error value is returned.
    pub fn remove_piece(&mut self, piece_index: usize) -> Result<(), ()> {
        if piece_index < self.have.len() {
            self.have.set(piece_index, false);
            Ok(())
        } else {
            Err(())
        }
    }

    /// All piece indices that are still missing
    fn missing_piece_indices(&self) -> Vec<usize> {
        self.have
            .iter()
            .enumerate()
            .filter(|(index, bit)| !*bit)
            .map(|(index, _)| index)
            .collect()
    }

    /// Selects the the next piece randomly
    fn next_random_piece(&self) -> Option<PieceBuffer> {
        let mut missing = self.missing_piece_indices();
        let mut rnd = rand::thread_rng();
        missing.shuffle(&mut rnd);

        for piece in missing {
            let peers: Option<Vec<_>> = self
                .peers
                .iter()
                .map(|(id, peer)| {
                    if peer.remote_can_seed_piece(piece) {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            if let Some(peers) = peers {
                return Some(PieceBuffer::new(
                    piece as u64,
                    self.piece_length,
                    self.id,
                    peers,
                ));
            }
        }
        None
    }

    /// Selects a randomized rare piece.
    fn next_least_common_piece(&self) -> Option<PieceBuffer> {
        let mut missing = self.missing_piece_indices();

        if !self.peers.values().any(BttPeer::remote_can_seed) {
            // no peers have the client unchoked with bitfields available
            return None;
        }

        // need to randomize otherwise a peer might get bloated with requests for the
        // same piece
        let mut rnd = rand::thread_rng();
        missing.shuffle(&mut rnd);

        let mut rarest_peers: Option<Vec<&PeerId>> = None;
        let mut rarest_piece = 0;
        for piece in missing {
            let peers: Option<Vec<_>> = self
                .peers
                .iter()
                .map(|(id, peer)| {
                    if peer.remote_can_seed_piece(piece) {
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect();

            if let Some(peers) = peers {
                if peers.len() == 1 {
                    return Some(PieceBuffer::new(
                        piece as u64,
                        self.piece_length,
                        self.id,
                        peers.into_iter().cloned().collect(),
                    ));
                }
                if let Some(rare_peer) = &rarest_peers {
                    if peers.len() < rare_peer.len() {
                        rarest_piece = piece;
                        rarest_peers = Some(peers)
                    }
                } else {
                    rarest_piece = piece;
                    rarest_peers = Some(peers)
                }
            }
        }

        rarest_peers.map(|peers| {
            PieceBuffer::new(
                rarest_piece as u64,
                self.piece_length,
                self.id,
                peers.into_iter().cloned().collect(),
            )
        })
    }

    /// Compute the next piece to leech
    fn next_piece_buffer(&mut self) -> Option<PieceBuffer> {
        match self.selection_strategy {
            PieceSelection::Random => self.next_random_piece(),
            PieceSelection::Rarest => self.next_least_common_piece(),
        }
    }
}

/// Drives a specific piece to completion.
#[derive(Debug)]
pub struct PieceBuffer {
    /// Identifier for the torrent.
    torrent_id: TorrentId,
    /// All the blocks that still need to be downloaded.
    missing_blocks: Vec<BlockMetadata>,
    /// Blocks currently waited for.
    pending_blocks: HashMap<PeerId, BlockMetadata>,
    /// The piece currently tracked.
    piece_index: u64,
    /// All the single blocks sorted by offset
    blocks: BTreeMap<u64, Block>,
    /// The peers that have the `current_piece`
    peers: Vec<PeerId>,
    /// The length of the piece all blocks belong to
    piece_length: u64,
    /// The amount of blocks in the Piece
    total_blocks: u64,
    /// The size of the last block in the piece that might be truncated.
    last_block_size: u64,
}

impl PieceBuffer {
    /// 2^14 16kb per block
    const BLOCK_SIZE: u64 = 16384;

    /// Return the next block to download if blocks are still missing and a peer
    /// is not busy
    pub fn next(&mut self) -> Option<NextBlock> {
        if let Some(peer) = self.peers.pop() {
            if let Some(block) = self.missing_blocks.pop() {
                self.pending_blocks.insert(peer.clone(), block.clone());
                return Some(NextBlock {
                    torrent_id: self.torrent_id,
                    peer,
                    block,
                });
            } else {
                self.peers.push(peer);
            }
        }
        None
    }

    pub fn remove_peer(&mut self, id: &PeerId) -> Option<PeerId> {
        let pos = self.peers.iter().position(|x| *x == *id);
        if let Some(pos) = pos {
            return Some(self.peers.remove(pos));
        }
        // check pending blocks
        if let Some((peer, block)) = self.pending_blocks.remove_entry(id) {
            self.missing_blocks.push(block);
            return Some(peer);
        }
        None
    }

    /// Whether all the buffer owns the right amount of blocks
    #[inline]
    pub fn is_full_piece(&self) -> bool {
        self.blocks.len() == self.total_blocks as usize
    }

    /// Adds the block in the buffer.
    /// If the `block`s metadata could not be validated, the block is returned
    pub fn add_block(&mut self, block: Block) -> Option<Block> {
        // TODO this needs the &PeerId of the peer that sent this block
        unimplemented!()
        //        if self.missing_blocks.remove(&block.metadata()) {
        //            self.blocks.insert(block.metadata().block_offset, block);
        //            None
        //        } else {
        //            Some(block)
        //        }
    }

    /// Turn the buffer into a single block
    pub fn try_finalize_piece(self) -> Result<Block, TorrentError> {
        let mut buffer = BytesMut::with_capacity(self.piece_length as usize);
        for block in self.blocks.values() {
            if block.is_correct_len() {
                buffer.put_slice(block);
            } else {
                return Err(TorrentError::BadPiece {
                    index: self.piece_index,
                });
            }
        }

        Ok(Block::new(
            BlockMetadata::new(self.piece_index, 0, self.piece_length as usize),
            buffer.freeze(),
        ))
    }

    /// Create a new buffer with the `NextPiece` message.
    fn new(piece_index: u64, piece_length: u64, torrent_id: TorrentId, peers: Vec<PeerId>) -> Self {
        let total_blocks = piece_length / PieceBuffer::BLOCK_SIZE;
        let mut last_block_size = piece_length % PieceBuffer::BLOCK_SIZE;

        let mut missing_blocks = (0..total_blocks).fold(
            Vec::with_capacity(total_blocks as usize),
            |mut blocks, index| {
                blocks.push(BlockMetadata::new(
                    piece_index,
                    index * PieceBuffer::BLOCK_SIZE,
                    PieceBuffer::BLOCK_SIZE as usize,
                ));
                blocks
            },
        );

        if last_block_size == 0 {
            last_block_size = PieceBuffer::BLOCK_SIZE;
        } else {
            missing_blocks.push(BlockMetadata::new(
                piece_index,
                piece_length - last_block_size,
                last_block_size as usize,
            ));
        }

        Self {
            torrent_id,
            pending_blocks: HashMap::with_capacity(peers.len()),
            missing_blocks,
            piece_index,
            blocks: Default::default(),
            piece_length: 0,
            peers,
            total_blocks,
            last_block_size,
        }
    }
}

#[derive(Debug)]
pub struct NextBlock {
    /// Identifier for the torrent.
    pub torrent_id: TorrentId,
    /// The index of the piece in the bitfield.
    pub block: BlockMetadata,
    /// The peer the block should be requested from.
    pub peer: PeerId,
}
