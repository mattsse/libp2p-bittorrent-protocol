use std::{borrow::Cow, convert::TryFrom, time::Duration};
use std::{io, iter};

use bytes::BytesMut;
use futures::{
    future::{self, FutureResult},
    sink,
    stream,
    Sink,
    Stream,
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

#[derive(Debug, Clone)]
pub struct BitTorrentProtocolConfig {
    protocol_name: Cow<'static, [u8]>,
}

impl BitTorrentProtocolConfig {
    /// Modifies the protocol name used on the wire. Can be used to create
    /// incompatibilities between networks on purpose.
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

/// Stream and Sink of `PeerMessage`
pub type BttStreamSink<S> = Framed<S, PeerWireCodec>;

impl<C> InboundUpgrade<C> for BitTorrentProtocolConfig
where
    C: AsyncRead + AsyncWrite,
{
    type Output = BttStreamSink<Negotiated<C>>;
    type Error = io::Error;
    type Future = FutureResult<Self::Output, io::Error>;

    #[inline]
    fn upgrade_inbound(self, socket: Negotiated<C>, info: Self::Info) -> Self::Future {
        let mut codec = Default::default();
        future::ok(Framed::new(socket, codec))
    }
}

impl<C> OutboundUpgrade<C> for BitTorrentProtocolConfig
where
    C: AsyncRead + AsyncWrite,
{
    type Output = BttStreamSink<Negotiated<C>>;
    type Error = io::Error;
    type Future = FutureResult<Self::Output, io::Error>;

    #[inline]
    fn upgrade_outbound(self, socket: Negotiated<C>, info: Self::Info) -> Self::Future {
        let mut codec = PeerWireCodec::default();
        future::ok(Framed::new(socket, codec))
    }
}
