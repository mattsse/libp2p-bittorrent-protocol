use crate::protobuf_structs::btt as proto;
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

impl From<proto::PeerMessage_Piece> for BttPiece {
    fn from(mut value: proto::PeerMessage_Piece) -> BttPiece {
        Self {
            index: value.index,
            begin: value.begin,
            piece: value.take_piece(),
        }
    }
}

impl Into<proto::PeerMessage_Piece> for BttPiece {
    fn into(self) -> proto::PeerMessage_Piece {
        let mut piece = proto::PeerMessage_Piece::new();
        piece.set_index(self.index);
        piece.set_begin(self.begin);
        piece.set_piece(self.piece);
        piece
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeerRequest {
    pub index: u32,
    /// begin offset
    pub begin: u32,
    /// in power of 2
    pub length: u32,
}

// Builds a `BttPeerRequest` from a corresponding protobuf message.
impl From<&proto::PeerMessage_Request> for BttPeerRequest {
    fn from(value: &proto::PeerMessage_Request) -> BttPeerRequest {
        Self {
            index: value.index,
            begin: value.begin,
            length: value.length,
        }
    }
}
impl Into<proto::PeerMessage_Request> for BttPeerRequest {
    fn into(self) -> proto::PeerMessage_Request {
        let mut req = proto::PeerMessage_Request::new();
        req.set_index(self.index);
        req.set_begin(self.begin);
        req.set_length(self.length);
        req
    }
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

impl From<proto::PeerMessage> for BttPeerMessage {
    fn from(mut value: proto::PeerMessage) -> BttPeerMessage {
        match value.field_type {
            proto::PeerMessage_MessageType::CHOKE => BttPeerMessage::Choke,
            proto::PeerMessage_MessageType::UN_CHOKE => BttPeerMessage::UnChoke,
            proto::PeerMessage_MessageType::INTERESTED => BttPeerMessage::Interested,
            proto::PeerMessage_MessageType::NOT_INTERESTED => BttPeerMessage::NotInterested,
            proto::PeerMessage_MessageType::HAVE => BttPeerMessage::Have {
                index: value.have_index,
            },
            proto::PeerMessage_MessageType::BITFIELD => BttPeerMessage::Bitfield {
                index_field: value.take_index_field(),
            },
            proto::PeerMessage_MessageType::REQUEST => BttPeerMessage::Request {
                peer_request: BttPeerRequest::from(&value.take_request()),
            },
            proto::PeerMessage_MessageType::CANCEL => BttPeerMessage::Cancel {
                peer_request: BttPeerRequest::from(&value.take_request()),
            },
            proto::PeerMessage_MessageType::PIECE => BttPeerMessage::Piece {
                piece: BttPiece::from(value.take_piece()),
            },
        }
    }
}

impl Into<proto::PeerMessage> for BttPeerMessage {
    fn into(self) -> proto::PeerMessage {
        let mut msg = proto::PeerMessage::new();
        match self {
            BttPeerMessage::Choke => {
                msg.set_field_type(proto::PeerMessage_MessageType::CHOKE);
            }
            BttPeerMessage::UnChoke => {
                msg.set_field_type(proto::PeerMessage_MessageType::UN_CHOKE);
            }
            BttPeerMessage::Interested => {
                msg.set_field_type(proto::PeerMessage_MessageType::INTERESTED);
            }
            BttPeerMessage::NotInterested => {
                msg.set_field_type(proto::PeerMessage_MessageType::NOT_INTERESTED);
            }
            BttPeerMessage::Have { index } => {
                msg.set_field_type(proto::PeerMessage_MessageType::HAVE);
                msg.set_have_index(index);
            }
            BttPeerMessage::Bitfield { index_field } => {
                msg.set_field_type(proto::PeerMessage_MessageType::BITFIELD);
                msg.set_index_field(index_field);
            }
            BttPeerMessage::Request { peer_request } => {
                msg.set_field_type(proto::PeerMessage_MessageType::REQUEST);
                msg.set_request(peer_request.into());
            }
            BttPeerMessage::Cancel { peer_request } => {
                msg.set_field_type(proto::PeerMessage_MessageType::CANCEL);
                msg.set_request(peer_request.into());
            }
            BttPeerMessage::Piece { piece } => {
                msg.set_field_type(proto::PeerMessage_MessageType::PIECE);
                msg.set_piece(piece.into());
            }
        }
        msg
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BttPeer {
    /// Identifier of the peer.
    pub node_id: PeerId,
    /// The multiaddresses that the sender think can be used in order to reach the peer.
    pub multiaddrs: Vec<Multiaddr>,
}

impl Into<proto::Peer> for BttPeer {
    fn into(self) -> proto::Peer {
        let mut out = proto::Peer::new();
        out.set_id(self.node_id.into_bytes());
        for addr in self.multiaddrs {
            out.mut_addrs().push(addr.to_vec());
        }
        out
    }
}

// Builds a `BttPeer` from a corresponding protobuf message.
impl TryFrom<&mut proto::Peer> for BttPeer {
    type Error = io::Error;

    fn try_from(peer: &mut proto::Peer) -> Result<BttPeer, Self::Error> {
        // TODO: this is in fact a CID; not sure if this should be handled in `from_bytes` or as a special case here
        let node_id = PeerId::from_bytes(peer.get_id().to_vec())
            .map_err(|_| invalid_data("invalid peer id"))?;

        let mut addrs = Vec::with_capacity(peer.get_addrs().len());
        for addr in peer.take_addrs().into_iter() {
            let as_ma = Multiaddr::try_from(addr).map_err(invalid_data)?;
            addrs.push(as_ma);
        }
        debug_assert_eq!(addrs.len(), addrs.capacity());

        Ok(BttPeer {
            node_id,
            multiaddrs: addrs,
        })
    }
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
