use std::ops::{Deref, DerefMut};

use bit_vec::BitVec;

use crate::piece::PieceState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitField {
    inner: BitVec,
}

impl BitField {
    /// returns the index of the first set bit
    pub fn first_set(&self) -> Option<u32> {
        for (i, nbit) in self.inner.iter().enumerate() {
            if nbit {
                return Some(i as u32);
            }
        }
        None
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            inner: BitVec::from_bytes(bytes),
        }
    }

    /// Create a new `Bitfield` with `nbits` bits all set to true.
    pub fn new_all_set(nbits: usize) -> Self {
        BitVec::from_elem(nbits, true).into()
    }

    /// Create a new `Bitfield` with `nbits` bits all set to false.
    pub fn new_all_clear(nbits: usize) -> Self {
        BitVec::from_elem(nbits, false).into()
    }
}

impl<T: Into<BitVec>> From<T> for BitField {
    fn from(bitvec: T) -> Self {
        Self {
            inner: bitvec.into(),
        }
    }
}

impl Deref for BitField {
    type Target = BitVec;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for BitField {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
