use crate::disk::fs::FileSystem;
use std::io;
use std::io::Error;
use std::path::{Path, PathBuf};
use tokio_fs::File;

/// File that exists on disk.
pub struct NativeFile {
    file: File,
}

/// File system that maps to the OS file system.
pub struct NativeFileSystem {
    target_dir: PathBuf,
}

impl NativeFile {
    /// Create a new file with read and write options.
    /// Intermediate directories will be created if they do not exist.
    fn create_new_file<P: AsRef<Path>>(path: P) -> io::Result<File> {
        unimplemented!()
    }
}

impl<T: AsRef<Path>> From<T> for NativeFileSystem {
    fn from(dir: T) -> Self {
        NativeFileSystem {
            target_dir: dir.as_ref().to_path_buf(),
        }
    }
}

impl FileSystem for NativeFileSystem {
    type File = NativeFile;
    type Error = ();
    type Future = ();

    fn open_file<P>(&self, path: P) -> Result<Self::File, Error>
    where
        P: AsRef<Path> + Send + 'static,
    {
        unimplemented!()
    }

    fn sync_file<P>(&self, path: P) -> Result<(), Error>
    where
        P: AsRef<Path> + Send + 'static,
    {
        unimplemented!()
    }

    fn file_size(&self, file: &Self::File) -> Result<u64, Error> {
        unimplemented!()
    }

    fn read_file(
        &self,
        file: &mut Self::File,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error> {
        unimplemented!()
    }

    fn write_file(
        &self,
        file: &mut Self::File,
        offset: u64,
        buffer: &[u8],
    ) -> Result<usize, Error> {
        unimplemented!()
    }
}
