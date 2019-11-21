use std::borrow::Cow;
use std::convert::TryInto;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use bendy::decoding::{Decoder, DictDecoder, ListDecoder};
use bendy::encoding::SingleItemEncoder;
use bendy::{
    decoding::{Error as DecodingError, ErrorKind, FromBencode, Object, ResultExt},
    encoding::{AsString, Error as EncodingError, ToBencode},
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use sha1::{Sha1, DIGEST_LENGTH};

use crate::bitfield::BitField;
use crate::disk::error::TorrentError;
use crate::error::Result;
use crate::util::{ShaHash, SHA_HASH_LEN};

pub mod builder;

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

    pub fn write_torrent_file<T: AsRef<Path>>(&self, path: T) -> Result<Vec<u8>> {
        let data = self.to_bencode()?;
        let mut buffer = File::create(path)?;
        buffer.write_all(&data)?;
        Ok(data)
    }

    /// time of creation based on the epoch second timestamp
    pub fn creation_time(&self) -> Option<DateTime<Utc>> {
        if let Some(seconds) = &self.creation_date {
            Some(Utc.timestamp(*seconds, 0))
        } else {
            None
        }
    }
}

impl ToBencode for MetaInfo {
    const MAX_DEPTH: usize = 3;

    fn encode(&self, encoder: SingleItemEncoder) -> Result<(), EncodingError> {
        encoder.emit_dict(|mut e| {
            if let Some(announce) = &self.announce {
                e.emit_pair(b"announce", announce)?;
            }
            if !self.announce_list.is_empty() {
                e.emit_pair(b"announce list", &self.announce_list)?;
            }
            if let Some(comment) = &self.comment {
                e.emit_pair(b"comment", comment)?;
            }
            if let Some(creation_date) = &self.creation_date {
                e.emit_pair(b"creation date", creation_date)?;
            }
            if let Some(encoding) = &self.encoding {
                e.emit_pair(b"encoding", encoding)?;
            }
            if !self.httpseeds.is_empty() {
                e.emit_pair(b"httpseeds", &self.httpseeds)?;
            }
            if !self.nodes.is_empty() {
                e.emit_pair(b"nodes", &self.nodes)?;
            }
            if !self.webseeds.is_empty() {
                e.emit_pair(b"url-list", &self.webseeds)?;
            }
            e.emit_pair(b"info", &self.info)
        })
    }
}

impl FromBencode for MetaInfo {
    fn decode_bencode_object(object: Object) -> Result<Self, DecodingError>
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
            info: info.ok_or(DecodingError::missing_field("info"))?,
            info_hash: info_hash.ok_or(DecodingError::missing_field("info"))?,
            nodes,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DhtNode {
    pub host: String,
    pub port: u32,
}

impl ToBencode for DhtNode {
    const MAX_DEPTH: usize = 1;

