use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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

use crate::disk::block::{
    Block,
    BlockFile,
    BlockFileRead,
    BlockFileWrite,
    BlockIn,
    BlockMetadata,
    BlockMut,
    BlockRead,
    BlockResult,
    BlockWrite,
};
use crate::disk::error::TorrentError;
use crate::disk::error::TorrentError::TorrentNotFound;
use crate::disk::file::{FileEntry, FileWindow, TorrentFile, TorrentFileId};
use crate::disk::fs::FileSystem;
use crate::disk::message::{DiskMessageIn, DiskMessageOut};
use crate::disk::native::NativeFileSystem;
use crate::peer::torrent::TorrentId;
use crate::piece::PieceState;
use crate::torrent::MetaInfo;
use bytes::Bytes;
use std::ops::Deref;

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
            .push_back(DiskMessageIn::WriteBlock(torrent_id, block));
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
        self.queued_events.push_back(DiskMessageIn::SyncTorrent(id))
    }

    fn get_torrent_files(&self, id: &TorrentId) -> Option<&HashSet<FileEntry>> {
        if let Some(file) = self.torrents.get(id) {
            Some(&file.files)
        } else {
            None
        }
    }

    // TODO dont sort by block offset in case of multiple files,
    // use a normal vector instead
    fn poll_files(
        &mut self,
        id: TorrentId,
        meta: BlockMetadata,
    ) -> Result<
        Async<Result<Vec<(FileWindow, BlockFile<TFileSystem::File>)>, TorrentError>>,
        TFileSystem::Error,
    > {
        if let Some(torrent) = self.torrents.get(&id) {
            match torrent.files_for_block(meta.clone()) {
                Ok(files) => {
                    assert!(!files.is_empty());

                    let mut queued_files = Vec::with_capacity(files.len());
                    let mut all_ready = true;

                    for (file, window) in files {
                        if let Some(state) = self.file_cache.remove(&file.id) {
                            match state {
                                FileState::Ready(fs_file) => {
                                    queued_files.push((file.id, fs_file, window));
                                }
                                FileState::Queued(fs_file, queued_window) => {
                                    if window != queued_window {
                                        all_ready = false;
                                    }
                                    queued_files.push((file.id, fs_file, window));
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
                                queued_files.push((file.id, fs_file, window));
                            } else {
                                all_ready = false;
                                break;
                            }
                        }
                    }
                    if all_ready {
                        // multiple files for a block needed
                        let mut blocks = Vec::with_capacity(queued_files.len());

                        for (id, file, window) in queued_files {
                            blocks.push((window, BlockFile::new(id, file)));

                            self.file_cache.insert(id, FileState::Busy(window));
                        }

                        Ok(Async::Ready(Ok(blocks)))
                    } else {
                        // add removed states back, but queued
                        for (id, file, window) in queued_files {
                            self.file_cache.insert(id, FileState::Queued(file, window));
                        }
                        Ok(Async::NotReady)
                    }
                }
                Err(e) => {
                    debug!("No torrent files for block available.");
                    Ok(Async::Ready(Err(e)))
                }
            }
        } else {
            Ok(Async::Ready(Err(TorrentError::TorrentNotFound { id })))
        }
    }

    fn is_good_piece(&self, id: TorrentId, block: &Block) -> bool {
        if let Some(torrent) = self.torrents.get(&id) {
            torrent.is_good_piece(block)
        } else {
            false
        }
    }

    fn poll_create_block_write(
        &mut self,
        id: TorrentId,
        block: Block,
    ) -> Result<Async<Result<BlockIn<TFileSystem::File>, TorrentError>>, TFileSystem::Error> {
        // validate piece hash
        if !self.is_good_piece(id, &block) {
            error!("Piece hash mismatch {:?}", block.len());
            return Ok(Async::Ready(Err(TorrentError::BadPiece {
                index: block.metadata().piece_index,
            })));
        }

        if let Async::Ready(res) = self.poll_files(id, block.metadata())? {
            match res {
                Ok(files) => {
                    if files.len() == 1 {
                        let (window, file) = files.into_iter().next().unwrap();

                        Ok(Async::Ready(Ok(BlockIn::Write(BlockWrite::Single {
                            metadata: block.metadata(),
                            block: BlockFileWrite::new(file, block.into_bytes(), window),
                        }))))
                    } else {
                        let mut blocks = Vec::with_capacity(files.len());
                        let metadata = block.metadata();

                        for (window, file) in files {
                            // adjust blocks
                            let fragment =
                                Bytes::from(&block[window.offset as usize..window.length as usize]);

                            blocks.push(BlockFileWrite::new(file, fragment, window));
                        }
                        let block = BlockIn::Write(BlockWrite::Overlap {
                            torrent: id,
                            blocks,
                            metadata,
                        });
                        Ok(Async::Ready(Ok(block)))
                    }
                }
                Err(err) => {
                    debug!("No files found to write piece to {:?}", err);
                    Ok(Async::Ready(Err(err)))
                }
            }
        } else {
            self.queued_events
                .push_back(DiskMessageIn::WriteBlock(id, block));
            Ok(Async::NotReady)
        }
    }

    fn poll_create_block_read(
        &mut self,
        id: TorrentId,
        meta: BlockMetadata,
    ) -> Result<Async<Result<BlockIn<TFileSystem::File>, TorrentError>>, TFileSystem::Error> {
        if let Async::Ready(res) = self.poll_files(id, meta.clone())? {
            match res {
                Ok(files) => {
                    if files.len() == 1 {
                        let (window, file) = files.into_iter().next().unwrap();
                        Ok(Async::Ready(Ok(BlockIn::Read(BlockRead::Single {
                            metadata: meta,
                            block: BlockFileRead::new(file, window),
                        }))))
                    } else {
                        let mut blocks = Vec::with_capacity(files.len());

                        for (window, file) in files {
                            blocks.push(BlockFileRead::new(file, window));
                        }
                        let block = BlockIn::Read(BlockRead::Overlap {
                            torrent: id,
                            blocks,
                            metadata: meta,
                        });
                        Ok(Async::Ready(Ok(block)))
                    }
                }
                Err(err) => Ok(Async::Ready(Err(err))),
            }
        } else {
            self.queued_events
                .push_back(DiskMessageIn::ReadBlock(id, meta));
            Ok(Async::NotReady)
        }
    }

    fn finalize_block(&mut self, block: BlockIn<TFileSystem::File>) -> DiskMessageOut {
        match block.finalize() {
            BlockResult::Read {
                torrent,
                result,
                files,
            } => {
                for file in files {
                    self.file_cache
                        .insert(file.id, FileState::Ready(file.inner));
                }
                match result {
                    Ok(block) => DiskMessageOut::BlockRead(torrent, block),
                    Err(meta) => {
                        debug!("Failed to finalize read block {:?}", meta);
                        DiskMessageOut::ReadBlockError(torrent, TorrentError::BadBlock { meta })
                    }
                }
            }
            BlockResult::Write {
                torrent,
                result,
                files,
            } => {
                for file in files {
                    self.file_cache
                        .insert(file.id, FileState::Ready(file.inner));
                }
                match result {
                    Ok(block) => DiskMessageOut::BlockWritten(torrent, block),
                    Err(metadata) => DiskMessageOut::WriteBlockError(
                        torrent,
                        TorrentError::BadPiece {
                            index: metadata.piece_index,
                        },
                    ),
                }
            }
        }
    }
}

