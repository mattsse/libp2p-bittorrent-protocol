use std::cmp::max;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use futures::future::ok;
use futures::{Future, Stream};
use sha1::Sha1;
use walkdir::{DirEntry, WalkDir};

use crate::error::Error;
use crate::piece::{BLOCK_SIZE_MAX, BLOCK_SIZE_MIN};
use crate::torrent::{DhtNode, InfoContent, MetaInfo, SubFileInfo, TorrentInfo};

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
    /// tracker url
    announce: Option<String>,
    announce_list: Option<Vec<String>>,
    webseeds: Option<Vec<String>>,
    httpseeds: Option<Vec<String>>,
    /// the name of the content, if none then the file name of `content_root` is
    /// used
    name: Option<String>,
    encoding: Option<String>,
    comment: Option<String>,
    created_by: Option<String>,
    creation_date: Option<i64>,
    /// root of the content to include, either a directory or single file
    content_root: PathBuf,
    piece_length: Option<usize>,
    nodes: Option<Vec<DhtNode>>,
}

impl TorrentBuilder {
    /// create a new builder using `file` as targeted content
    pub fn new<P: AsRef<Path>>(file: P) -> TorrentBuilder {
        TorrentBuilder {
            content_root: file.as_ref().to_path_buf(),
            ..Default::default()
        }
    }

    pub fn name<T: ToString>(mut self, name: T) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn encoding<T: ToString>(mut self, encoding: T) -> Self {
        self.encoding = Some(encoding.to_string());
        self
    }

    pub fn comment<T: ToString>(mut self, comment: T) -> Self {
        self.comment = Some(comment.to_string());
        self
    }

    pub fn created_by<T: ToString>(mut self, created_by: T) -> Self {
        self.created_by = Some(created_by.to_string());
        self
    }

    pub fn creation_date<Tz: TimeZone>(mut self, creation_date: DateTime<Tz>) -> Self {
        self.creation_date = Some(creation_date.timestamp());
        self
    }

    pub fn creation_now(mut self) -> Self {
        self.creation_date(Utc::now())
    }

    /// sets the desired piece length.
    /// If min or max boundaries are violated self as error is returned
    pub fn piece_length(mut self, piece_length: usize) -> Result<Self, Self> {
        if piece_length >= BLOCK_SIZE_MIN && piece_length <= BLOCK_SIZE_MAX {
            self.piece_length = Some(piece_length);
            Ok(self)
        } else {
            Err(self)
        }
    }

    pub fn announce<T: ToString>(mut self, announce: T) -> Self {
        self.announce = Some(announce.to_string());
        self
    }

    pub fn add_announce_list<T: ToString>(mut self, announce: T) -> Self {
        if let Some(ref mut list) = self.announce_list {
            list.push(announce.to_string());
        } else {
            self.announce_list = Some(vec![announce.to_string()]);
        }
        self
    }

    pub fn announce_list<T: ToString>(mut self, announce: &[T]) -> Self {
        self.announce_list = Some(announce.into_iter().map(T::to_string).collect());
        self
    }

    pub fn add_webseed<T: ToString>(mut self, webseed: T) -> Self {
        if let Some(ref mut seeds) = self.webseeds {
            seeds.push(webseed.to_string());
        } else {
            self.webseeds = Some(vec![webseed.to_string()]);
        }
        self
    }

    pub fn webseeds<T: ToString>(mut self, webseeds: &[T]) -> Self {
        self.webseeds = Some(webseeds.into_iter().map(T::to_string).collect());
        self
    }

    pub fn add_httpseed<T: ToString>(mut self, httpseed: T) -> Self {
        if let Some(ref mut seeds) = self.httpseeds {
            seeds.push(httpseed.to_string());
        } else {
            self.httpseeds = Some(vec![httpseed.to_string()]);
        }
        self
    }

    pub fn httpseeds<T: ToString>(mut self, httpseeds: &[T]) -> Self {
        self.httpseeds = Some(httpseeds.into_iter().map(T::to_string).collect());
        self
    }

    pub fn add_node(mut self, node: DhtNode) -> Self {
        if let Some(ref mut nodes) = self.nodes {
            nodes.push(node);
        } else {
            self.nodes = Some(vec![node]);
        }
        self
    }

