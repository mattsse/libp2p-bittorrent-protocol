use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::io::SeekFrom;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use bit_vec::BitBlock;
use bytes::Bytes;
use fnv::{FnvHashMap, FnvHashSet};
use futures::future::Either;
use futures::prelude::*;
use lru_cache::LruCache;
use tokio::fs::OpenOptions;

use crate::disk::block::{
    Block,
    BlockFile,
    BlockFileRead,
    BlockFileWrite,
    BlockJob,
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
use crate::handler::BitTorrentHandlerEvent::TorrentErr;
use crate::peer::torrent::TorrentId;
use crate::piece::PieceState;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;

/// `DiskManager` object which handles the storage of `Blocks` to the
/// `FileSystem`.
pub struct DiskManager<TFileSystem: FileSystem> {
    active_blocks: Vec<BlockJob<TFileSystem::File>>,
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

    fn move_torrent(&mut self, id: TorrentId, dir: PathBuf) {}

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

    fn poll_files(
        &mut self,
        id: TorrentId,
        meta: BlockMetadata,
        cx: &mut Context,
    ) -> Poll<Result<Vec<(FileWindow, BlockFile<TFileSystem::File>)>, TorrentError>> {
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
                            if let Poll::Ready(res) =
                                self.file_system.poll_open_file(&file.path, cx)
                            {
                                match res {
                                    Ok(fs_file) => {
                                        queued_files.push((file.id, fs_file, window));
                                    }
                                    Err(err) => {
                                        return Poll::Ready(Err(TorrentError::Io {
                                            err: io::Error::new(
                                                io::ErrorKind::Other,
                                                "Failed to open file.",
                                            ),
                                        }))
                                    }
                                }
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

                        Poll::Ready(Ok(blocks))
                    } else {
                        // add removed states back, but queued
                        for (id, file, window) in queued_files {
                            self.file_cache.insert(id, FileState::Queued(file, window));
                        }
                        Poll::Pending
                    }
                }
                Err(e) => {
                    debug!("No torrent files for block available.");
                    Poll::Ready(Err(e))
                }
            }
        } else {
            Poll::Ready(Err(TorrentError::TorrentNotFound { id }))
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
        cx: &mut Context,
    ) -> Poll<Result<BlockJob<TFileSystem::File>, TorrentError>> {
        // validate piece hash
        if !self.is_good_piece(id, &block) {
            error!("Piece hash mismatch {:?}", block.len());
            return Poll::Ready(Err(TorrentError::BadPiece {
                index: block.metadata().piece_index,
            }));
        }

        if let Poll::Ready(res) = self.poll_files(id, block.metadata(), cx) {
            match res {
                Ok(files) => {
                    if files.len() == 1 {
                        let (window, file) = files.into_iter().next().unwrap();

                        Poll::Ready(Ok(BlockJob::Write(BlockWrite::Single {
                            metadata: block.metadata(),
                            block: BlockFileWrite::new(file, block.into_bytes(), window),
                        })))
                    } else {
                        let mut blocks = Vec::with_capacity(files.len());
                        let metadata = block.metadata();

                        let mut fragemt_offset = 0;

                        for (window, file) in files {
                            // adjust blocks
                            let fragment =
                                Bytes::from(block[fragemt_offset..window.length as usize].to_vec());
                            fragemt_offset += window.length as usize;
                            blocks.push(BlockFileWrite::new(file, fragment, window));
                        }
                        let block = BlockJob::Write(BlockWrite::Overlap {
                            torrent: id,
                            blocks,
                            metadata,
                        });
                        Poll::Ready(Ok(block))
                    }
                }
                Err(err) => {
                    debug!("No files found to write piece to {:?}", err);
                    Poll::Ready(Err(err))
                }
            }
        } else {
            self.queued_events
                .push_back(DiskMessageIn::WriteBlock(id, block));
            Poll::Pending
        }
    }

    fn poll_create_block_read(
        &mut self,
        id: TorrentId,
        meta: BlockMetadata,
        cx: &mut Context,
    ) -> Poll<Result<BlockJob<TFileSystem::File>, TorrentError>> {
        if let Poll::Ready(res) = self.poll_files(id, meta.clone(), cx) {
            match res {
                Ok(files) => {
                    if files.len() == 1 {
                        let (window, file) = files.into_iter().next().unwrap();
                        Poll::Ready(Ok(BlockJob::Read(BlockRead::Single {
                            metadata: meta,
                            block: BlockFileRead::new(file, window),
                        })))
                    } else {
                        let mut blocks = Vec::with_capacity(files.len());

                        for (window, file) in files {
                            blocks.push(BlockFileRead::new(file, window));
                        }
                        let block = BlockJob::Read(BlockRead::Overlap {
                            torrent: id,
                            blocks,
                            metadata: meta,
                        });
                        Poll::Ready(Ok(block))
                    }
                }
                Err(err) => {
                    error!("Error opening files {:?}", err);
                    Poll::Ready(Err(err))
                }
            }
        } else {
            debug!("Pending acquiring files");
            self.queued_events
                .push_back(DiskMessageIn::ReadBlock(id, meta));
            Poll::Pending
        }
    }

    fn finalize_block(&mut self, block: BlockJob<TFileSystem::File>) -> DiskMessageOut {
        debug!("Finalizing block");
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
                    Ok(block) => {
                        debug!("Successfully read block {:?}", block.metadata());
                        DiskMessageOut::BlockRead(torrent, block)
                    }
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
                    Ok(block) => DiskMessageOut::PieceWritten(torrent, block),
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

impl<TFileSystem: FileSystem + Unpin> Future for DiskManager<TFileSystem> {
    type Output = Result<DiskMessageOut, TFileSystem::Error>;

    // TODO implement statemachine for advancing blocks: open file -> read ->
    // finalize --> cache files
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        // remove each `block`s one by one, update file state or add them back
        loop {
            debug!("polling active blocks {}", self.active_blocks.len());
            for block in (0..self.active_blocks.len()).rev() {
                let mut block = self.active_blocks.swap_remove(block);
                if block.poll(&mut self.file_system, cx).is_ready() {
                    debug!("Block ready for finalizing");
                    return Poll::Ready(Ok(self.finalize_block(block)));
                } else {
                    debug!("Block not ready for finalizing");
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
                            return Poll::Ready(Ok(msg));
                        }
                        DiskMessageIn::RemoveTorrent(id) => {
                            return Poll::Ready(Ok(self.remove_torrent(id)));
                        }
                        DiskMessageIn::SyncTorrent(id) => {}
                        DiskMessageIn::ReadBlock(id, metadata) => {
                            debug!("polling readblock {:?} {:?}", id, metadata);
                            if let Poll::Ready(res) =
                                self.poll_create_block_read(id, metadata.clone(), cx)
                            {
                                match res {
                                    Ok(block) => self.active_blocks.push(block),
                                    Err(err) => {
                                        return Poll::Ready(Ok(DiskMessageOut::ReadBlockError(
                                            id, err,
                                        )));
                                    }
                                }
                            } else {
                                debug!("Pending read...");
                                return Poll::Pending;
                            }
                        }
                        DiskMessageIn::WriteBlock(id, block) => {
                            debug!("polling write block {:?} {:?}", id, block.metadata());
                            if let Poll::Ready(res) = self.poll_create_block_write(id, block, cx) {
                                match res {
                                    Ok(block) => {
                                        self.active_blocks.push(block);
                                    }
                                    Err(err) => {
                                        debug!("Found bad piece to write {:?}", err);
                                        return Poll::Ready(Ok(DiskMessageOut::WriteBlockError(
                                            id, err,
                                        )));
                                    }
                                }
                            } else {
                                debug!("Pending write...");
                                return Poll::Pending;
                            }
                        }
                        DiskMessageIn::MoveTorrent(id, dir) => self.move_torrent(id, dir),
                    }
                } else {
                    break;
                }
            }

            if self.active_blocks.is_empty() {
                debug!("no active blocks");
                return Poll::Pending;
            }
        }
    }
}

pub enum FileState<TFile> {
    Queued(TFile, FileWindow),
    Ready(TFile),
    Busy(FileWindow),
}
