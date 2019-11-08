use std::{borrow::Cow, convert::TryFrom, time::Duration};
use std::{io, iter};

use bytes::BytesMut;
use futures::{
    future::{self, FutureResult},
    sink, stream, Sink, Stream,
};
use libp2p_core::upgrade::{InboundUpgrade, Negotiated, OutboundUpgrade, UpgradeInfo};
use libp2p_core::{Multiaddr, PeerId};
use sha1::Sha1;
use tokio_codec::Framed;
use tokio_io::{AsyncRead, AsyncWrite};
use wasm_timer::Instant;

use crate::proto::codec::PeerWireCodec;
use crate::proto::message::PeerMessage;

pub mod codec;
pub mod message;
pub mod tracker;

/// Creates an `io::Error` with `io::ErrorKind::InvalidData`.
fn invalid_data<E>(e: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, e)
}

#[derive(Debug, Clone)]
pub struct BittorrentProtocolConfig {
    protocol_name: Cow<'static, [u8]>,
}

impl BittorrentProtocolConfig {
    /// Modifies the protocol name used on the wire. Can be used to create
    /// incompatibilities between networks on purpose.
    pub fn with_protocol_name(mut self, name: impl Into<Cow<'static, [u8]>>) -> Self {
        self.protocol_name = name.into();
        self
    }
}

impl Default for BittorrentProtocolConfig {
    fn default() -> Self {
        BittorrentProtocolConfig {
            protocol_name: Cow::Borrowed(b"/btt/1.0.0"),
        }
    }
}

impl UpgradeInfo for BittorrentProtocolConfig {
    type Info = Cow<'static, [u8]>;
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(self.protocol_name.clone())
    }
}

/// Stream and Sink of `PeerMessage`
pub type BttStreamSink<S> = stream::FromErr<Framed<S, PeerWireCodec>, io::Error>;

impl<C> InboundUpgrade<C> for BittorrentProtocolConfig
where
    C: AsyncRead + AsyncWrite,
{
    type Output = BttStreamSink<Negotiated<C>>;
    type Error = io::Error;
    type Future = FutureResult<Self::Output, io::Error>;

    #[inline]
    fn upgrade_inbound(self, socket: Negotiated<C>, info: Self::Info) -> Self::Future {
        let mut codec = Default::default();
        future::ok(Framed::new(socket, codec).from_err())
    }
}

impl<C> OutboundUpgrade<C> for BittorrentProtocolConfig
where
    C: AsyncRead + AsyncWrite,
{
    type Output = BttStreamSink<Negotiated<C>>;
    type Error = io::Error;
    type Future = FutureResult<Self::Output, io::Error>;

    #[inline]
    fn upgrade_outbound(self, socket: Negotiated<C>, info: Self::Info) -> Self::Future {
        let mut codec = PeerWireCodec::default();
        future::ok(Framed::new(socket, codec).from_err())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use futures::{Future, Sink, Stream};
    use libp2p_core::{PeerId, PublicKey, Transport};
    use libp2p_tcp::TcpConfig;
    use tokio::runtime::Runtime;

    use crate::bitfield::BitField;
    use crate::piece::Piece;
    use crate::proto::message::{Handshake, PeerRequest};
    use crate::util::ShaHash;

    use super::*;

    /// We open a server and a client, send a message between the two, and check
    /// that they were successfully received.
    #[test]
    fn correct_transfer() {
        test_one(PeerMessage::Handshake {
            handshake: Handshake::new_with_random_id(ShaHash::random()),
        });

        test_one(PeerMessage::KeepAlive);
        test_one(PeerMessage::Choke);
        test_one(PeerMessage::UnChoke);
        test_one(PeerMessage::Interested);
        test_one(PeerMessage::NotInterested);
        test_one(PeerMessage::Have { index: 100 });
        test_one(PeerMessage::Bitfield {
            index_field: BitField::from_bytes(&[0b10100000, 0b00010010]),
        });
        test_one(PeerMessage::Request {
            request: PeerRequest {
                index: 1,
                begin: 2,
                length: 16384,
            },
        });
        test_one(PeerMessage::Piece {
            piece: Piece {
                index: 1,
                begin: 2,
                block: std::iter::repeat(1).take(16384).collect(),
            },
        });
        test_one(PeerMessage::Cancel {
            request: PeerRequest {
                index: 1,
                begin: 2,
                length: 16384,
            },
        });
        test_one(PeerMessage::Port { port: 8080 });

        fn test_one(msg_server: PeerMessage) {
            let msg_client = msg_server.clone();
            let (tx, rx) = mpsc::channel();

            let bg_thread = thread::spawn(move || {
                let transport = TcpConfig::new().with_upgrade(BittorrentProtocolConfig::default());

                let addr: Multiaddr = "/ip4/127.0.0.1/tcp/20500".parse().unwrap();

                let listener = transport.listen_on(addr.clone()).unwrap();

                tx.send(addr).unwrap();

                let future = listener
                    .into_future()
                    .and_then(|(_, listener)| {
                        // skip the `NewAddress` msg
                        listener.into_future()
                    })
                    .map_err(|(err, _)| err)
                    .and_then(|(client, _)| client.unwrap().into_upgrade().unwrap().0)
                    .map_err(|_| ())
                    .and_then(|proto| proto.into_future().map_err(|_| ()))
                    .map(move |(recv_msg, _)| {
                        assert_eq!(recv_msg.unwrap(), msg_server);
                        ()
                    });
                let mut rt = Runtime::new().unwrap();
                let _ = rt.block_on(future).unwrap();
            });

            let transport = TcpConfig::new().with_upgrade(BittorrentProtocolConfig::default());

            let future = transport
                .dial(rx.recv().unwrap())
                .unwrap()
                .map_err(|err| ())
                .and_then(|proto| proto.send(msg_client).map_err(|_| ()))
                .map(|_| ());
            let mut rt = Runtime::new().unwrap();
            let _ = rt.block_on(future).unwrap();
            bg_thread.join().unwrap();
        }
    }
}
