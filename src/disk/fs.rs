use std::io;
use std::path::Path;

/// Trait for performing operations on some file system.
/// Provides the necessary abstractions for handling files
pub trait FileSystem {
    /// Some file object.
    type File;
    type Error;
    type Future;

    /// Open a file, create it if it does not exist.
    ///
    /// Intermediate directories will be created if necessary.
    fn open_file<P>(&self, path: P) -> io::Result<Self::File>
    where
        P: AsRef<Path> + Send + 'static;

    /// Sync the file.
    fn sync_file<P>(&self, path: P) -> io::Result<()>
    where
        P: AsRef<Path> + Send + 'static;

    /// Get the size of the file in bytes.
    fn file_size(&self, file: &Self::File) -> io::Result<u64>;

    /// Read the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes read.
    fn read_file(&self, file: &mut Self::File, offset: u64, buffer: &mut [u8])
        -> io::Result<usize>;

    /// Write the contents of the file at the given offset.
    ///
    /// On success, return the number of bytes written. If offset is
    /// past the current size of the file, zeroes will be filled in.
    fn write_file(&self, file: &mut Self::File, offset: u64, buffer: &[u8]) -> io::Result<usize>;
}
