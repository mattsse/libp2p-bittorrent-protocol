use sha1::Sha1;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq)]
pub struct TorrentInfo {
    /// The URL of the tracker
    pub annouce_url: String,
    /// suggested name to save the file (or directory)
    pub name: String,
    /// number of bytes in each piece the file is split into
    /// as power of two 2^14 (16 kb)
    pub piece_length: u32,
    /// each is a SHA1 hash of the piece at the corresponding index
    pub pieces: Vec<Sha1>,
    /// the length of the
    pub length: u64,
    /// either single file or directory structure
    pub file_info: FileInfo,
}

impl TorrentInfo {
    #[inline]
    pub fn is_file(&self) -> bool {
        self.file_info.is_file()
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.file_info.is_dir()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileInfo {
    File {
        /// length in bytes
        length: u64,
    },
    Dir {
        files: Vec<SubFileInfo>,
    },
}

impl FileInfo {
    #[inline]
    pub fn is_file(&self) -> bool {
        match self {
            FileInfo::File { .. } => true,
            _ => false,
        }
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        !self.is_file()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubFileInfo {
    /// The length of the file in bytes
    pub length: u64,
    /// A list of UTF-8 encoded strings corresponding to subdirectory names, the last of which is the actual file name.
    pub paths: Vec<String>,
}

impl SubFileInfo {
    /// the last entry in `paths` is the actual file name
    #[inline]
    pub fn file_name(&self) -> Option<&str> {
        self.paths.last().map(String::as_str)
    }

    /// returns the complete Path of the file including all parent directories
    #[inline]
    pub fn file_path(&self) -> PathBuf {
        self.paths
            .iter()
            .fold(PathBuf::new(), |mut buf, path| buf.join(path))
    }
}
