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
