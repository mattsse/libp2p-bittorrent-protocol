use crate::protobuf_structs::btt as proto;
use sha1::Sha1;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq)]
pub struct MetaInfo {
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

impl Into<proto::MetaInfo> for MetaInfo {
    fn into(self) -> proto::MetaInfo {
        let mut info = proto::MetaInfo::new();
        info.set_annouce_url(self.annouce_url);
        info.set_name(self.name);
        info.set_piece_length(self.piece_length);
        info.set_length(self.length);
        info.set_pieces(::protobuf::RepeatedField::from_vec(
            self.pieces
                .into_iter()
                .map(|sha| sha.digest().bytes().to_vec())
                .collect(),
        ));
        self.file_info.set_to_proto(&mut info);
        info
    }
}

//impl fmt::Display for MetaInfo {
//    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//        unimplemented!()
//    }
//}

impl MetaInfo {
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

    /// completes the proto transformation
    pub(crate) fn set_to_proto(self, info: &mut proto::MetaInfo) {
        match self {
            FileInfo::File { length } => info.set_length(length),
            FileInfo::Dir { files } => info.set_files(::protobuf::RepeatedField::from_vec(
                files.into_iter().map(SubFileInfo::into).collect(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubFileInfo {
    /// The length of the file in bytes
    pub length: u64,
    /// A list of UTF-8 encoded strings corresponding to subdirectory names, the last of which is the actual file name.
    pub paths: Vec<String>,
}

impl Into<proto::MetaInfo_FileInfo> for SubFileInfo {
    fn into(self) -> proto::MetaInfo_FileInfo {
        let mut info = proto::MetaInfo_FileInfo::new();
        info.set_length(self.length);
        info.set_paths(::protobuf::RepeatedField::from_vec(self.paths));
        info
    }
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
