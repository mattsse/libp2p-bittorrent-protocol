use crate::torrent::MetaInfo;
use crate::util::ShaHash;
use sha1::Sha1;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct FileEntry {
    /// the full path of this file.
    pub path: PathBuf,
    /// a SHA-1 hash of the content of the file
    pub file_hash: Sha1,
    /// the offset of this file inside the torrent
    pub offset: u32,
    /// the size of the file (in bytes) of the file within the torrent. i.e. the sum of all the sizes of the files
    /// before it in the list.
    pub size: u32,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct FileSlice {
    /// index of the file in the torrentinfo
    pub file_index: u32,
    /// byte offset in the file where the range
    pub offset: u32,
    /// number of bytes this range is
    pub window_size: u32,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct FileStorage {
    /// the number of bytes in a regular piece
    pub piece_length: u32,
    /// the number of pieces in the torrent
    pub num_pieces: u32,
}

#[derive(Debug)]
pub struct FileHandle {
    /// the torrent's info
    meta: MetaInfo,
    files: HashMap<u32, Option<tokio_fs::File>>,
}
