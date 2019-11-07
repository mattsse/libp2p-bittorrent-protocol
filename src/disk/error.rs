use std::io;
use std::path::PathBuf;

use snafu::Snafu;

use crate::disk::block::BlockMetadata;
use crate::peer::piece::TorrentId;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;

#[derive(Debug, Snafu)]
pub enum TorrentError {
    #[snafu(display("IO Err {}", err))]
    Io { err: io::Error },
    #[snafu(display("Failed To Add Torrent Because Size Checker Failed For {:?} Where File Size Was {} But Should Have Been {}", file_path, actual_size, expected_size))]
    ExistingFileSizeCheck {
        file_path: PathBuf,
        expected_size: u64,
        actual_size: u64,
    },
    #[snafu(display("Failed To Add Torrent Because Another Torrent With The Same InfoHash {:?} Is Already Added", meta))]
    ExistingTorrent { meta: MetaInfo },
    #[snafu(display("Can't process block: {:?}", meta))]
    BadBlock { meta: BlockMetadata },
    #[snafu(display("Bad piece at: {}", index))]
    BadPiece { index: u64 },
    #[snafu(display("No torrent found with id: {:?}", id))]
    TorrentNotFound { id: TorrentId },
    #[snafu(display("Found mismatched hashes, expected {}, got {}", expected, got))]
    MismatchedHashes { got: ShaHash, expected: ShaHash },
}
