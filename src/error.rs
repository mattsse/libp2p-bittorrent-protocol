use bendy::decoding::Error as BError;
use snafu::{Backtrace, Snafu};
use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Bencoding error: {}", err))]
    Bencode { err: BError },
    #[snafu(display("IO Err {}", err))]
    Io { err: io::Error },
}

impl Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io { err }
    }
}

impl From<BError> for Error {
    fn from(err: BError) -> Error {
        Error::Bencode { err }
    }
}
