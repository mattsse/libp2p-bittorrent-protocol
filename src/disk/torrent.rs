use std::path::{Path, PathBuf};

use crate::disk::error::TorrentError;
use crate::error::Result;
use crate::torrent::MetaInfo;
use crate::util::ShaHash;

/// used to document the progress of a torrent for restarting purposes
pub struct TorrentProgress {
    torrent_file: PathBuf,
    content_root: PathBuf,
    file_hash: ShaHash,
}

/// files that are ready to seed
#[derive(Debug, Clone)]
pub struct TorrentSeed {
    /// Path to the content root.
    pub path: PathBuf,
    /// The corresponding torrent file for the content.
    pub torrent: MetaInfo,
}

impl TorrentSeed {
    #[inline]
    pub fn new<Seed: AsRef<Path>, Meta: Into<MetaInfo>>(path: Seed, torrent: Meta) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            torrent: torrent.into(),
        }
    }

    pub fn from_paths<Seed: AsRef<Path>, TorrentFile: AsRef<Path>>(
        path: Seed,
        torrent: TorrentFile,
    ) -> Result<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            torrent: MetaInfo::from_torrent_file(torrent)?,
        })
    }

    //    pub fn set_metainfo_from_torrent<T: AsRef<Path>>(
    //        &mut self,
    //        torrentfile: T,
    //    ) -> Result<&MetaInfo> {
    //        self.torrent = Some(MetaInfo::from_torrent_file(torrentfile)?);
    //        Ok(self.torrent.as_ref().unwrap())
    //    }

    //    pub fn save_torrent_file<T: AsRef<Path>>(
    //        &mut self,
    //        torrentfile: T,
    //    ) -> Result<(&MetaInfo, PathBuf)> {
    //        let torrentfile = torrentfile.as_ref().to_path_buf();
    //        let meta = if let Some(ref meta) = self.torrent {
    //            meta
    //        } else {
    //            self.set_metainfo_from_torrent(&torrentfile)?
    //        };
    //        meta.write_torrent_file(&torrentfile)?;
    //        Ok((meta, torrentfile))
    //    }
}

//impl<T: AsRef<Path>> From<T> for TorrentSeed {
//    fn from(path: T) -> Self {
//        Self {
//            path: path.as_ref().to_path_buf(),
//            torrent: None,
//        }
//    }
//}
