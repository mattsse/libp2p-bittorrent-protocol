use libp2p_core::multiaddr::multihash;
use sha1::{Digest, Sha1};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::Path;

/// Length of a SHA-1 hash.
pub const SHA_HASH_LEN: usize = 20;

/// SHA-1 hash wrapper type for performing operations on the hash.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ShaHash {
    hash: [u8; SHA_HASH_LEN],
}

impl ShaHash {
    /// Create a ShaHash by hashing the given bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let sha = Sha1::from(bytes);
        Self {
            hash: sha.digest().bytes(),
        }
    }

    pub fn bytes(&self) -> &[u8; 20] {
        &self.hash
    }

    pub fn random() -> Self {
        multihash::Multihash::random(multihash::Hash::SHA1)
            .digest()
            .try_into()
            .unwrap()
    }

    pub fn for_file<T: AsRef<Path>>(file: T) -> io::Result<Self> {
        let mut file = BufReader::new(fs::File::open(file)?);

        let mut hasher = Sha1::default();

        let mut buffer = [0u8; 1024];
        loop {
            let bytes_read = file.read(&mut buffer)?;
            hasher.update(&buffer[..bytes_read]);
            if bytes_read == 0 {
                break;
            }
        }
        Ok(hasher.digest().bytes().into())
    }

    #[inline]
    pub fn len() -> usize {
        SHA_HASH_LEN
    }
}

impl AsRef<[u8]> for ShaHash {
    fn as_ref(&self) -> &[u8] {
        &self.hash
    }
}

impl Into<[u8; SHA_HASH_LEN]> for ShaHash {
    fn into(self) -> [u8; SHA_HASH_LEN] {
        self.hash
    }
}

impl From<[u8; SHA_HASH_LEN]> for ShaHash {
    fn from(sha_hash: [u8; SHA_HASH_LEN]) -> ShaHash {
        ShaHash { hash: sha_hash }
    }
}

impl From<Sha1> for ShaHash {
    fn from(hasher: Sha1) -> ShaHash {
        ShaHash {
            hash: hasher.digest().bytes(),
        }
    }
}

impl TryFrom<&[u8]> for ShaHash {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let data = value.as_ref();
        if data.len() < SHA_HASH_LEN {
            Err(())
        } else {
            let hash: [u8; SHA_HASH_LEN] = data[..SHA_HASH_LEN].try_into().map_err(|_| ())?;

            Ok(Self { hash })
        }
    }
}

impl PartialEq<[u8]> for ShaHash {
    fn eq(&self, other: &[u8]) -> bool {
        let is_equal = other.len() == self.hash.len();

        self.hash
            .iter()
            .zip(other.iter())
            .fold(is_equal, |prev, (h, o)| prev && h == o)
    }
}
impl PartialEq<Sha1> for ShaHash {
    fn eq(&self, other: &Sha1) -> bool {
        self.hash == other.digest().bytes()
    }
}

impl fmt::Display for ShaHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for i in self.hash.iter() {
            write!(f, "{:08x}", i)?
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_sha1() {
        let _ = ShaHash::random();
    }
}
