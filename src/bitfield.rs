use crate::pieces::PieceState;
use bit_vec::BitVec;
use bitflags::_core::ops::{Deref, DerefMut};

#[derive(Clone, Debug, PartialEq)]
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
