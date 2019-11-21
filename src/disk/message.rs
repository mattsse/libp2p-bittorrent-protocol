use std::path::PathBuf;

use crate::disk::block::{Block, BlockMetadata, BlockMut};
use crate::disk::error::TorrentError;
use crate::peer::torrent::TorrentId;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;

/// Messages that can be sent to the `DiskManager`.
#[derive(Debug)]
pub enum DiskMessageIn {
    /// Message to add a torrent to the disk manager.
    AddTorrent(TorrentId, MetaInfo),
    /// Message to remove a torrent from the disk manager.
    ///
    /// Note, this will NOT remove any data from the `FileSystem`,
    /// and as an added convenience, this message will also trigger
    /// a `IDiskMessage::SyncTorrent` message.
    RemoveTorrent(TorrentId),
    /// Message to move a torrent to another destination.
    MoveTorrent(TorrentId, PathBuf),
    /// Message to tell the `FileSystem` to sync the torrent.
    ///
    /// This message will trigger a call to `FileSystem::sync` for every
    /// file in the torrent, so the semantics will differ depending on the
    /// `FileSystem` in use.
    ///
    /// In general, if a torrent has finished downloading, but will be kept
    /// in the `DiskManager` to, for example, seed the torrent, then this
    /// message should be sent, otherwise, `IDiskMessage::RemoveTorrent` is
    /// sufficient.
    SyncTorrent(TorrentId),
    /// Message to load the given block in to memory.
    ReadBlock(TorrentId, BlockMetadata),
    /// Message to process the given block and persist it.
    WriteBlock(TorrentId, Block),
}

/// Messages that can be received from the `DiskManager`.
#[derive(Debug)]
pub enum DiskMessageOut {
    /// Message indicating that the torrent has been added.
    TorrentAdded(TorrentId),
    /// Message indicating that the torrent has been removed.
    TorrentRemoved(TorrentId, MetaInfo),
    /// Message indicating that the torrent has beed moved to another
    /// destination.
    MovedTorrent(TorrentId, PathBuf),
    /// Message indicating that the torrent has been synced.
    TorrentSynced(TorrentId),
    /// Message indicating that the given block has been successfully loaded.
    BlockRead(TorrentId, BlockMut),
    /// Message indicating that the given block has been successfully processed.
    PieceWritten(TorrentId, Block),
    /// Error occurring from a `AddTorrent` or `RemoveTorrent` message.
    TorrentError(TorrentId, TorrentError),
    /// Error occurring from a `ReadBlock` message.
    ReadBlockError(TorrentId, TorrentError),
    /// Error occurring from a `WriteBlock` message.
    WriteBlockError(TorrentId, TorrentError),
}
