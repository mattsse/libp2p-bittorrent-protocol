//! Implementation of the `Bittorrent` network behaviour.

use std::borrow::Cow;

/// Network behaviour that handles Bittorrent.
// TODO TFs should probably a DiskManager for the Filesystem
pub struct Bittorrent<TSubstream, TFilesystem> {
    s: TSubstream,
    /// The filesystem storage.
    fs: TFilesystem,
}

/// The configuration for the `Bittorrent` behaviour.
///
/// The configuration is consumed by [`Bittorrent::new`].
#[derive(Debug, Clone)]
pub struct BittorrentConfig {
    protocol_name_override: Option<Cow<'static, [u8]>>,
    // TODO add support for resuming saved torrent states
}

// TODO impl NetworkBehaviour for Bittorrent
