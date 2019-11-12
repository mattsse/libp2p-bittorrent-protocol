use std::io::SeekFrom;
use std::ops::{Deref, DerefMut};

use bytes::{Bytes, BytesMut};
use futures::Async;

use crate::util::{ShaHash, SHA_HASH_LEN};

use crate::disk::error::TorrentError;
use crate::disk::file::TorrentFileId;
use crate::disk::fs::FileSystem;
use crate::piece::Piece;
use crate::proto::message::PeerRequest;

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
    file_id: TorrentFileId,
    file: TFile,
}

impl<TFile> BlockFile<TFile> {
    pub fn new(file_id: TorrentFileId, file: TFile) -> Self {
        Self { file_id, file }
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
                    &mut self.file.file,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_write_block(&mut self.file.file, &mut self.block)?
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
                    fs.poll_write_block(&mut self.file.file, &mut self.block)?
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
    pub fn new(file_id: TorrentFileId, file: TFile, metadata: BlockMetadata) -> Self {
        Self {
            file: BlockFile::new(file_id, file),
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
                    &mut self.file.file,
                    SeekFrom::Start(self.block.metadata().block_offset),
                )? {
                    if let Async::Ready(_) =
                        fs.poll_read_block(&mut self.file.file, &mut self.block)?
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
                if let Async::Ready(_) = fs.poll_read_block(&mut self.file.file, &mut self.block)? {
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
    fn piece_index(&self) -> u64 {
        unimplemented!()
    }

    pub fn poll<TFileSystem: FileSystem<File = TFile>>(
        &mut self,
        fs: &mut TFileSystem,
    ) -> Result<Async<()>, TFileSystem::Error> {
        match self {
            BlockIn::Read(read) => match read {
                BlockRead::Single(block) => block.poll(fs),
                BlockRead::Overlap(blocks) => {
                    let mut ready = true;
                    for block in blocks {
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
                BlockWrite::Overlap(blocks) => {
                    let mut ready = true;
                    for block in blocks {
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

    fn into_block_out(self) -> BlockOut<TFile> {
        match self {
            BlockIn::Read(read) => BlockOut::Read(read),
            BlockIn::Write(write) => BlockOut::Written(write),
        }
    }
}

#[derive(Debug)]
pub enum BlockRead<TFile> {
    Single(BlockFileRead<TFile>),
    Overlap(Vec<BlockFileRead<TFile>>),
}

impl<TFile> Into<BlockMut> for BlockRead<TFile> {
    fn into(self) -> BlockMut {
        match self {
            BlockRead::Single(block) => {
                block.block;
            }
            BlockRead::Overlap(blocks) => {}
        };

        unimplemented!()
    }
}

#[derive(Debug)]
pub enum BlockWrite<TFile> {
    Single(BlockFileWrite<TFile>),
    Overlap(Vec<BlockFileWrite<TFile>>),
}

#[derive(Debug)]
pub enum BlockOut<TFile> {
    Read(BlockRead<TFile>),
    Written(BlockWrite<TFile>),
    /// Error occurring from a `LoadBlock` message.
    LoadBlockError(TorrentError),
    /// Error occurring from a `ProcessBlock` message.
    ProcessBlockError(TorrentError),
    FoundBadPiece(u64),
}
