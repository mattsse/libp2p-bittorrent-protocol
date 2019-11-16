use std::io::SeekFrom;
use std::ops::{Deref, DerefMut};

use bytes::{BufMut, Bytes, BytesMut};
use futures::Async;

use crate::util::{ShaHash, SHA_HASH_LEN};

use crate::behavior::BlockOk;
use crate::disk::error::TorrentError;
use crate::disk::file::TorrentFileId;
use crate::disk::fs::FileSystem;
use crate::peer::torrent::TorrentId;
use crate::piece::Piece;
use crate::proto::message::PeerRequest;
use std::collections::BTreeMap;

/// `BlockMetadata` which tracks metadata associated with a `Block` of memory.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub struct BlockMetadata {
    pub piece_index: u64,
    pub block_offset: u64,
    pub block_length: usize,
}

impl BlockMetadata {
    pub fn new(piece_index: u64, block_offset: u64, block_length: usize) -> BlockMetadata {
        BlockMetadata {
            piece_index,
            block_offset,
            block_length,
        }
    }

    pub fn piece_index(&self) -> u64 {
        self.piece_index
    }

    pub fn block_offset(&self) -> u64 {
        self.block_offset
    }

    pub fn block_length(&self) -> usize {
        self.block_length
    }
}

impl Default for BlockMetadata {
    fn default() -> BlockMetadata {
        BlockMetadata::new(0, 0, 0)
    }
}

impl From<PeerRequest> for BlockMetadata {
    fn from(request: PeerRequest) -> Self {
        BlockMetadata {
            piece_index: request.index as u64,
            block_offset: request.begin as u64,
            block_length: request.length as usize,
        }
    }
}

impl Into<PeerRequest> for BlockMetadata {
    fn into(self) -> PeerRequest {
        PeerRequest {
            index: self.piece_index as u32,
            begin: self.block_offset as u32,
            length: self.block_length as u32,
        }
    }
}

/// `Block` of immutable memory.
#[derive(Debug)]
pub struct Block {
    metadata: BlockMetadata,
    block_data: Bytes,
}

impl Block {
    /// Create a new `Block`.
    pub fn new(metadata: BlockMetadata, block_data: Bytes) -> Block {
        Block {
            metadata,
            block_data,
        }
    }

    /// Access the metadata for the block.
    pub fn metadata(&self) -> BlockMetadata {
        self.metadata
    }

    pub fn into_parts(self) -> (BlockMetadata, Bytes) {
        (self.metadata, self.block_data)
    }

    pub fn is_correct_len(&self) -> bool {
        self.block_data.len() == self.metadata.block_length
    }

    pub fn into_piece(self) -> Piece {
        self.into()
    }

    pub fn is_valid_len(&self) -> bool {
        self.metadata.block_length == self.block_data.len()
    }
}

impl From<BlockMut> for Block {
    fn from(block: BlockMut) -> Block {
        Block::new(block.metadata(), block.block_data.freeze())
    }
}

impl From<Piece> for Block {
    fn from(piece: Piece) -> Self {
        Self {
            metadata: BlockMetadata::new(piece.index as u64, piece.begin as u64, piece.block.len()),
            block_data: Bytes::from(piece.block),
        }
    }
}

impl Into<Piece> for Block {
    fn into(self) -> Piece {
        Piece {
            index: self.metadata.piece_index as u32,
            begin: self.metadata.block_offset as u32,
            block: self.block_data,
        }
    }
}

impl Deref for Block {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.block_data
    }
}

/// `BlockMut` of mutable memory.
#[derive(Debug)]
pub struct BlockMut {
    metadata: BlockMetadata,
    block_data: BytesMut,
}

impl BlockMut {
    /// Create a new `BlockMut`.
    pub fn new(metadata: BlockMetadata, block_data: BytesMut) -> Self {
        Self {
            metadata,
            block_data,
        }
    }

    /// create a new `BlockMut` with the capacity requested in the metadata
    pub fn empty_for_metadata(metadata: BlockMetadata) -> Self {
        Self {
            block_data: BytesMut::with_capacity(metadata.block_length),
            metadata,
        }
    }

    pub fn is_valid_len(&self) -> bool {
        self.metadata.block_length == self.block_data.len()
    }

    pub fn bytes_mut(&mut self) -> &mut BytesMut {
        &mut self.block_data
    }

    /// Access the metadata for the block.
    pub fn metadata(&self) -> BlockMetadata {
        self.metadata
    }

    pub fn split_into(self) -> (BlockMetadata, BytesMut) {
        (self.metadata, self.block_data)
    }

    pub fn into_piece(self) -> Piece {
        let block: Block = self.into();
        block.into()
    }
}

impl Deref for BlockMut {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.block_data
    }
}

impl DerefMut for BlockMut {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.block_data
    }
}

#[derive(Debug)]
pub struct BlockFile<TFile> {
    pub id: TorrentFileId,
    pub inner: TFile,
}

impl<TFile> BlockFile<TFile> {
    pub fn new(file_id: TorrentFileId, file: TFile) -> Self {
        Self {
            id: file_id,
            inner: file,
        }
    }
}

#[derive(Debug)]
pub struct BlockFileWrite<TFile> {
    file: BlockFile<TFile>,
    block: Block,
    state: BlockState,
}

impl<TFile> BlockFileWrite<TFile> {
    pub fn new(file_id: TorrentFileId, file: TFile, block: Block) -> Self {
        Self {
            file: BlockFile::new(file_id, file),
            block,
            state: Default::default(),
        }
    }

