use bytes::BufMut;
use futures::task::{Context, Poll};
use std::io;
use std::io::SeekFrom;
use std::path::Path;

use crate::disk::block::{Block, BlockMut};

/// Trait for performing operations on some file system.
/// Provides the necessary abstractions for handling files
pub trait FileSystem {
    /// Some file object.
    type File: Unpin;
    type Error: Unpin + std::fmt::Debug;

    /// Open a file, create it if it does not exist.
    ///
    /// Intermediate directories will be created if necessary.
    fn poll_open_file<P>(&self, path: P, cx: &mut Context) -> Poll<Result<Self::File, Self::Error>>
    where
        P: AsRef<Path>;

    /// Attempts to sync all OS-internal metadata to disk.
    fn sync_file<P>(
        &mut self,
        file: &mut Self::File,
        cx: &mut Context,
    ) -> Poll<Result<(), Self::Error>>;

    /// Get the size of the file in bytes.
    fn poll_file_size(
        &self,
        file: &mut Self::File,
        cx: &mut Context,
    ) -> Poll<Result<u64, Self::Error>>;

    fn poll_seek(
        &self,
        file: &mut Self::File,
        seek: SeekFrom,
        cx: &mut Context,
    ) -> Poll<Result<u64, Self::Error>>;

    /// Read the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes read.
    fn poll_read_block<B: BufMut>(
        &self,
        file: &mut Self::File,
        block: &mut B,
        cx: &mut Context,
    ) -> Poll<Result<usize, Self::Error>>;

    /// Write the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes written.
    fn poll_write_block(
        &self,
        file: &mut Self::File,
        buf: &[u8],
        cx: &mut Context,
    ) -> Poll<Result<usize, Self::Error>>;
}
