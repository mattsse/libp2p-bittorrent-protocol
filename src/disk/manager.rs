use crate::disk::block::{Block, BlockMut};
use crate::disk::file::FileHandle;
use crate::disk::fs::FileSystem;
use crate::disk::message::{DiskMessageIn, DiskMessageOut};
use crate::disk::native::NativeFileSystem;
use crate::peer::piece::TorrentId;
use fnv::FnvHashMap;
use futures::{Async, AsyncSink, Future, Poll, Sink, StartSend, Stream};
use std::collections::VecDeque;
use std::io;
use std::path::Path;

/// `DiskManager` object which handles the storage of `Blocks` to the `FileSystem`.
pub struct DiskManager<TFileSystem: FileSystem> {
    torrents: FnvHashMap<TorrentId, FileHandle>,
    file_system: TFileSystem,
    queued_ios: VecDeque<DiskMessageIn>,
    queued_reads: VecDeque<DiskMessageOut>,
}

impl<TFileSystem: FileSystem> DiskManager<TFileSystem> {
    pub fn new(file_system: TFileSystem) -> Self {
        DiskManager {
            torrents: FnvHashMap::default(),
            file_system,
            queued_ios: VecDeque::new(),
            queued_reads: VecDeque::new(),
        }
    }

    pub fn with_native_fs<T: AsRef<Path>>(dir: T) -> DiskManager<NativeFileSystem> {
        DiskManager::new(NativeFileSystem::from(dir))
    }

    /// queue in a new block to write to disk
    pub fn write_block(&mut self, torrent_id: TorrentId, block: Block) {
        self.queued_ios
            .push_back(DiskMessageIn::ProcessBlock((torrent_id, block)));
    }

    /// fill a new block from a seed
    pub fn load_block(&mut self, torrent_id: TorrentId, block: BlockMut) {
        self.queued_ios
            .push_back(DiskMessageIn::LoadBlock((torrent_id, block)));
    }
}

impl<TFileSystem: FileSystem> Future for DiskManager<TFileSystem> {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        unimplemented!()
    }
}

impl<TFileSystem: FileSystem> Sink for DiskManager<TFileSystem> {
    type SinkItem = DiskMessageIn;
    type SinkError = ();

    fn start_send(&mut self, item: DiskMessageIn) -> StartSend<DiskMessageIn, ()> {
        unimplemented!()
    }

    fn poll_complete(&mut self) -> Poll<(), ()> {
        unimplemented!()
    }
}

impl<TFileSystem: FileSystem> Stream for DiskManager<TFileSystem> {
    type Item = DiskMessageOut;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<DiskMessageOut>, ()> {
        unimplemented!()
    }
}
