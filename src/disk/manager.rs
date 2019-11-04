use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::disk::fs::FileSystem;
use crate::disk::message::{DiskMessageIn, DiskMessageOut};
use crate::disk::native::NativeFileSystem;
use crate::peer::piece::TorrentId;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;
use fnv::FnvHashMap;
use futures::future::Either;
use futures::{Async, AsyncSink, Future, Poll, Sink, StartSend, Stream};
use lru_cache::LruCache;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use tokio_fs::file::{OpenFuture, SeekFuture};
use tokio_fs::OpenOptions;

pub struct TorrentEntry {
    meta_info: MetaInfo,
    file_map: HashMap<u64, (ShaHash, Vec<PathBuf>)>,
}

/// `DiskManager` object which handles the storage of `Blocks` to the `FileSystem`.
pub struct DiskManager<TFileSystem: FileSystem> {
    active_blocks: Vec<BlockIn<TFileSystem::File>>,
    file_cache: LruCache<PathBuf, FileState<TFileSystem::File>>,
    torrents: FnvHashMap<TorrentId, MetaInfo>,
    file_system: TFileSystem,
    queued_events: VecDeque<DiskMessageIn>,
    next_file_id: usize,
}

impl<TFileSystem: FileSystem> DiskManager<TFileSystem> {
    fn remove_torrent(&mut self, id: TorrentId) -> Option<MetaInfo> {
        unimplemented!()
        // drain files
    }

    fn add_torrent(&mut self, id: TorrentId, meta: MetaInfo) {}
}

impl<TFileSystem: FileSystem> DiskManager<TFileSystem> {
    pub fn with_capacity(file_system: TFileSystem, capacity: usize) -> Self {
        DiskManager {
            active_blocks: Default::default(),
            file_cache: LruCache::new(capacity),
            torrents: FnvHashMap::default(),
            file_system,
            queued_events: VecDeque::new(),
            next_file_id: 0,
        }
    }

    pub fn with_native_fs<T: AsRef<Path>>(dir: T) -> DiskManager<NativeFileSystem> {
        DiskManager::with_capacity(NativeFileSystem::from(dir), 100)
    }

    /// queue in a new block to write to disk
    // TODO this should return a poll
    pub fn write_block(&mut self, torrent_id: TorrentId, block: Block) {
        self.queued_events
            .push_back(DiskMessageIn::ProcessBlock((torrent_id, block)));
    }

    /// fill a new block from a seed
    // TODO return a poll
    pub fn load_block(&mut self, torrent_id: TorrentId, block: BlockMut) {
        self.queued_events
            .push_back(DiskMessageIn::LoadBlock((torrent_id, block)));
    }
}

/// Unique identifier for an active Torrent operation.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct FileId(usize);

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

            loop {

                // TODO loop through all messages
                // try to poll pending diskmessages directly
                // skip busy files

                //                if let Some(mut handles) = self.files.get_mut(&block.piece_index()) {
                //
                //                    for h in (0..handles.len()).rev() {
                //                        let mut block = handles.swap_remove(h) {
                //
                //                        }
                //                    }
                //                } else {
                //                    let (hash, files) =
                //                        self.meta.info.files_for_piece_index(block.piece_index())?;

                //                }

                // TODO validate the block if its complete
            }
        }

        if self.queued_events.is_empty() {
            return Ok(Async::NotReady);
        }

        Ok(Async::NotReady)
    }
}

#[derive(Debug)]
pub struct BlockFile<TFile> {
    path: PathBuf,
    file: TFile,
}

#[derive(Debug)]
pub struct BlockFileWrite<TFile> {
    file: BlockFile<TFile>,
    block: Block,
    state: BlockState,
}

