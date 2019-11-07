use std::io;

use bendy::decoding::Error as DecodingError;
use bendy::encoding::Error as EncodingError;
use snafu::{Backtrace, Snafu};

use crate::disk::error::TorrentError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Bendecoding error: {}", err))]
    BendecodingError { err: DecodingError },
    #[snafu(display("Encoding error: {}", err))]
    BencodingError { err: EncodingError },
    #[snafu(display("IO Err {}", err))]
    Io { err: io::Error },
    #[snafu(display("{}", err))]
    Torrent { err: TorrentError },
    #[snafu(display("{}", msg))]
    Message { msg: String },
}

impl Error {
    pub fn message<T: ToString>(msg: T) -> Self {
        Error::Message {
            msg: msg.to_string(),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io { err }
    }
}

impl From<DecodingError> for Error {
    fn from(err: DecodingError) -> Error {
        Error::BendecodingError { err }
    }
}

impl From<EncodingError> for Error {
    fn from(err: EncodingError) -> Error {
        Error::BencodingError { err }
    }
}

impl From<TorrentError> for Error {
    fn from(err: TorrentError) -> Error {
        Error::Torrent { err }
    }
}
