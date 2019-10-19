use crate::bitfield::BitField;
use chrono::NaiveDate;
use sha1::Sha1;
use std::path::PathBuf;

#[derive(Clone, Eq, PartialEq)]
pub struct MetaInfo {
    /// urls of the trackers
    pub announce: Option<String>,
    /// takes precedence over `announce`
    pub announce_list: Vec<String>,
    /// BEP-0170
    pub http_seeds: Vec<String>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub encoding: Option<String>,
    pub info: TorrentInfo,
    /// dht nodes to add to the routing table/bootstrap from
    pub nodes: Vec<DhtNode>,
    /// the hash that identifies this torrent
    pub info_hash: Sha1,
}

impl MetaInfo {
    #[inline]
    pub fn is_tracker_less(&self) -> bool {
        !self.nodes.is_empty()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DhtNode {
    pub host: String,
    pub port: u32,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TorrentInfo {
    pub num_pieces: u32,
    pub pieces: Vec<Sha1>,
    pub piece_length: u32,
    // /// only merkle tree BEP-0030
    // pub root_hash: Option<Vec<u8>>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    /// either single file or directory structure
    pub content: InfoContent,
    pub name: String,
}

impl TorrentInfo {
    #[inline]
    pub fn is_file(&self) -> bool {
        self.content.is_file()
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.content.is_dir()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoContent {
    File {
        /// length in bytes
        length: u64,
    },
    Dir {
        files: Vec<SubFileInfo>,
    },
}

impl InfoContent {
    #[inline]
    pub fn is_file(&self) -> bool {
        match self {
            InfoContent::File { .. } => true,
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

/// TODO convert to bitflag
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum TorrentType {
    MultiFile,
    Private,
    I2P,
    SSL,
}
