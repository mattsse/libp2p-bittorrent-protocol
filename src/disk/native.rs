use std::collections::HashMap;
use std::io;
use std::io::{Error, SeekFrom};
use std::path::{Path, PathBuf};

use futures::{Async, Future};
use tokio_fs::file::{OpenFuture, SeekFuture};
use tokio_fs::{File, OpenOptions};
use tokio_io::{AsyncRead, AsyncWrite};

use crate::disk::block::{Block, BlockMut};
use crate::disk::error::TorrentError;
use crate::disk::fs::FileSystem;
use crate::torrent::MetaInfo;

/// File that exists on disk.
pub struct NativeFile {
    file: File,
}

/// File system that maps to the OS file system.
pub struct NativeFileSystem {
    target_dir: PathBuf,
    file_ops: OpenOptions,
}

impl<T: AsRef<Path>> From<T> for NativeFileSystem {
    fn from(dir: T) -> Self {
        let mut file_ops = OpenOptions::new();
        file_ops.read(true).write(true).create(true);

        NativeFileSystem {
            target_dir: dir.as_ref().to_path_buf(),
            file_ops,
        }
    }
}

impl FileSystem for NativeFileSystem {
    type File = tokio_fs::File;
    type Error = io::Error;

    fn poll_open_file<P>(&mut self, path: P) -> Result<Async<Self::File>, Self::Error>
    where
        P: AsRef<Path>,
    {
        let path = self.target_dir.join(path);
        self.file_ops.open(path).poll()
    }

    fn sync_file<P>(&mut self, path: P) -> Result<Async<()>, Self::Error>
    where
        P: AsRef<Path> + Send + 'static,
    {
        unimplemented!()
    }

    fn poll_file_size(&self, file: &mut Self::File) -> Result<Async<u64>, Self::Error> {
        file.poll_metadata().map(|meta| {
            if let Async::Ready(meta) = meta {
                Async::Ready(meta.len())
            } else {
                Async::NotReady
            }
        })
    }

    fn read_file(
        &self,
        file: &mut Self::File,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error> {
        unimplemented!()
    }

    fn poll_seek(&self, file: &mut Self::File, seek: SeekFrom) -> Result<Async<u64>, Self::Error> {
        file.poll_seek(seek)
    }

    fn poll_read_block(
        &self,
        file: &mut Self::File,
        block: &mut BlockMut,
    ) -> Result<Async<usize>, Self::Error> {
        file.read_buf(block.bytes_mut())
    }

    fn poll_write_block(
        &self,
        file: &mut Self::File,
        buf: &[u8],
    ) -> Result<Async<usize>, Self::Error> {
        file.poll_write(buf)
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
