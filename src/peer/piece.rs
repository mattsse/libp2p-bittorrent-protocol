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
use crate::peer::BttPeer;
use crate::piece::PieceSelection;

/// Tracks the progress of the handler
pub enum TorrentPieceHandlerState {
    /// Wait that the state of a peer changes
    Waiting,
    /// Currently tracking the progress of a piece
    PieceNotReady,
    /// Start a new piece to leech
    Ready(NextPiece),
    /// A new piece is ready
    Finished(Result<Block, TorrentError>),
}

/// Tracks the state of a single torrent with all their remotes
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
    pub fn insert_peer(&mut self, id: PeerId, peer: BttPeer) -> Option<BttPeer> {
        self.peers.insert(id, peer)
    }

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
        if let Some(buffer) = self.piece_buffer.take() {
            if buffer.is_full_piece() {
                return TorrentPieceHandlerState::Finished(buffer.finalize_piece());
            } else {
                self.piece_buffer = Some(buffer);
                return TorrentPieceHandlerState::PieceNotReady;
            }
        }

        if let Some((buffer, piece)) = self.next_piece() {
            self.piece_buffer = Some(buffer);
            TorrentPieceHandlerState::Ready(piece)
        } else {
            TorrentPieceHandlerState::Waiting
        }
    }

    pub fn add_peer_piece(
        &mut self,
        peer_id: &PeerId,
        piece_index: usize,
    ) -> Option<Result<(), ()>> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            if peer.has_bitfield() {
                peer.add_piece(piece_index)
            } else {
                if piece_index < self.have.len() {
                    let mut field = self.have.clone();
                    field.clear();
                    field.set(piece_index, true);
                    peer.set_bitfield(field);
                    Some(Ok(()))
                } else {
                    Some(Err(()))
                }
            }
        } else {
            None
        }
    }

    /// Set the piece at the index to owned.
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
    fn next_random_piece(&self) -> Option<(PieceBuffer, NextPiece)> {
        let mut missing = self.missing_piece_indices();
        let mut rnd = rand::thread_rng();
        missing.shuffle(&mut rnd);

        for piece in missing {
            let peers: Option<Vec<_>> = self
                .peers
                .iter()
                .filter(|(_, peer)| peer.is_unchoked())
                .map(|(id, peer)| {
                    if peer.has_piece(piece).unwrap_or_default() {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect();

            if let Some(peers) = peers {
                return Some(PieceBuffer::new_with_piece(
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
    fn next_least_common_piece(&self) -> Option<(PieceBuffer, NextPiece)> {
        let mut missing = self.missing_piece_indices();

        if self
            .peers
            .values()
            .all(|x| !x.has_bitfield() || x.is_choked())
        {
            // no peers that have the client unchoked with bitfields available
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
                .filter(|(_, peer)| peer.is_unchoked())
                .map(|(id, peer)| {
                    if peer.has_piece(piece).unwrap_or_default() {
                        Some(id)
                    } else {
                        None
                    }
                })
                .collect();

            if let Some(peers) = peers {
                if peers.len() == 1 {
                    return Some(PieceBuffer::new_with_piece(
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
            PieceBuffer::new_with_piece(
                rarest_piece as u64,
                self.piece_length,
                self.id,
                peers.into_iter().cloned().collect(),
            )
        })
    }

    /// Compute the next piece to leech
    fn next_piece(&mut self) -> Option<(PieceBuffer, NextPiece)> {
        match self.selection_strategy {
            PieceSelection::Random => self.next_random_piece(),
            PieceSelection::Rarest => self.next_least_common_piece(),
        }
    }
}

pub struct PieceBuffer {
    /// All the blocks that are still missing.
    pending_blocks: HashSet<BlockMetadata>,
    /// The piece currently tracked.
    current_piece: u64,
    /// All the single blocks sorted by offset
    blocks: BTreeMap<u64, Block>,
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

    /// Whether all the buffer owns the right amount of blocks
    #[inline]
    pub fn is_full_piece(&self) -> bool {
        self.blocks.len() == self.total_blocks as usize
    }

    /// Adds the block in the buffer.
    /// If the `block`s metadata could not be validated, the block is returned
    pub fn add_block(&mut self, block: Block) -> Option<Block> {
        if self.pending_blocks.remove(&block.metadata()) {
            self.blocks.insert(block.metadata().block_offset, block);
            None
        } else {
            Some(block)
        }
    }

    /// Turn the buffer into a single block
    pub fn finalize_piece(self) -> Result<Block, TorrentError> {
        let mut buffer = BytesMut::with_capacity(self.piece_length as usize);
        for block in self.blocks.values() {
            if block.is_correct_len() {
                buffer.put_slice(block);
            } else {
                return Err(TorrentError::BadPiece {
                    index: self.current_piece,
                });
            }
        }

        Ok(Block::new(
            BlockMetadata::new(self.current_piece, 0, self.piece_length as usize),
            buffer.freeze(),
        ))
    }

    /// Create a new buffer with the `NextPiece` message.
    fn new_with_piece(
        current_piece: u64,
        piece_length: u64,
        torrent_id: TorrentId,
        peers: Vec<PeerId>,
    ) -> (Self, NextPiece) {
        let total_blocks = piece_length / PieceBuffer::BLOCK_SIZE;
        let mut last_block_size = piece_length % PieceBuffer::BLOCK_SIZE;

        let mut blocks = (0..total_blocks).fold(
            Vec::with_capacity(total_blocks as usize),
            |mut blocks, index| {
                blocks.push(BlockMetadata::new(
                    current_piece,
                    index * PieceBuffer::BLOCK_SIZE,
                    PieceBuffer::BLOCK_SIZE as usize,
                ));
                blocks
            },
        );

        if last_block_size == 0 {
            last_block_size = PieceBuffer::BLOCK_SIZE;
        } else {
            blocks.push(BlockMetadata::new(
                current_piece,
                piece_length - last_block_size,
                last_block_size as usize,
            ));
        }

        let buffer = PieceBuffer {
            pending_blocks: blocks.clone().into_iter().collect(),
            current_piece,
            blocks: Default::default(),
            piece_length: 0,
            total_blocks,
            last_block_size,
        };

        let piece = NextPiece {
            torrent_id,
            peers,
            piece_index: current_piece,
            blocks,
        };
        (buffer, piece)
    }
}

pub struct NextPiece {
    /// Identifier for the torrent.
    pub torrent_id: TorrentId,
    /// The index of the piece in the bitfield
    pub piece_index: u64,
    /// The peers that own that piece and have the client unchoked
    pub peers: Vec<PeerId>,
    /// The blocks this piece is split up
    pub blocks: Vec<BlockMetadata>,
}
