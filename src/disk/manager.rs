use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use bit_vec::BitBlock;
use fnv::{FnvHashMap, FnvHashSet};
use futures::future::Either;
use futures::{Async, AsyncSink, Future, Poll, Sink, StartSend, Stream};
use lru_cache::LruCache;
use tokio_fs::file::{OpenFuture, SeekFuture};
use tokio_fs::OpenOptions;

use crate::util::ShaHash;

use crate::behavior::BittorrentEvent::LeechBlockResult;
use crate::disk::block::{
    Block,
    BlockFileRead,
    BlockFileWrite,
    BlockIn,
    BlockMetadata,
    BlockMut,
    BlockRead,
    BlockWrite,
};
use crate::disk::error::TorrentError;
use crate::disk::file::{FileEntry, TorrentFile, TorrentFileId};
use crate::disk::fs::FileSystem;
use crate::disk::message::{DiskMessageIn, DiskMessageOut};
use crate::disk::native::NativeFileSystem;
use crate::peer::torrent::TorrentId;
use crate::torrent::MetaInfo;

/// `DiskManager` object which handles the storage of `Blocks` to the
/// `FileSystem`.
pub struct DiskManager<TFileSystem: FileSystem> {
    active_blocks: Vec<BlockIn<TFileSystem::File>>,
    file_cache: LruCache<TorrentFileId, FileState<TFileSystem::File>>,
    torrents: FnvHashSet<TorrentFile>,
    file_system: TFileSystem,
    queued_events: VecDeque<DiskMessageIn>,
}

impl<TFileSystem: FileSystem> DiskManager<TFileSystem> {
    pub fn with_capacity(file_system: TFileSystem, capacity: usize) -> Self {
        DiskManager {
            active_blocks: Default::default(),
            file_cache: LruCache::new(capacity),
            torrents: FnvHashSet::default(),
            file_system,
            queued_events: VecDeque::new(),
        }
    }

    pub fn with_native_fs<T: AsRef<Path>>(dir: T) -> DiskManager<NativeFileSystem> {
        DiskManager::with_capacity(NativeFileSystem::from(dir), 100)
    }

    /// queue in a new block to write to disk
    pub fn write_block(&mut self, torrent_id: TorrentId, block: Block) {
        self.queued_events
            .push_back(DiskMessageIn::WriteBlock((torrent_id, block)));
    }

    /// fill a new block from a seed
    pub fn read_block(&mut self, torrent_id: TorrentId, metadata: BlockMetadata) {
        self.queued_events
            .push_back(DiskMessageIn::ReadBlock(torrent_id, metadata));
    }

    /// removes the torrent, means future request targeting to read/load a block
    /// from the torrent won't succeed
    fn remove_torrent(&mut self, id: TorrentId) -> DiskMessageOut {
        if let Some(file) = self.torrents.take(&id) {
            DiskMessageOut::TorrentRemoved(id, file.meta_info)
        } else {
            DiskMessageOut::TorrentError(id, TorrentError::TorrentNotFound { id })
        }
    }

    /// stores a new torrent
    pub fn add_torrent(&mut self, id: TorrentId, meta: MetaInfo) {
        self.queued_events
            .push_back(DiskMessageIn::AddTorrent(id, meta))
    }

    fn sync_torrent(&mut self, id: TorrentId) {
        if let Some(meta) = self.torrents.get(&id) {}
        unimplemented!()
    }

    fn get_torrent_files(&self, id: &TorrentId) -> Option<&HashSet<FileEntry>> {
        if let Some(file) = self.torrents.get(id) {
            Some(&file.files)
        } else {
            None
        }
    }

