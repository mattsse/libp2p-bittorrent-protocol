use chrono::NaiveDate;
use sha1::Sha1;

#[derive(Clone, Eq, PartialEq)]
pub struct TorrentInfo {
    /// the hash that identifies this torrent
    pub info_hash: Sha1,

    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<NaiveDate>,
}

/// TODO convert to bitflag
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum TorrentType {
    MultiFile,
    Private,
    I2P,
    SSL,
}
