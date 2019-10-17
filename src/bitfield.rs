use bitflags::_core::ops::{Deref, DerefMut};

#[derive(Clone, Debug, PartialEq)]
pub struct BitField {
    inner: Vec<u32>,
}

impl Deref for BitField {
    type Target = [u32];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for BitField {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