    pub fn nodes(mut self, nodes: Vec<DhtNode>) -> Self {
        self.nodes = Some(nodes);
        self
    }

    /// For torrents up to 1 GiB, the maximum number of pieces is 1000 which
    /// means the maximum piece size is 1 MiB. With increasing torrent size,
    /// both the number of pieces and the maximum piece size are increased.
    /// For torrents between 32 and 80 GiB a maximum piece size of 8 MiB is
    /// maintained by increasing the number of pieces up to 10,000.
    /// For torrents larger than 80 GiB the piece size is 16 MiB, using as many
    /// pieces as necessary.
    fn compute_piece_length(&self) -> io::Result<usize> {
        let mut size = 0;
        for entry in WalkDir::new(&self.content_root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e) && e.file_type().is_file())
        {
            size += entry?.metadata()?.len();
        }

        let two_pow_30 = 1073741824;

        let pieces = match size {
            size if size <= two_pow_30 => size as f64 / 1024.,
            size if size <= 4 * two_pow_30 => size as f64 / 2048.,
            size if size <= 6 * two_pow_30 => size as f64 / 3072.,
            size if size <= 8 * two_pow_30 => size as f64 / 2048.,
            size if size <= 16 * two_pow_30 => size as f64 / 2048.,
            size if size <= 32 * two_pow_30 => size as f64 / 4096.,
            size if size <= 64 * two_pow_30 => size as f64 / 8192.,
            size if size <= 80 * two_pow_30 => size as f64 / 10000.,
            _ => return Ok(BLOCK_SIZE_MAX),
        };

        let size = max(1 << pieces.log2().ceil().max(0.) as usize, 16 * 1024);

        Ok(size)
    }

    /// Build a `MetaInfo` object from this `TorrentBuilder`.
    pub fn build(self) -> Result<MetaInfo, Error> {
        let piece_length = self.piece_length.unwrap_or(self.compute_piece_length()?);
        let (content, pieces) = self.read_content(piece_length)?;

        let info = TorrentInfo {
            pieces: pieces
                .into_iter()
                .map(|hash| hash.digest().bytes().into())
                .collect(),
            piece_length: piece_length as u64,
            content,
            name: self.name.unwrap_or(
                self.content_root
                    .file_name()
                    .and_then(|os| os.to_os_string().into_string().ok())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid target filename")
                    })?,
            ),
        };

        Ok(MetaInfo {
            announce: self.announce,
            announce_list: self.announce_list.unwrap_or_default(),
            encoding: self.encoding,
            comment: self.comment,
            created_by: self.created_by,
            creation_date: self.creation_date,
            webseeds: self.webseeds.unwrap_or_default(),
            httpseeds: self.httpseeds.unwrap_or_default(),
            nodes: self.nodes.unwrap_or_default(),
            info_hash: info.sha1_hash()?,
            info,
        })
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

    /// returns the length of the file and the hashed pieces and potential
    /// unfinished bits
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_torrent() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        let meta = TorrentBuilder::new(dir.join("Cargo.toml"))
            .announce("Some Tracker")
            .build()
            .unwrap();

        let torrent = dir.join("cargo.torrent");
        meta.write_torrent_file(&torrent).unwrap();
        let read = MetaInfo::from_torrent_file(&torrent).unwrap();
        assert_eq!(meta, read);
        std::fs::remove_file(&torrent).unwrap();
    }

    #[test]
    fn roundtrip() {
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("resources");

        let meta = MetaInfo::from_torrent_file(dir.join("debian-9.4.0-amd64-netinst.iso.torrent"))
            .unwrap();

        let build = TorrentBuilder::new(dir.join("pieces.iso")).announce("http://bttracker.debian.org:6969/announce")
            .comment("\"Debian CD from cdimage.debian.org\"")
            .httpseeds(&[ "https://cdimage.debian.org/cdimage/release/9.4.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-9.4.0-amd64-netinst.iso",
                "https://cdimage.debian.org/cdimage/archive/9.4.0//srv/cdbuilder.debian.org/dst/deb-cd/weekly-builds/amd64/iso-cd/debian-9.4.0-amd64-netinst.iso"])
            .name("debian-9.4.0-amd64-netinst.iso")
            .creation_date(Utc.timestamp(1_520_682_848,0))
            .piece_length(262_144).unwrap().build().unwrap();
    }
}
