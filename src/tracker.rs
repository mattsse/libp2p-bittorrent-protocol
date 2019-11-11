use std::convert::TryInto;

use bendy::decoding::{Decoder, DictDecoder, ListDecoder};
use bendy::{
    decoding::{Error as DecodingError, ErrorKind, FromBencode, Object, ResultExt},
    encoding::{AsString, Error as EncodingError, SingleItemEncoder, ToBencode},
};
use libp2p_core::PeerId;

use crate::util::ShaHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerResponse {
    Success {
        /// Number of seconds the downloader should wait between regular
        /// rerequests, and peers.
        interval: usize,
        /// The known peers for torrent.
        peers: Vec<PeerResponse>,
    },
    Failure {
        /// Why the query failed.
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerResponse {
    /// A string of length 20 which this downloader uses as its id.
    pub identifier: ShaHash,
    /// The libp2p identifier of the peer.
    pub peer_id: PeerId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerRequest {
    /// The 20 byte sha1 hash of the bencoded form of the info value from the
    /// metainfo file.
    pub info_hash: ShaHash,
    /// A string of length 20 which this downloader uses as its id.
    pub bittorrent_peer_id: ShaHash,
    /// The libp2p identifier of the peer.
    pub libp2p_peer_id: PeerId,
    /// The total amount uploaded so far.
    pub uploaded: usize,
    /// The total amount downloaded so far.
    pub downloaded: usize,
    /// The number of bytes this peer still has to download
    pub left: usize,
    /// An announcement using started is sent when a download first begins, and
    /// one using completed is sent when the download is complete. No completed
    /// is sent if the file was complete when started. Downloaders send an
    /// announcement using stopped when they cease downloading.
    pub event: Option<PeerEvent>,
}

#[derive(Debug)]
pub struct TrackerRequestBuilder {
    /// The 20 byte sha1 hash of the bencoded form of the info value from the
    /// metainfo file.
    info_hash: Option<ShaHash>,
    /// A string of length 20 which this downloader uses as its id.
    bittorrent_peer_id: Option<ShaHash>,
    /// The libp2p identifier of the peer.
    libp2p_peer_id: Option<PeerId>,
    /// The total amount uploaded so far.
    uploaded: Option<usize>,
    /// The total amount downloaded so far.
    downloaded: Option<usize>,
    /// The number of bytes this peer still has to download
    left: Option<usize>,
    /// An announcement using started is sent when a download first begins, and
    /// one using completed is sent when the download is complete. No completed
    /// is sent if the file was complete when started. Downloaders send an
    /// announcement using stopped when they cease downloading.
    event: Option<PeerEvent>,
}

impl TrackerRequest {
    pub fn builder() -> TrackerRequestBuilder {
        TrackerRequestBuilder::default()
    }

    pub fn new<T, S, U>(info_hash: T, bittorrent_peer_id: S, libp2p_peer_id: U, left: usize) -> Self
    where
        T: Into<ShaHash>,
        S: Into<ShaHash>,
        U: Into<PeerId>,
    {
        Self {
            info_hash: info_hash.into(),
            bittorrent_peer_id: bittorrent_peer_id.into(),
            libp2p_peer_id: libp2p_peer_id.into(),
            uploaded: 0,
            downloaded: 0,
            left,
            event: None,
        }
    }
}

impl Default for TrackerRequestBuilder {
    fn default() -> Self {
        Self {
            info_hash: None,
            bittorrent_peer_id: None,
            libp2p_peer_id: None,
            uploaded: None,
            downloaded: None,
            left: None,
            event: None,
        }
    }
}

impl TrackerRequestBuilder {
    pub fn info_hash<T: Into<ShaHash>>(mut self, info_hash: T) -> Self {
        self.info_hash = Some(info_hash.into());
        self
    }

    pub fn bittorrent_peer_id<T: Into<ShaHash>>(mut self, bittorrent_peer_id: T) -> Self {
        self.bittorrent_peer_id = Some(bittorrent_peer_id.into());
        self
    }

    pub fn libp2p_peer_id<T: Into<PeerId>>(mut self, libp2p_peer_id: T) -> Self {
        self.libp2p_peer_id = Some(libp2p_peer_id.into());
        self
    }

    pub fn uploaded(mut self, uploaded: usize) -> Self {
        self.uploaded = Some(uploaded);
        self
    }

    pub fn downloaded(mut self, downloaded: usize) -> Self {
        self.downloaded = Some(downloaded);
        self
    }

    pub fn left(mut self, left: usize) -> Self {
        self.left = Some(left);
        self
    }

    pub fn event(mut self, event: PeerEvent) -> Self {
        self.event = Some(event);
        self
    }

    pub fn build(self) -> Result<TrackerRequest, String> {
        Ok(TrackerRequest {
            info_hash: self
                .info_hash
                .ok_or_else(|| "info_hash must be initialized.")?,
            bittorrent_peer_id: self
                .bittorrent_peer_id
                .ok_or_else(|| "bittorrent_peer_id must be initialized.")?,
            libp2p_peer_id: self
                .libp2p_peer_id
                .ok_or_else(|| "libp2p_peer_id must be initialized.")?,
            uploaded: self
                .uploaded
                .ok_or_else(|| "uploaded must be initialized.")?,
            downloaded: self
                .downloaded
                .ok_or_else(|| "downloaded must be initialized.")?,
            left: self.downloaded.ok_or_else(|| "left must be initialized.")?,
            event: self.event,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Started,
    Completed,
    Stopped,
}

impl PeerEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            PeerEvent::Started => "started",
            PeerEvent::Completed => "completed",
            PeerEvent::Stopped => "stopped",
        }
    }
}

impl ToBencode for TrackerRequest {
    const MAX_DEPTH: usize = 1;

    fn encode(&self, encoder: SingleItemEncoder) -> Result<(), EncodingError> {
        encoder.emit_dict(|mut e| {
            e.emit_pair(b"info_hash", AsString(&self.info_hash));
            e.emit_pair(b"peer_id", AsString(&self.bittorrent_peer_id));

            // TODO how to handle libp2p peer id
            e.emit_pair(b"libp2p_peer_ip", AsString(&self.libp2p_peer_id));

            e.emit_pair(b"uploaded", self.uploaded);
            e.emit_pair(b"downloaded", self.downloaded);
            e.emit_pair(b"left", self.downloaded);

            let event = self.event.as_ref().map_or_else(|| "empty", |x| x.as_str());
            e.emit_pair(b"event", event)
        })
    }
}

impl FromBencode for TrackerRequest {
    const EXPECTED_RECURSION_DEPTH: usize = 1;
    fn decode_bencode_object(object: Object) -> Result<Self, DecodingError>
    where
        Self: Sized,
    {
        let mut dict = object.try_into_dictionary()?;
        let mut builder = TrackerRequestBuilder::default();

        while let Some(pair) = dict.next_pair()? {
            match pair {
                (b"info_hash", value) => {
                    let info_hash: ShaHash = value
                        .try_into_bytes()?
                        .try_into()
                        .map_err(|_| DecodingError::unexpected_field("malformed info_hash"))?;

                    builder = builder.info_hash(info_hash);
                }
                (b"bittorrent_peer_id", value) => {
                    let bittorrent_peer_id: ShaHash = value
                        .try_into_bytes()?
                        .try_into()
                        .map_err(|_| DecodingError::unexpected_field("malformed info_hash"))?;

                    builder = builder.bittorrent_peer_id(bittorrent_peer_id);
                }
                (b"libp2p_peer_ip", value) => {
                    let peer_id: PeerId = String::decode_bencode_object(value)
                        .context("libp2p_peer_ip")?
                        .parse()
                        .map_err(|e| DecodingError::missing_field(e))?;

                    builder = builder.libp2p_peer_id(peer_id);
                }
                (b"uploaded", value) => {
                    builder =
                        builder.uploaded(usize::decode_bencode_object(value).context("uploaded")?);
                }
                (b"downloaded", value) => {
                    builder = builder
                        .downloaded(usize::decode_bencode_object(value).context("downloaded")?);
                }
                (b"left", value) => {
                    builder = builder.left(usize::decode_bencode_object(value).context("left")?);
                }
                (unknown_field, _) => {
                    return match std::str::from_utf8(unknown_field) {
                        Ok(s) => Err(DecodingError::unexpected_field(s)),
                        Err(e) => Err(DecodingError::unexpected_field(e)),
                    };
                }
            }
        }

        builder.build().map_err(|e| DecodingError::missing_field(e))
    }
}
