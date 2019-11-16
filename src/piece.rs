use std::collections::{HashMap, HashSet};

use crate::disk::block::BlockMetadata;
use bytes::Bytes;

/// smallest allowed piece size : 16 KB
pub const BLOCK_SIZE_MIN: usize = 16384;

/// greatest allowed piece size: 16 MB
pub const BLOCK_SIZE_MAX: usize = 16777216;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// block of data, which is a subset of the piece specified by index.
    pub block: Bytes,
}

#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PieceOwnerShip {
    Missing = 0,
    Owned = 1,
}

#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PieceSelection {
    /// select a random piece
    Random,
    /// Once peers finish downloading the current piece, it will select the next
    /// piece which is the fewest among its neighbors
    Rarest,
}

impl Default for PieceSelection {
    fn default() -> Self {
        PieceSelection::Random
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IndexRange {
    pub begin: usize,
    pub end: usize,
}

/// Stores state for the PieceChecker between invocations.
pub struct PieceCheckerState {
    new_states: Vec<PieceState>,
    old_states: HashSet<PieceState>,
    pending_blocks: HashMap<u64, Vec<BlockMetadata>>,
    total_blocks: usize,
    last_block_size: usize,
}

#[derive(PartialEq, Eq, Hash)]
pub enum PieceState {
    /// Piece was discovered as good.
    Good(u64),
    /// Piece was discovered as bad.
    Bad(u64),
}
