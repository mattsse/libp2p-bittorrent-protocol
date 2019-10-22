use crate::metainfo::{MetaInfo, SubFileInfo};
use futures::Future;
use sha1::Sha1;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use tokio_fs;
use walkdir::DirEntry;

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

// <F: Fn(&DirEntry) -> bool

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TorrentBuilder {
    announce: Option<String>,
    announce_list: Option<Vec<String>>,
    name: Option<String>,
    file: PathBuf,
    piece_length: Option<u64>,
}

impl TorrentBuilder {
    pub fn new<P: AsRef<Path>>(file: P) -> TorrentBuilder {
        TorrentBuilder {
            file: file.as_ref().to_path_buf(),
            ..Default::default()
        }
    }

    fn compute_piece_size() -> usize {
        unimplemented!()
    }

    /// Build a `MetaInfo` object from this `TorrentBuilder`.
    ///
    pub fn build(self) -> Result<MetaInfo, ()> {
        unimplemented!()
    }

    /// returns the length of the file and the hashed pieces and potential unfinished bits
    fn read_file<P: AsRef<Path>>(
        path: P,
        piece_length: usize,
        mut overlap: Option<(usize, Sha1)>,
    ) -> impl Future<Item = (SubFileInfo, Vec<Sha1>, Option<(usize, Sha1)>), Error = io::Error>
    {
        let path = path.as_ref().to_path_buf();

        tokio_fs::File::open(path).and_then(move |file| {
            file.metadata().and_then(move |(mut file, meta)| {
                let length = meta.len() as usize;
                let mut pieces = Vec::with_capacity(length / piece_length);
                let mut total_read = 0;

                if let Some((previous_read, mut hasher)) = overlap {
                    let left = piece_length - previous_read;
                    let mut buf = Vec::with_capacity(left);

                    let exact = file.read(&mut buf)?;
                    if exact != left {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Expected to read {} bytes but got {}", left, exact),
                        ));
                    }
                    hasher.update(&mut buf);
                    pieces.push(hasher);
                    total_read += left;
                }

                let mut overlap = None;

                loop {
                    let left = length - total_read;
                    if left == 0 {
                        break;
                    }
                    if piece_length > left {
                        let mut buf = Vec::with_capacity(left);

                        let exact = file.read(&mut buf)?;
                        if exact != left {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Expected to read {} bytes but got {}", left, exact),
                            ));
                        }
                        let hasher = Sha1::from(&buf);
                        overlap = Some((left, hasher));
                        break;
                    }
                    let mut buf = Vec::with_capacity(piece_length);
                    file.read_exact(&mut buf)?;
                    pieces.push(Sha1::from(&buf));
                    total_read += piece_length;
                }
                // TODO compute parent paths
                path.ancestors()

                Ok((length, pieces, overlap))
            })
        })
    }
}