    pub fn metadata(&self) -> &BlockMetadata {
        &self.block.metadata
    }

    pub fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self.state {
            BlockState::Ready => Ok(Async::Ready(())),
            BlockState::Seek => {
                if let Async::Ready(_) = fs.poll_seek(
                    &mut self.file.inner,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_write_block(&mut self.file.inner, &mut self.block)?
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
                    fs.poll_write_block(&mut self.file.inner, &mut self.block)?
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

impl Default for BlockState {
    fn default() -> Self {
        BlockState::Seek
    }
}

#[derive(Debug)]
pub struct BlockFileRead<TFile> {
    file: BlockFile<TFile>,
    block: BlockMut,
    state: BlockState,
}

impl<TFile> BlockFileRead<TFile> {
    pub fn new(file: BlockFile<TFile>, metadata: BlockMetadata) -> Self {
        Self {
            file,
            block: BlockMut::empty_for_metadata(metadata),
            state: Default::default(),
        }
    }

    pub fn metadata(&self) -> &BlockMetadata {
        &self.block.metadata
    }

    pub fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self.state {
            BlockState::Ready => Ok(Async::Ready(())),
            BlockState::Seek => {
                if let Async::Ready(_) = fs.poll_seek(
                    &mut self.file.inner,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_read_block(&mut self.file.inner, &mut self.block)?
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
                    fs.poll_read_block(&mut self.file.inner, &mut self.block)?
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

#[derive(Debug)]
pub enum BlockIn<TFile> {
    Read(BlockRead<TFile>),
    Write(BlockWrite<TFile>),
}

impl<TFile> BlockIn<TFile> {
    pub fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self {
            BlockIn::Read(read) => match read {
                BlockRead::Single(block) => block.poll(fs),
                BlockRead::Overlap { blocks, .. } => {
                    let mut ready = true;
                    for (_, block) in blocks {
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

            BlockIn::Write(write) => match write {
                BlockWrite::Single(block) => block.poll(fs),
                BlockWrite::Overlap { blocks, .. } => {
                    let mut ready = true;
                    for (_, block) in blocks {
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

    pub fn finalize(self) -> BlockResult<TFile> {
        match self {
            BlockIn::Read(read) => read.finalize(),
            BlockIn::Write(write) => write.finalize(),
        }
    }
}

#[derive(Debug)]
pub enum BlockRead<TFile> {
    Single(BlockFileRead<TFile>),
    Overlap {
        torrent: TorrentId,
        metadata: BlockMetadata,
        blocks: BTreeMap<u64, BlockFileRead<TFile>>,
    },
}

impl<TFile> BlockRead<TFile> {
    pub fn finalize(self) -> BlockResult<TFile> {
        match self {
            BlockRead::Single(block) => {
                let result = if block.block.is_valid_len() {
                    Ok(block.block)
                } else {
                    Err(block.block.metadata)
                };

                BlockResult::Read {
                    torrent: block.file.id.torrent,
                    files: vec![block.file],
                    result,
                }
            }
            BlockRead::Overlap {
                torrent,
                blocks,
                metadata,
            } => {
                let mut block = BytesMut::with_capacity(metadata.block_length);
                let mut files = Vec::with_capacity(blocks.len());

                let mut valid = true;

                for (_, file) in blocks {
                    if !file.block.is_valid_len() {
                        valid = false;
                    }
                    files.push(file.file);
                    if valid {
                        block.put_slice(file.block.as_ref());
                    }
                }

                let result = if valid {
                    Ok(BlockMut::new(metadata, block))
                } else {
                    Err(metadata)
                };

                BlockResult::Read {
                    torrent,
                    files,
                    result,
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum BlockWrite<TFile> {
    Single(BlockFileWrite<TFile>),
    Overlap {
        torrent: TorrentId,
        metadata: BlockMetadata,
        blocks: BTreeMap<u64, BlockFileWrite<TFile>>,
    },
}

impl<TFile> BlockWrite<TFile> {
    pub fn finalize(self) -> BlockResult<TFile> {
        match self {
            BlockWrite::Single(block) => {
                let result = if block.block.is_valid_len() {
                    Ok(block.block)
                } else {
                    Err(block.block.metadata)
                };

                BlockResult::Write {
                    torrent: block.file.id.torrent,
                    files: vec![block.file],
                    result,
                }
            }
            BlockWrite::Overlap {
                torrent,
                blocks,
                metadata,
            } => {
                let mut block = BytesMut::with_capacity(metadata.block_length);
                let mut files = Vec::with_capacity(blocks.len());

                let mut valid = true;

                for (_, file) in blocks {
                    if !file.block.is_valid_len() {
                        valid = false;
                    }
                    files.push(file.file);
                    if valid {
                        block.put_slice(file.block.as_ref());
                    }
                }

                let result = if valid {
                    Ok(Block::new(metadata, block.freeze()))
                } else {
                    Err(metadata)
                };

                BlockResult::Write {
                    torrent,
                    files,
                    result,
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum BlockResult<TFile> {
    Read {
        torrent: TorrentId,
        files: Vec<BlockFile<TFile>>,
        result: Result<BlockMut, BlockMetadata>,
    },
    Write {
        torrent: TorrentId,
        files: Vec<BlockFile<TFile>>,
        result: Result<Block, BlockMetadata>,
    },
}
