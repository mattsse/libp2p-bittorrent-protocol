use crate::bitfield::BitField;
use crate::error::Result;
use bendy::decoding::{Decoder, DictDecoder, ListDecoder};
use bendy::{
    decoding::{Error as BError, ErrorKind, FromBencode, Object, ResultExt},
    encoding::{AsString, ToBencode},
};
use chrono::NaiveDate;
use sha1::{Sha1, DIGEST_LENGTH};
use std::convert::TryInto;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Eq, PartialEq)]
pub struct MetaInfo {
    /// urls of the trackers
    pub announce: Option<String>,
    /// takes precedence over `announce`
    pub announce_list: Vec<String>,
    pub encoding: Option<String>,
    /// BEP-0170
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub httpseeds: Vec<String>,
    pub webseeds: Vec<String>,
    pub info: TorrentInfo,
    /// dht nodes to add to the routing table/bootstrap from
    pub nodes: Vec<DhtNode>,
    /// the hash that identifies this torrent
    pub info_hash: Sha1,
}

impl fmt::Debug for MetaInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MetaInfo {{ annouce: {:?}, announce_list: {:?}, encoding: {:?}, comment: {:?}, created by: {:?}, creation date: {:?}, httpseeds: {:?}, webseeds: {:?}, info: {:?}, nodes: {:?}, info_hash: {} }}", self.announce, self.announce_list, self.encoding, self.comment, self.created_by, self.creation_date, self.httpseeds, self.webseeds, self.info, self.nodes, self.info_hash.digest().to_string())
    }
}

impl MetaInfo {
    #[inline]
    pub fn is_tracker_less(&self) -> bool {
        !self.nodes.is_empty()
    }

    pub fn from_torrent_file<T: AsRef<Path>>(path: T) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        Ok(Self::from_bencode(&data)?)
    }
}