impl<TFileSystem: FileSystem> Future for DiskManager<TFileSystem> {
    type Item = DiskMessageOut;
    type Error = TFileSystem::Error;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        // remove each `block`s one by one, update file state or add them back
        loop {
            for block in (0..self.active_blocks.len()).rev() {
                let mut block = self.active_blocks.swap_remove(block);
                if block.poll(&mut self.file_system)?.is_ready() {
                    return Ok(Async::Ready(self.finalize_block(block)));
                } else {
                    self.active_blocks.push(block);
                }
            }

            loop {
                if let Some(msg) = self.queued_events.pop_front() {
                    match msg {
                        DiskMessageIn::AddTorrent(id, meta) => {
                            let msg = if self.torrents.contains(&id) {
                                DiskMessageOut::TorrentError(
                                    id,
                                    TorrentError::ExistingTorrent { meta },
                                )
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
                            debug!("read block received {:?} {:?}", id, metadata);
                            if let Async::Ready(res) =
                                self.poll_create_block_read(id, metadata.clone())?
                            {
                                match res {
                                    Ok(block) => self.active_blocks.push(block),
                                    Err(err) => {
                                        return Ok(Async::Ready(DiskMessageOut::ReadBlockError(
                                            id, err,
                                        )));
                                    }
                                }
                            } else {
                                return Ok(Async::NotReady);
                            }
                        }
                        DiskMessageIn::WriteBlock(id, block) => {
                            debug!("Write block received {:?} {:?}", id, block.metadata());
                            if let Async::Ready(res) = self.poll_create_block_write(id, block)? {
                                match res {
                                    Ok(block) => {
                                        self.active_blocks.push(block);
                                    }
                                    Err(err) => {
                                        debug!("Found bad piece to write {:?}", err);
                                        return Ok(Async::Ready(DiskMessageOut::WriteBlockError(
                                            id, err,
                                        )));
                                    }
                                }
                            } else {
                                return Ok(Async::NotReady);
                            }
                        }
                    }
                } else {
                    break;
                }
            }

            if self.queued_events.is_empty() {
                return Ok(Async::NotReady);
            }
        }
    }
}

pub enum FileState<TFile> {
    Queued(TFile, FileWindow),
    Ready(TFile),
    Busy(FileWindow),
}