    fn encode(&self, encoder: SingleItemEncoder) -> Result<(), EncodingError> {
        encoder.emit_list(|e| {
            e.emit_str(&self.host)?;
            e.emit_int(self.port)
        })
    }
}

impl FromBencode for DhtNode {
    const EXPECTED_RECURSION_DEPTH: usize = 1;
    fn decode_bencode_object(object: Object) -> Result<Self, DecodingError>
    where
        Self: Sized,
    {
        let mut list = object.try_into_list()?;

        let host = list
            .next_object()?
            .ok_or(DecodingError::missing_field("host"))?;
        let host = String::decode_bencode_object(host)?;

        let port = list
            .next_object()?
            .ok_or(DecodingError::missing_field("port"))?;
        let port = u32::decode_bencode_object(port)?;

        Ok(Self { host, port })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TorrentInfo {
    pub pieces: Vec<ShaHash>,
    /// number of bytes in each piece the file is split into
    pub piece_length: u64,
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

    #[inline]
    pub fn last_piece_length(&self) -> u64 {
        self.content.length() - (self.pieces.len() as u64 - 1) * self.piece_length
    }

    /// computes the sha1 hash from the bencoded form of this info
    pub fn sha1_hash(&self) -> Result<Sha1> {
        let data = self.to_bencode()?;
        Ok(Sha1::from(&data))
    }

    #[inline]
    pub fn last_piece_size(&self) -> usize {
        (self.content.length() % self.piece_length) as usize
    }

    /// all files contained in this torrent
    pub fn all_files(&self) -> Vec<PathBuf> {
        if let InfoContent::Multi { files } = &self.content {
            files.iter().map(SubFileInfo::relative_file_path).collect()
        } else {
            vec![PathBuf::from(&self.name)]
        }
    }
}

impl ToBencode for TorrentInfo {
    const MAX_DEPTH: usize = 3;

    fn encode(&self, encoder: SingleItemEncoder) -> Result<(), EncodingError> {
        encoder.emit_dict(|mut e| {
            match &self.content {
                InfoContent::Single { length } => e.emit_pair(b"length", length),
                InfoContent::Multi { files } => e.emit_pair(b"files", files),
            }?;

            e.emit_pair(b"name", &self.name)?;
            e.emit_pair(b"piece length", &self.piece_length)?;

            let data = self
                .pieces
                .iter()
                .flat_map(|x| x.as_ref().iter())
                .map(|x| *x)
                .collect::<Vec<_>>();

            e.emit_pair(b"pieces", AsString(&data))
        })
    }
}

impl FromBencode for TorrentInfo {
    const EXPECTED_RECURSION_DEPTH: usize = 3;
    fn decode_bencode_object(object: Object) -> Result<Self, DecodingError>
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
                    piece_length = u64::decode_bencode_object(value)
                        .context("piece length")
                        .map(Some)?
                }

                (b"pieces", value) => {
                    let piece_res: Result<Vec<ShaHash>, _> = value
                        .try_into_bytes()?
                        .chunks(SHA_HASH_LEN)
                        .map(TryInto::try_into)
                        .collect();
                    pieces =
                        Some(piece_res.map_err(|_| {
                            DecodingError::unexpected_field("malformed pieces chunks")
                        })?);
                }

                (b"files", value) => {
                    if length.is_some() {
                        return return Err(DecodingError::unexpected_field("files"));
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
                        return return Err(DecodingError::unexpected_field("length"));
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
            name: name.ok_or(DecodingError::missing_field("name"))?,
            piece_length: piece_length.ok_or(DecodingError::missing_field("pieces length"))?,
            pieces: pieces.ok_or(DecodingError::missing_field("pieces"))?,
            content: content.ok_or(DecodingError::missing_field("length or files"))?,
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

    /// determines the overall length of the content
    #[inline]
    pub fn length(&self) -> u64 {
        match self {
            InfoContent::Single { length } => *length,
            InfoContent::Multi { files } => files.iter().map(|f| f.length).sum(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubFileInfo {
    /// The length of the file in bytes
    pub length: u64,
    /// A list of UTF-8 encoded strings corresponding to subdirectory names, the
    /// last of which is the actual file name.
    pub paths: Vec<String>,
}

impl SubFileInfo {
    /// the last entry in `paths` is the actual file name
    #[inline]
    pub fn file_name(&self) -> Option<&str> {
        self.paths.last().map(String::as_str)
    }

    /// returns the complete Path of the file including its parent directories,
    /// but without the root directory of the torrent
    #[inline]
    pub fn relative_file_path(&self) -> PathBuf {
        self.paths
            .iter()
            .fold(PathBuf::new(), |mut buf, path| buf.join(path))
    }
}

impl ToBencode for SubFileInfo {
    const MAX_DEPTH: usize = 2;

    fn encode(&self, encoder: SingleItemEncoder) -> Result<(), EncodingError> {
        encoder.emit_dict(|mut e| {
            e.emit_pair(b"length", &self.length)?;
            e.emit_pair(b"path", &self.paths)
        })
    }
}

impl FromBencode for SubFileInfo {
    const EXPECTED_RECURSION_DEPTH: usize = 2;
    fn decode_bencode_object(object: Object) -> Result<Self, DecodingError>
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
                    return Err(DecodingError::unexpected_field(String::from_utf8_lossy(
                        unknown_field,
                    )));
                }
            }
        }

        Ok(Self {
            length: length.ok_or(DecodingError::missing_field("length"))?,
            paths: paths.ok_or(DecodingError::missing_field("paths"))?,
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
    fn decode_torrent() {
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("resources");

        let metainfo =
            MetaInfo::from_torrent_file(dir.join("debian-9.4.0-amd64-netinst.iso.torrent"))
                .unwrap();
    }

    #[test]
    fn encode_torrent() {
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("resources");

        let mut data = Vec::new();
        let mut file = File::open(dir.join("pieces.iso")).unwrap();
        file.read_to_end(&mut data).unwrap();

        let pieces: Result<Vec<ShaHash>, _> =
            data.chunks(SHA_HASH_LEN).map(TryInto::try_into).collect();

        let info = TorrentInfo {
            piece_length: 262_144,
            pieces: pieces.unwrap(),
            name: "debian-9.4.0-amd64-netinst.iso".to_owned(),
            content: InfoContent::Single {
                length: 305_135_616,
            },
        };

        let info_hash = Sha1::from(&info.to_bencode().unwrap());

        let torrent = MetaInfo {
            announce: Some("http://bttracker.debian.org:6969/announce".to_owned()),
            announce_list: vec![],
            comment: Some("\"Debian CD from cdimage.debian.org\"".to_owned()),
            created_by: None,
            creation_date: Some(1_520_682_848),
            encoding: None,
            httpseeds: vec![
                "https://cdimage.debian.org/cdimage/release/9.4.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-9.4.0-amd64-netinst.iso".to_owned(),
                "https://cdimage.debian.org/cdimage/archive/9.4.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-9.4.0-amd64-netinst.iso".to_owned(),
            ],
            webseeds: vec![],
            info,
            nodes: vec![],
            info_hash,
        };

        let _ = torrent.write_torrent_file("dummy.torrent").unwrap();
        std::fs::remove_file("dummy.torrent").unwrap();
    }
}
