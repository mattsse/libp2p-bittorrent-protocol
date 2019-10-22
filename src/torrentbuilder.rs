use crate::metainfo::{InfoContent, MetaInfo, SubFileInfo};
use bitflags::_core::cmp::max;
use bitflags::_core::ops::Sub;
use futures::future::ok;
use futures::{Future, Stream};
use sha1::Sha1;
use std::io;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use tokio_fs;
use walkdir::{DirEntry, WalkDir};

/// ignore all hidden entries
fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TorrentBuilder {
    announce: Option<String>,
    announce_list: Option<Vec<String>>,
    /// the name of the content, if none then the file name of `content_root` is used
    name: Option<String>,
    /// root of the content to include, either a directory or single file
    content_root: PathBuf,
    piece_length: Option<usize>,
}

impl TorrentBuilder {
    pub fn new<P: AsRef<Path>>(file: P) -> TorrentBuilder {
        TorrentBuilder {
            content_root: file.as_ref().to_path_buf(),
            ..Default::default()
        }
    }

    /// For torrents up to 1 GiB, the maximum number of pieces is 1000 which means the maximum piece size is 1 MiB.
    /// With increasing torrent size, both the number of pieces and the maximum piece size are increased.
    /// For torrents between 32 and 80 GiB a maximum piece size of 8 MiB is maintained by increasing the number of pieces up to 10,000.
    /// For torrents larger than 80 GiB the piece size is 16 MiB, using as many pieces as necessary.
    fn compute_piece_length(&self) -> io::Result<usize> {
        let mut size = 0;
        for entry in WalkDir::new(&self.content_root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e) && e.file_type().is_file())
        {
            size += entry?.metadata()?.len();
        }

        let pieces = match size {
            size if size <= 2 ^ 30 => size as f64 / 1024.,
            size if size <= 4 * 2 ^ 30 => size as f64 / 2048.,
            size if size <= 6 * 2 ^ 30 => size as f64 / 3072.,
            size if size <= 8 * 2 ^ 30 => size as f64 / 2048.,
            size if size <= 16 * 2 ^ 30 => size as f64 / 2048.,
            size if size <= 32 * 2 ^ 30 => size as f64 / 4096.,
            size if size <= 64 * 2 ^ 30 => size as f64 / 8192.,
            size if size <= 80 * 2 ^ 30 => size as f64 / 10000.,
            _ => return Ok(16 * 2 ^ 20),
        };

        let size = max(1 << pieces.log2().ceil().max(0.) as usize, 16 * 1024);

        Ok(size)
    }

    /// Build a `MetaInfo` object from this `TorrentBuilder`.
    ///
    pub fn build(self) -> io::Result<MetaInfo> {
        let piece_length = self.piece_length.unwrap_or(self.compute_piece_length()?);
        unimplemented!()
    }

    fn read_dir(&self, piece_length: usize) -> io::Result<(Vec<SubFileInfo>, Vec<Sha1>)> {
        let mut overlap = None;
        let mut all_files = Vec::new();
        let mut all_hashes = Vec::new();

        for entry in WalkDir::new(&self.content_root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e) && e.file_type().is_file())
        {
            let (file, file_hashes, file_overlap) =
                self.read_file(entry?.path(), piece_length, overlap)?;
            all_files.push(file);
            all_hashes.extend(file_hashes.into_iter());
            overlap = file_overlap;
        }

        if let Some((_, hash)) = overlap {
            all_hashes.push(hash);
        }
        Ok((all_files, all_hashes))
    }

    fn read_content(&self, piece_length: usize) -> io::Result<(InfoContent, Vec<Sha1>)> {
        if self.content_root.is_dir() {
            self.read_dir(piece_length)
                .map(|(files, pieces)| (InfoContent::Multi { files }, pieces))
        } else {
            self.read_file(&self.content_root, piece_length, None).map(
                |(info, mut pieces, overlap)| {
                    if let Some((_, hash)) = overlap {
                        pieces.push(hash);
                    }
                    (
                        InfoContent::Single {
                            length: info.length,
                        },
                        pieces,
                    )
                },
            )
        }
    }

    fn file_info_paths<P: AsRef<Path>>(&self, path: P) -> io::Result<Vec<String>> {
        let path = path.as_ref();

        let prefix = path
            .strip_prefix(&self.content_root)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let paths: Result<Vec<_>, _> = prefix
            .iter()
            .map(|x| x.to_os_string().into_string())
            .collect();
        paths.map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid filename for {}", path.display()),
            )
        })
    }

    /// returns the length of the file and the hashed pieces and potential unfinished bits
    fn read_file<P: AsRef<Path>>(
        &self,
        path: P,
        piece_length: usize,
        mut overlap: Option<(usize, Sha1)>,
    ) -> io::Result<(SubFileInfo, Vec<Sha1>, Option<(usize, Sha1)>)> {
        let path = path.as_ref();
        let length = path.metadata()?.len() as usize;
        let file_info = SubFileInfo {
            length: length as u64,
            paths: self.file_info_paths(path)?,
        };

        let mut file = BufReader::new(::std::fs::File::open(path)?);

        let mut pieces = Vec::with_capacity(length / piece_length);
        let mut total_read = 0;
        let mut piece_overlap = None;

        if let Some((previous_read, mut hasher)) = overlap {
            let left = piece_length - previous_read;
            if left <= length {
                let mut buf = Vec::with_capacity(left);
                file.read_exact(&mut buf)?;
                hasher.update(&buf);
                pieces.push(hasher);
                total_read += left;
            } else {
                let mut buf = Vec::with_capacity(length);
                let read = file.read_to_end(&mut buf)?;
                if read != length {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Expected to read {} from file {}, but got {}",
                            length,
                            read,
                            path.display()
                        ),
                    ));
                }
                hasher.update(&buf);
                return Ok((file_info, pieces, Some((previous_read + read, hasher))));
            }
        }

        loop {
            let left = length - total_read;
            if left == 0 {
                break;
            }
            if piece_length > left {
                let mut buf = Vec::with_capacity(left);
                let exact = file.read_exact(&mut buf)?;
                let hasher = Sha1::from(&buf);
                piece_overlap = Some((left, hasher));
                break;
            }
            let mut buf = Vec::with_capacity(piece_length);
            file.read_exact(&mut buf)?;
            pieces.push(Sha1::from(&buf));
            total_read += piece_length;
        }

        Ok((file_info, pieces, piece_overlap))
    }
}
