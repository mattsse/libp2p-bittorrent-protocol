use sha1::Sha1;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct FileEntry {
    pub file_hash: Sha1,

    pub offset: u64,
}