impl<TFile> BlockFileWrite<TFile> {
    fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self.state {
            BlockState::Ready => Ok(Async::Ready(())),
            BlockState::Seek => {
                if let Async::Ready(_) = fs.poll_seek(
                    &mut self.file.file,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_write_block(&mut self.file.file, &mut self.block)?
                    {
                        self.state = BlockState::Ready;
                        Ok(Async::Ready(()))
                    } else {
                        self.state = BlockState::Process;
                        Ok(Async::NotReady)
                    }
                } else {
                    Ok(Async::NotReady)
                }
            }
            BlockState::Process => {
                if let Async::Ready(_) =
                    fs.poll_write_block(&mut self.file.file, &mut self.block)?
                {
                    self.state = BlockState::Ready;
                    Ok(Async::Ready(()))
                } else {
                    Ok(Async::NotReady)
                }
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum BlockState {
    Seek,
    Process,
    Ready,
}

#[derive(Debug)]
pub struct BlockFileRead<TFile> {
    file: BlockFile<TFile>,
    block: BlockMut,
    state: BlockState,
}

impl<TFile> BlockFileRead<TFile> {
    fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self.state {
            BlockState::Ready => Ok(Async::Ready(())),
            BlockState::Seek => {
                if let Async::Ready(_) = fs.poll_seek(
                    &mut self.file.file,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_read_block(&mut self.file.file, &mut self.block)?
                    {
                        self.state = BlockState::Ready;
                        Ok(Async::Ready(()))
                    } else {
                        self.state = BlockState::Process;
                        Ok(Async::NotReady)
                    }
                } else {
                    Ok(Async::NotReady)
                }
            }
            BlockState::Process => {
                if let Async::Ready(_) = fs.poll_read_block(&mut self.file.file, &mut self.block)? {
                    self.state = BlockState::Ready;
                    Ok(Async::Ready(()))
                } else {
                    Ok(Async::NotReady)
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum BlockIn<TFile> {
    Read(ReadBlock<TFile>),
    Write(WriteBlock<TFile>),
}

impl<TFile> BlockIn<TFile> {
    fn piece_index(&self) -> u64 {
        match self {
            BlockIn::Read(block) => block.piece_index,
            BlockIn::Write(block) => block.piece_index,
        }
    }

    fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self {
            BlockIn::Read(read) => match &mut read.block {
                BlockRead::Single(block) => block.poll(fs),
                BlockRead::Overlap(blocks) => {
                    let mut ready = true;
                    for block in blocks {
                        if block.poll(fs)?.is_not_ready() {
                            ready = false;
                        }
                    }
                    if ready {
                        Ok(Async::Ready(()))
                    } else {
                        Ok(Async::NotReady)
                    }
                }
            },

            BlockIn::Write(write) => match &mut write.block {
                BlockWrite::Single(block) => block.poll(fs),
                BlockWrite::Overlap(blocks) => {
                    let mut ready = true;
                    for block in blocks {
                        if block.poll(fs)?.is_not_ready() {
                            ready = false;
                        }
                    }
                    if ready {
                        Ok(Async::Ready(()))
                    } else {
                        Ok(Async::NotReady)
                    }
                }
            },
        }
    }

    fn into_block_out(self) -> BlockOut<TFile> {
        match self {
            BlockIn::Read(read) => BlockOut::Read(read),
            BlockIn::Write(write) => BlockOut::Written(write),
        }
    }
}

pub enum FileState<TFile> {
    Ready(TFile, FileId),
    Busy(u64),
}

#[derive(Debug)]
pub enum BlockOut<TFile> {
    Read(ReadBlock<TFile>),
    Written(WriteBlock<TFile>),
    /// Error occurring from a `LoadBlock` message.
    LoadBlockError(TorrentError),
    /// Error occurring from a `ProcessBlock` message.
    ProcessBlockError(TorrentError),
    FoundBadPiece(u64),
}

#[derive(Debug)]
pub struct ReadBlock<TFile> {
    piece_index: u64,
    block: BlockRead<TFile>,
}

#[derive(Debug)]
pub enum BlockRead<TFile> {
    Single(BlockFileRead<TFile>),
    Overlap(Vec<BlockFileRead<TFile>>),
}

#[derive(Debug)]
pub struct WriteBlock<TFile> {
    piece_index: u64,
    block: BlockWrite<TFile>,
}

#[derive(Debug)]
pub enum BlockWrite<TFile> {
    Single(BlockFileWrite<TFile>),
    Overlap(Vec<BlockFileWrite<TFile>>),
}
