use sha1::Sha1;

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct FileEntry {
    pub file_hash: Sha1,

    pub offset: u64,
}

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct FileSlice {
    /// index of the file in the torrentinfo
    pub file_index: u32,
    /// byte offset in the file where the range
    pub offset: u32,
    /// number of bytes this range is
    pub window_size: u32,
}
