use bytes::BytesMut;
use codec::UviBytes;
use futures::{
    future::{self, FutureResult},
    sink, stream, Sink, Stream,
};
use libp2p_core::upgrade::{InboundUpgrade, Negotiated, OutboundUpgrade, UpgradeInfo};
use libp2p_core::{Multiaddr, PeerId};
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
    pub index: u32,
    /// begin offset
    pub begin: u32,
    /// the payload
    pub piece: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeerRequest {
    pub index: u32,
    /// begin offset
    pub begin: u32,
    /// in power of 2
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BttPeerMessage {
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have { index: u32 },
    Bitfield { index_field: Vec<u8> },
    Request { peer_request: BttPeerRequest },
    Cancel { peer_request: BttPeerRequest },
    Piece { piece: BttPiece },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeer {
    /// Identifier of the peer.
    pub node_id: PeerId,
    /// The multiaddresses that the sender think can be used in order to reach the peer.
    pub multiaddrs: Vec<Multiaddr>,
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
