pub const BLOCK_SIZE: usize = 16 * 2 ^ 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// block of data, which is a subset of the piece specified by index.
    pub block: Vec<u8>,
}

#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PieceState {
    Missing = 0,
    Owned = 1,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IndexRange {
    pub begin: usize,
    pub end: usize,
}
