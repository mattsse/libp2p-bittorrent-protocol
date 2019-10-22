pub mod codec;
pub mod message;
pub mod tracker;

use bytes::BytesMut;
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
use wasm_timer::Instant;

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
            protocol_name: Cow::Borrowed(b"//1.0.0"),
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

//pub type BttStreamSink<S, A> = stream::AndThen<
//    sink::With<
//        stream::FromErr<Framed<S, UviBytes<Vec<u8>>>, io::Error>,
//        A,
//        fn(A) -> Result<Vec<u8>, io::Error>,
//        Result<Vec<u8>, io::Error>,
//    >,
//    fn(BytesMut) -> Result<A, io::Error>,
//    Result<A, io::Error>,
//>;

// T
