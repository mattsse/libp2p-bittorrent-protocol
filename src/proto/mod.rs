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
            protocol_name: Cow::Borrowed(b"//1.0.0"),
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
