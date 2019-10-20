use bendy::decoding::Error as DecodingError;
use bendy::encoding::Error as EncodingError;
use snafu::{Backtrace, Snafu};
use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Bendecoding error: {}", err))]
    BendecodingError { err: DecodingError },
    #[snafu(display("Encoding error: {}", err))]
    BencodingError { err: EncodingError },
    #[snafu(display("IO Err {}", err))]
    Io { err: io::Error },
}

impl Error {}

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
