use bytes::BufMut;
use futures::io::Error;
use futures::prelude::*;
use futures::task::{Context, Poll};
use std::collections::HashMap;
use std::io;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::fs::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

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
    type File = File;
    type Error = io::Error;

    fn poll_open_file<P>(&self, path: P, cx: &mut Context) -> Poll<Result<Self::File, Self::Error>>
    where
        P: AsRef<Path>,
    {
        let path = self.target_dir.join(path);
        self.file_ops.open(path).boxed().poll_unpin(cx)
    }

    fn sync_file<P>(
        &mut self,
        file: &mut Self::File,
        cx: &mut Context,
    ) -> Poll<Result<(), Self::Error>> {
        file.sync_all().boxed().poll_unpin(cx)
    }

    fn poll_file_size(
        &self,
        file: &mut Self::File,
        cx: &mut Context,
    ) -> Poll<Result<u64, Self::Error>> {
        file.metadata()
            .boxed()
            .poll_unpin(cx)
            .map(|r| r.map(|meta| meta.len()))
    }

    fn poll_seek(
        &self,
        file: &mut Self::File,
        seek: SeekFrom,
        cx: &mut Context,
    ) -> Poll<Result<u64, Self::Error>> {
        file.seek(seek).boxed().poll_unpin(cx)
    }

    fn poll_read_block<B: BufMut>(
        &self,
        file: &mut Self::File,
        block: &mut B,
        cx: &mut Context,
    ) -> Poll<Result<usize, Self::Error>> {
        Pin::new(file).poll_read_buf(cx, block)
    }

    fn poll_write_block(
        &self,
        file: &mut Self::File,
        buf: &[u8],
        cx: &mut Context,
    ) -> Poll<Result<usize, Self::Error>> {
        Pin::new(file).poll_write(cx, buf)
    }
}
