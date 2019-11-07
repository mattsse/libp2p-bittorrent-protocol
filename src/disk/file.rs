use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use futures::{Async, Future};
use sha1::Sha1;
use tokio_fs::file::{OpenFuture, SeekFuture};
use tokio_fs::OpenOptions;

use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::disk::message::{DiskMessageIn, DiskMessageOut};
use crate::peer::piece::TorrentId;
use crate::torrent::{InfoContent, MetaInfo};
use crate::util::ShaHash;

#[derive(Debug, Eq, PartialEq)]
pub struct TorrentFile {
    pub id: TorrentId,
    pub meta_info: MetaInfo,
    pub files: HashSet<FileEntry>,
}

impl TorrentFile {
    pub fn new(id: TorrentId, meta_info: MetaInfo) -> Self {
        let root_dir = PathBuf::from(&meta_info.info.name);

        let files = match &meta_info.info.content {
            InfoContent::Single { length } => {
                let mut hs = HashSet::with_capacity(1);
                hs.insert(FileEntry {
                    path: root_dir,
                    id: TorrentFileId(id.0, 0),
                    offset: 0,
                    size: *length,
                });
                hs
            }
            InfoContent::Multi { files } => {
                let (_, _, files) = files.iter().fold(
                    (0, 0, HashSet::with_capacity(files.len())),
                    |(file_index, offset, mut files), file| {
                        let entry = FileEntry {
                            id: TorrentFileId(id.0, file_index),
                            offset,
                            path: root_dir.join(file.relative_file_path()),
                            size: file.length,
                        };

                        files.insert(entry);
                        (file_index + 1, offset + file.length, files)
                    },
                );
                files
            }
        };

        Self {
            id,
            meta_info,
            files,
        }
    }

    /// returns the `FileEntry` that matches the `path`
    pub fn get_file<T: AsRef<Path>>(&self, path: T) -> Option<&FileEntry> {
        self.files.get(path.as_ref())
    }

    /// return all files that are related to the targeted block
    /// in case there is an overlap, multi entries with their adjusted metadatas
    /// are returned
    pub fn files_for_block(
        &self,
        metadata: BlockMetadata,
    ) -> Result<Vec<(&FileEntry, BlockMetadata)>, TorrentError> {
        if (metadata.piece_index as usize) < self.meta_info.info.pieces.len()
            && (metadata.block_offset + metadata.block_length as u64)
                <= self.meta_info.info.piece_length
        {
            // offset where the requested piece starts inside the torrent
            let piece_start = self.meta_info.info.piece_length * metadata.piece_index;
            // offset for the requested block inside the piece
            let (block_start, block_end) = (
                piece_start + metadata.block_offset,
                piece_start + metadata.block_offset + metadata.block_length as u64,
            );

            let mut files = Vec::new();

            for file in &self.files {
                if block_start >= file.offset {
                    if file.offset + file.size >= block_end {
                        // whole block inside this file
                        return Ok(vec![(file, metadata)]);
                    } else {
                        // block overlap in next file
                        let meta = BlockMetadata::new(
                            metadata.piece_index,
                            metadata.block_offset,
                            (file.offset + file.size - block_start) as usize,
                        );
                        files.push((file, meta));
                    }
                } else {
                    // block starts before file and ends in range
                    if file.size + file.offset <= block_end {
                        let meta = BlockMetadata::new(
                            metadata.piece_index,
                            file.offset,
                            (block_end - file.offset + file.size) as usize,
                        );
                        files.push((file, meta));
                    }
                }
            }
            Ok(files)
        } else {
            Err(TorrentError::BadPiece {
                index: metadata.piece_index,
            })
        }
    }
}

impl Hash for TorrentFile {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Borrow<TorrentId> for TorrentFile {
    fn borrow(&self) -> &TorrentId {
        &self.id
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileEntry {
    /// the relatve  path of this file.
    pub path: PathBuf,
    /// index of the file in the torrentinfo
    pub id: TorrentFileId,
    /// the offset of this file inside the torrent
    pub offset: u64,
    /// the size of the file (in bytes) of the file within the torrent. i.e. the
    /// sum of all the sizes of the files before it in the list.
    pub size: u64,
}

impl Hash for FileEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl Borrow<Path> for FileEntry {
    fn borrow(&self) -> &Path {
        &self.path
    }
}

impl Borrow<TorrentFileId> for FileEntry {
    fn borrow(&self) -> &TorrentFileId {
        &self.id
    }
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

/// Unique identifier for an active Torrent operation.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct FileId(pub usize);

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct TorrentFileId(usize, usize);
