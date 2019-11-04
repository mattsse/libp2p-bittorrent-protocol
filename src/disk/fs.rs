use crate::disk::block::{Block, BlockMut};
use futures::Async;
use std::io;
use std::io::SeekFrom;
use std::path::Path;

/// Trait for performing operations on some file system.
/// Provides the necessary abstractions for handling files
pub trait FileSystem {
    /// Some file object.
    type File;
    type Error;

    /// Open a file, create it if it does not exist.
    ///
    /// Intermediate directories will be created if necessary.
    fn poll_open_file<P>(&mut self, path: P) -> Result<Async<Self::File>, Self::Error>
    where
        P: AsRef<Path> + Send + 'static;

    /// Sync the file.
    fn sync_file<P>(&mut self, path: P) -> Result<Async<()>, Self::Error>
    where
        P: AsRef<Path> + Send + 'static;

    /// Get the size of the file in bytes.
    fn poll_file_size(&self, file: &mut Self::File) -> Result<Async<u64>, Self::Error>;

    /// Read the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes read.
    fn read_file(&self, file: &mut Self::File, offset: u64, buffer: &mut [u8])
        -> io::Result<usize>;

    fn poll_seek(&self, file: &mut Self::File, seek: SeekFrom) -> Result<Async<u64>, Self::Error>;

    /// Read the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes read.
    fn poll_read_block(
        &self,
        file: &mut Self::File,
        block: &mut BlockMut,
    ) -> Result<Async<usize>, Self::Error>;

    /// Write the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes written.
    fn poll_write_block(
        &self,
        file: &mut Self::File,
        block: &Block,
    ) -> Result<Async<usize>, Self::Error>;

    /// Write the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes written. If offset is
    /// past the current size of the file, zeroes will be filled in.
    fn write_file(&self, file: &mut Self::File, offset: u64, buffer: &[u8]) -> io::Result<usize>;
}