    fn add_block_read(
        &mut self,
        id: TorrentId,
        meta: BlockMetadata,
    ) -> Result<Async<DiskMessageOut>, TFileSystem::Error> {
        if let Some(torrent) = self.torrents.get(&id) {
            match torrent.files_for_block(meta.clone()) {
                Ok(files) => {
                    assert!(!files.is_empty());

                    let mut queued_files = Vec::with_capacity(files.len());
                    let mut all_ready = true;

                    for (file, metadata) in files {
                        if let Some(state) = self.file_cache.remove(&file.id) {
                            match state {
                                FileState::Ready(fs_file) => {
                                    queued_files.push((file.id, fs_file, metadata));
                                }
                                FileState::Queued(fs_file, queued_meta) => {
                                    if metadata != queued_meta {
                                        all_ready = false;
                                    }
                                    queued_files.push((file.id, fs_file, metadata));
                                }
                                FileState::Busy(piece) => {
                                    all_ready = false;
                                    self.file_cache.insert(file.id, FileState::Busy(piece));
                                }
                            }
                        } else {
                            // file not available yet
                            if let Async::Ready(fs_file) =
                                self.file_system.poll_open_file(&file.path)?
                            {
                                queued_files.push((file.id, fs_file, metadata));
                            } else {
                                all_ready = false;
                                break;
                            }
                        }
                    }
                    if all_ready {
                        let mut block = if queued_files.len() == 1 {
                            let (id, fs, meta) = queued_files.remove(0);
                            self.file_cache
                                .insert(id, FileState::Busy(meta.piece_index));
                            BlockIn::Read(BlockRead::Single(BlockFileRead::new(id, fs, meta)))
                        } else {
                            let mut overlap = Vec::with_capacity(queued_files.len());

                            for (id, fs, meta) in queued_files {
                                overlap.push(BlockFileRead::new(id, fs, meta));
                                self.file_cache
                                    .insert(id, FileState::Busy(meta.piece_index));
                            }

                            BlockIn::Read(BlockRead::Overlap(overlap))
                        };

                        if block.poll(&mut self.file_system)?.is_ready() {
                            // TODO
                            panic!()
                        } else {
                            self.active_blocks.push(block);

                            Ok(Async::NotReady)
                        }
                    } else {
                        // add removed states back, but queued
                        for (id, fs, meta) in queued_files {
                            self.file_cache.insert(id, FileState::Queued(fs, meta));
                        }
                        self.queued_events
                            .push_back(DiskMessageIn::ReadBlock(id, meta));
                        Ok(Async::NotReady)
                    }
                }
                Err(e) => Ok(Async::Ready(DiskMessageOut::TorrentError(id, e))),
            }
        } else {
            Ok(Async::Ready(DiskMessageOut::TorrentError(
                id,
                TorrentError::TorrentNotFound { id },
            )))
        }
    }
}

impl<TFileSystem: FileSystem> Future for DiskManager<TFileSystem> {
    type Item = DiskMessageOut;
    type Error = TFileSystem::Error;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        // remove each `block`s one by one, update file state or add them back
        for b in (0..self.active_blocks.len()).rev() {
            let mut block = self.active_blocks.swap_remove(b);
            if block.poll(&mut self.file_system)?.is_ready() {
                // TODO convert to diskmessage and add files back to cache
                //                match block.into_block_out() {
                //
                //                }
            } else {
                self.active_blocks.push(block);
            }
        }

        loop {
            if let Some(msg) = self.queued_events.pop_front() {
                match msg {
                    DiskMessageIn::AddTorrent(id, meta) => {
                        let msg = if self.torrents.contains(&id) {
                            DiskMessageOut::TorrentError(id, TorrentError::ExistingTorrent { meta })
                        } else {
                            self.torrents.insert(TorrentFile::new(id, meta));
                            DiskMessageOut::TorrentAdded(id)
                        };

                        return Ok(Async::Ready(msg));
                    }
                    DiskMessageIn::RemoveTorrent(id) => {
                        return Ok(Async::Ready(self.remove_torrent(id)));
                    }
                    DiskMessageIn::SyncTorrent(id) => {}
                    DiskMessageIn::ReadBlock(id, metadata) => {
                        //                        let block = BlockIn::Read(
                        //                            BlockRead {
                        //
                        //                            }
                        //                        );
                    }
                    _ => panic!(),
                }
            } else {
                break;
            }
            // TODO loop through all messages
            // try to poll pending diskmessages directly
            // skip busy files

            //                if let Some(mut handles) =
            // self.files.get_mut(&block.piece_index()) {
            //
            //                    for h in (0..handles.len()).rev() {
            //                        let mut block = handles.swap_remove(h) {
            //
            //                        }
            //                    }
            //                } else {
            //                    let (hash, files) =
            //
            // self.meta.info.files_for_piece_index(block.piece_index())?;

            //                }

            // TODO validate the block if its complete
        }

        if self.queued_events.is_empty() {
            return Ok(Async::NotReady);
        }

        Ok(Async::NotReady)
    }
}

pub enum FileState<TFile> {
    Queued(TFile, BlockMetadata),
    Ready(TFile),
    Busy(u64),
}
