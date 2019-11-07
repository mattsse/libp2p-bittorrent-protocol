use crate::bitfield::BitField;
use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::peer::BttPeer;
use crate::piece::PieceSelection;
use fnv::FnvHashMap;
use libp2p_core::PeerId;
use rand::{
    self,
    distributions::{Distribution, Uniform},
};
use std::collections::{BTreeMap, HashMap};
use wasm_timer::Instant;

/// tracks the state of a single torrent with all their remotes
pub struct TorrentPieceHandler {
    /// How the next piece is selected
    selection_strategy: PieceSelection,
    /// The downloaded blocks for a torrent
    block_buffer: PieceBuffer,
    /// Which pieces the client owns and lacks
    have: BitField,
    /// All the peers related to the torrent.
    pub peers: FnvHashMap<PeerId, BttPeer>,
    /// Tracks the state for the state
    state: PieceState,
    /// Whether client is in endgame mode
    endgame: bool,
}

impl TorrentPieceHandler {
    /// Advances the state of the torrent's pieces.
    pub fn poll(&mut self, now: Instant) {
        unimplemented!()
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
    fn next_random_piece(&self) -> Result<usize, ()> {
        let missing = self.missing_piece_indices();
        if missing.is_empty() {
            return Err(());
        }
        let mut rng = rand::thread_rng();
        let die = Uniform::from(0..missing.len());
        let next = die.sample(&mut rng);
        Ok(missing[next])
    }

    /// Selects the next piece based on how many peers own a piece
    fn next_rarest_piece(&self) -> Result<usize, ()> {
        let missing = self.missing_piece_indices();
        if missing.is_empty() {
            return Err(());
        }
        let peer_iter = self
            .peers
            .values()
            .filter(|x| x.piece_field.is_some() && x.is_unchoked());

        let mut peer_ctn = peer_iter.clone().count();
        if peer_ctn == 0 {
            // no peers that have the client unchoked with bitfields available
            return Err(());
        }
        let mut rarest = 0;
        for piece in missing {
            let mut piece_occurrences = 0;
            for peer in peer_iter.clone() {
                if let Some(field) = &peer.piece_field {
                    if let Some(bit) = field.get(piece) {
                        // the peer owns the piece
                        piece_occurrences += 1;
                    }
                }
            }
            if piece_occurrences == peer_ctn {
                // finished, all peers own the piece
                return Ok(piece);
            }
            if piece_occurrences > rarest {
                rarest = piece_occurrences;
            }
        }

        Ok(rarest)
    }

    /// Compute the next piece to leech
    fn next_piece(&mut self) -> Result<usize, ()> {
        match self.selection_strategy {
            PieceSelection::Random => self.next_random_piece(),
            PieceSelection::Rarest => self.next_rarest_piece(),
        }
    }
}

/// Stores state for the piece to leech.
pub struct PieceState {
    /// The piece currently tracked
    current_piece: usize,
    /// All the blocks that are still missing
    pending_blocks: HashMap<usize, Vec<BlockMetadata>>,
    /// Amount of blocks the piece has
    total_blocks: usize,
    /// The size of the last block in the piece that might be truncated.
    last_block_size: usize,
}

pub struct PieceBuffer {
    /// all the data, not necessarily in correct order
    blocks: Vec<BlockMut>,
    /// the length of the piece all blocks belong to
    piece_length: usize,
    /// The amount of blocks in the Piece
    total_blocks: usize,
}

impl PieceBuffer {
    /// Whether all the buffer owns the right amount of blocks
    #[inline]
    pub fn is_full_piece(&self) -> bool {
        self.blocks.len() == self.total_blocks
    }
}
