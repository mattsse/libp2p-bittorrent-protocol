use crate::proto::BitTorrentProtocolConfig;
use libp2p_core::Negotiated;
use libp2p_swarm::KeepAlive;
use tokio_io::{AsyncRead, AsyncWrite};

///// Protocol handler that handles Bittorrent communications with the remote.
/////
///// The handler will automatically open a Bittorrent substream with the remote for each request we
///// make.
/////
///// It also handles requests made by the remote.
//pub struct BittorrentHandler<TSubstream, TUserData>
//    where
//        TSubstream: AsyncRead + AsyncWrite,
//{
//    /// Configuration for the Bittorrent protocol.
//    config: BitTorrentProtocolConfig,
//    /// If false, we always refuse incoming Bittorrent substreams.
//    allow_listening: bool,
//    /// List of active substreams with the state they are in.
//    substreams: Vec<SubstreamState<Negotiated<TSubstream>, TUserData>>,
//    /// Until when to keep the connection alive.
//    keep_alive: KeepAlive,
//
//}

///// State of an active substream, opened either by us or by the remote.
//enum SubstreamState<TSubstream, TUserData>
//    where
//        TSubstream: AsyncRead + AsyncWrite,
//{
//
//
//}
