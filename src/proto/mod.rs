pub mod tracker;

use bytes::BytesMut;
use codec::UviBytes;
use futures::{
    future::{self, FutureResult},
    sink, stream, Sink, Stream,
};
use libp2p_core::upgrade::{InboundUpgrade, Negotiated, OutboundUpgrade, UpgradeInfo};
use libp2p_core::{Multiaddr, PeerId};
use sha1::Sha1;
use std::{borrow::Cow, convert::TryFrom, time::Duration};
use std::{io, iter};
use tokio_codec::Framed;
use unsigned_varint::codec;
use wasm_timer::Instant;

/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BtConnectionType {
    /// Sender hasn't tried to connect to peer.
    NotConnected = 0,
    /// Sender is currently connected to peer.
    Connected = 1,
}

/// Status of our interest to download a target by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BtInterestType {
    /// Sender has no interest to download.
    NotInterested = 0,
    /// Sender is currently interested to download.
    Interested = 1,
}

/// Return format of the peer info returned BitTorrent tracker.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BttPeerInfoFormat {
    NotCompact = 0,
    Compact = 1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPiece {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// block of data, which is a subset of the piece specified by index.
    pub block: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeerRequest {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// specifying the requested length.
    pub length: u32,
}

/// All of the remaining messages in the protocol take the form of <length prefix><message ID><payload>.
/// The length prefix is a four byte big-endian value. The message ID is a single decimal byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BttPeerMessage {
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield {
        /// bitfield representing the pieces that have been successfully downloaded
        index_field: Vec<u8>,
    },
    Request {
        peer_request: BttPeerRequest,
    },
    Cancel {
        peer_request: BttPeerRequest,
    },
    Piece {
        piece: BttPiece,
    },
}

/// The handshake is a required message and must be the first message transmitted by the client.
/// `handshake: `<pstrlen><pstr><reserved><info_hash><peer_id>`
#[derive(Clone, PartialEq, Eq)]
pub struct Handshake {
    /// length of `pstr`
    // TODO can be derived from pstr.len
    pub pstrlen: u8,
    /// string identifier of the protocol
    pub pstr: String,
    /// eight (8) reserved bytes. All current implementations use all zeroes.
    /// Each bit in these bytes can be used to change the behavior of the protocol.
    /// An email from Bram suggests that trailing bits should be used first, so that leading bits may be used to change the meaning of trailing bits.
    pub reserved: [u8; 8],
    /// 20-byte SHA1 hash of the info key in the metainfo file.
    /// This is the same info_hash that is transmitted in tracker requests.
    pub info_hash: Sha1,
    /// 20-byte string used as a unique ID for the client
    pub peer_id: [u8; 20],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeer {
    /// Identifier of the peer.
    pub peer_id: [u8; 20],
    /// peer's IP address
    pub ip: PeerIp,
    /// peer's port number
    pub port: u32,
}

/// peer's IP address either IPv6 (hexed) or IPv4 (dotted quad) or DNS name (string)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIp {
    Ip4,
    Ip6,
    DNS(String),
}

/// Creates an `io::Error` with `io::ErrorKind::InvalidData`.
fn invalid_data<E>(e: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, e)
}

#[derive(Debug, Clone)]
pub struct BitTorrentProtocolConfig {
    protocol_name: Cow<'static, [u8]>,
}

impl BitTorrentProtocolConfig {
    /// Modifies the protocol name used on the wire. Can be used to create incompatibilities
    /// between networks on purpose.
    pub fn with_protocol_name(mut self, name: impl Into<Cow<'static, [u8]>>) -> Self {
        self.protocol_name = name.into();
        self
    }
}

impl Default for BitTorrentProtocolConfig {
    fn default() -> Self {
        BitTorrentProtocolConfig {
            protocol_name: Cow::Borrowed(b"/btt/1.0.0"),
        }
    }
}

impl UpgradeInfo for BitTorrentProtocolConfig {
    type Info = Cow<'static, [u8]>;
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(self.protocol_name.clone())
    }
}

pub type BttStreamSink<S, A, B> = stream::AndThen<
    sink::With<
        stream::FromErr<Framed<S, UviBytes<Vec<u8>>>, io::Error>,
        A,
        fn(A) -> Result<Vec<u8>, io::Error>,
        Result<Vec<u8>, io::Error>,
    >,
    fn(BytesMut) -> Result<B, io::Error>,
    Result<B, io::Error>,
>;