impl FromBencode for MetaInfo {
    fn decode_bencode_object(object: Object) -> Result<Self, BError>
    where
        Self: Sized,
    {
        let mut announce = None;
        let mut announce_list = Vec::new();
        let mut webseeds = Vec::new();
        let mut httpseeds = Vec::new();
        let mut encoding = None;
        let mut comment = None;
        let mut created_by = None;
        let mut creation_date = None;
        let mut info = None;
        let mut info_hash = None;
        let mut nodes = Vec::new();

        let mut dict = object.try_into_dictionary()?;
        while let Some(pair) = dict.next_pair()? {
            match pair {
                (b"announce", value) => {
                    announce = String::decode_bencode_object(value)
                        .context("announce")
                        .map(Some)?
                }
                (b"announce_list", value) => {
                    let mut list = value.try_into_list()?;
                    while let Some(url) = list.next_object()? {
                        announce_list.push(String::decode_bencode_object(url)?);
                    }
                }
                (b"url-list", value) => {
                    let mut list = value.try_into_list()?;
                    while let Some(url) = list.next_object()? {
                        webseeds.push(String::decode_bencode_object(url)?);
                    }
                }
                (b"httpseeds", value) => {
                    let mut list = value.try_into_list()?;
                    while let Some(url) = list.next_object()? {
                        httpseeds.push(String::decode_bencode_object(url)?);
                    }
                }
                (b"nodes", value) => {
                    let mut list = value.try_into_list()?;
                    while let Some(node) = list.next_object()? {
                        nodes.push(DhtNode::decode_bencode_object(node)?);
                    }
                }

                (b"comment", value) | (b"comment.utf-8", value) => {
                    comment = String::decode_bencode_object(value)
                        .context("comment")
                        .map(Some)?
                }
                (b"creation date", value) => {
                    creation_date = i64::decode_bencode_object(value)
                        .context("creation date")
                        .map(Some)?
                }
                (b"encoding", value) => {
                    encoding = String::decode_bencode_object(value)
                        .context("encoding")
                        .map(Some)?
                }
                (b"created by", value) | (b"created by.utf-8", value) => {
                    created_by = String::decode_bencode_object(value)
                        .context("created by")
                        .map(Some)?
                }
                (b"info", value) => {
                    let mut dict = value.try_into_dictionary()?;

                    let data = dict.into_raw()?;
                    info_hash = Some(Sha1::from(data));

                    info = TorrentInfo::from_bencode(data).context("info").map(Some)?
                }
                (unknown_field, _) => {
                    debug!("skipping unknown filed {:?}", unknown_field);
                }
            }
        }

        Ok(Self {
            announce,
            announce_list,
            webseeds,
            httpseeds,
            encoding,
            comment,
            creation_date,
            created_by,
            info: info.ok_or(BError::missing_field("info"))?,
            info_hash: info_hash.ok_or(BError::missing_field("info"))?,
            nodes,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DhtNode {
    pub host: String,
    pub port: u32,
}

impl FromBencode for DhtNode {
    const EXPECTED_RECURSION_DEPTH: usize = 1;
    fn decode_bencode_object(object: Object) -> Result<Self, BError>
    where
        Self: Sized,
    {
        let mut list = object.try_into_list()?;

        let host = list.next_object()?.ok_or(BError::missing_field("host"))?;
        let host = String::decode_bencode_object(host)?;

        let port = list.next_object()?.ok_or(BError::missing_field("port"))?;
        let port = u32::decode_bencode_object(port)?;

        Ok(Self { host, port })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TorrentInfo {
    pub pieces: Vec<[u8; DIGEST_LENGTH]>,
    /// number of bytes in each piece the file is split into
    pub piece_length: u32,
    // /// only merkle tree BEP-0030
    // pub root_hash: Option<Vec<u8>>,
    /// either single file or directory structure
    pub content: InfoContent,
    /// name of the file or directory
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

impl FromBencode for TorrentInfo {
    const EXPECTED_RECURSION_DEPTH: usize = 3;
    fn decode_bencode_object(object: Object) -> Result<Self, BError>
    where
        Self: Sized,
    {
        let mut name = None;
        let mut pieces = None;
        let mut piece_length = None;
        let mut files = None;
        let mut length = None;

        let mut dict = object.try_into_dictionary()?;

        while let Some(pair) = dict.next_pair()? {
            match pair {
                (b"name", value) | (b"name.utf-8", value) => {
                    name = String::decode_bencode_object(value)
                        .context("name")
                        .map(Some)?
                }
                (b"piece length", value) => {
                    piece_length = u32::decode_bencode_object(value)
                        .context("piece length")
                        .map(Some)?
                }

                (b"pieces", value) => {
                    let piece_res: Result<Vec<[u8; DIGEST_LENGTH]>, _> = value
                        .try_into_bytes()?
                        .chunks(20)
                        .map(TryInto::try_into)
                        .collect();
                    pieces = Some(piece_res?);
                }

                (b"files", value) => {
                    if length.is_some() {
                        return return Err(BError::unexpected_field("files"));
                    }
                    let mut sub_files = Vec::new();
                    let mut list = value.try_into_list()?;
                    while let Some(file) = list.next_object()? {
                        sub_files.push(SubFileInfo::decode_bencode_object(file)?);
                    }
                    files = Some(sub_files);
                }
                (b"length", value) => {
                    if files.is_some() {
                        return return Err(BError::unexpected_field("length"));
                    }
                    length = u64::decode_bencode_object(value)
                        .context("length")
                        .map(Some)?
                }
                (unknown_field, _) => {
                    debug!("skipping unknown filed {:?}", unknown_field);
                }
            }
        }

        let content = if let Some(length) = length {
            Some(InfoContent::Single { length })
        } else {
            if let Some(files) = files {
                Some(InfoContent::Multi { files })
            } else {
                None
            }
        };

        Ok(Self {
            name: name.ok_or(BError::missing_field("name"))?,
            piece_length: piece_length.ok_or(BError::missing_field("pieces length"))?,
            pieces: pieces.ok_or(BError::missing_field("pieces"))?,
            content: content.ok_or(BError::missing_field("length or files"))?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoContent {
    Single {
        /// length in bytes
        length: u64,
    },
    Multi {
        files: Vec<SubFileInfo>,
    },
}

impl InfoContent {
    #[inline]
    pub fn is_file(&self) -> bool {
        match self {
            InfoContent::Single { .. } => true,
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

impl FromBencode for SubFileInfo {
    const EXPECTED_RECURSION_DEPTH: usize = 2;
    fn decode_bencode_object(object: Object) -> Result<Self, BError>
    where
        Self: Sized,
    {
        let mut length = None;
        let mut paths = None;

        let mut dict = object.try_into_dictionary()?;
        while let Some(pair) = dict.next_pair()? {
            match pair {
                (b"length", value) => {
                    length = u64::decode_bencode_object(value)
                        .context("length")
                        .map(Some)?
                }
                (b"path", value) => {
                    let mut path_list = Vec::new();
                    let mut list = value.try_into_list()?;

                    while let Some(path) = list.next_object()? {
                        path_list.push(String::decode_bencode_object(path)?);
                    }
                    paths = Some(path_list);
                }
                (unknown_field, _) => {
                    return Err(BError::unexpected_field(String::from_utf8_lossy(
                        unknown_field,
                    )));
                }
            }
        }

        Ok(Self {
            length: length.ok_or(BError::missing_field("length"))?,
            paths: paths.ok_or(BError::missing_field("paths"))?,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_torrent() {
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("resources");

        let mut file = File::open(dir.join("debian-9.4.0-amd64-netinst.iso.torrent")).unwrap();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();

        let metainfo = MetaInfo::from_bencode(&data).unwrap();
    }
}
