use crate::behavior::{Bittorrent, SeedLeechConfig};
use crate::bitfield::BitField;
use crate::disk::torrent::TorrentSeed;
use crate::error;
use crate::peer::piece::{Torrent, TorrentId, TorrentPeer};
use crate::peer::{BttPeer, ChokeType, InterestType};
use crate::piece::Piece;
use crate::proto::message::{Handshake, PeerMessage, PeerRequest};
use crate::proto::{BittorrentProtocolConfig, BttStreamSink};
use crate::torrent::MetaInfo;
use crate::util::ShaHash;
use futures::prelude::*;
use libp2p_core::{
    either::EitherOutput,
    upgrade::{self, InboundUpgrade, Negotiated, OutboundUpgrade},
};
use libp2p_swarm::{
    KeepAlive, ProtocolsHandler, ProtocolsHandlerEvent, ProtocolsHandlerUpgrErr, SubstreamProtocol,
};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use tokio_io::{AsyncRead, AsyncWrite};
use wasm_timer::Instant;

/// Protocol handler that handles Bittorrent communications with the remote.
///
/// The handler will automatically open a Bittorrent substream with the remote for each request we
/// make.
///
/// It also handles requests made by the remote.
pub struct BittorrentHandler<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// Configuration for the Bittorrent protocol.
    config: BittorrentProtocolConfig,
    /// List of active substreams with the state they are in.
    substreams: Vec<SubstreamState<Negotiated<TSubstream>, TUserData>>,
    /// Until when to keep the connection alive.
    keep_alive: KeepAlive,
    /// seeding, leeching or both
    seed_leech: SeedLeechConfig,
}

impl<TSubstream, TUserData> BittorrentHandler<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// Create a `BittorrentHandler` that only allows leeching from remote but denying
    /// incoming piece request.
    pub fn leech_only() -> Self {
        BittorrentHandler::with_seed_leech_config(SeedLeechConfig::Leech)
    }

    /// Create a `BittorrentHandler` that only allows seeding to remotes but doesn't request any pieces.
    pub fn seed_only() -> Self {
        BittorrentHandler::with_seed_leech_config(SeedLeechConfig::Seed)
    }

    /// Create a `BittorrentHandler` that seeds and also leeches.
    pub fn seed_and_leech() -> Self {
        BittorrentHandler::with_seed_leech_config(SeedLeechConfig::Both)
    }

    pub fn with_seed_leech_config(seed_leech: SeedLeechConfig) -> Self {
        BittorrentHandler {
            config: Default::default(),
            substreams: Vec::new(),
            keep_alive: KeepAlive::Yes,
            seed_leech: Default::default(),
        }
    }
}

/// State of an active substream, opened either by us or by the remote.
enum SubstreamState<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// We haven't started opening the outgoing substream yet.
    /// Contains the request we want to send, and the user data if we expect an answer.
    OutPendingOpen(PeerMessage, Option<TUserData>),
    /// Waiting to send a message to the remote.
    OutPendingSend(BttStreamSink<TSubstream>, PeerMessage, Option<TUserData>),
    /// An error happened on the substream and we should report the error to the user.
    OutReportError(BittorrentHandlerTorrentErr, TUserData),
    /// The substream is being closed.
    OutClosing(BttStreamSink<TSubstream>),
    /// Waiting to flush the substream so that the data arrives to the remote.
    OutPendingFlush(BttStreamSink<TSubstream>, Option<TUserData>),
    /// Waiting for an answer back from the remote.
    OutWaitingAnswer(BttStreamSink<TSubstream>, TUserData),
    /// Waiting for the user to send a `BittorrentHandlerIn` event containing the response.
    InWaitingUser(BttStreamSink<TSubstream>),
    /// The substream is being closed.
    InClosing(BttStreamSink<TSubstream>),
    /// Waiting for a request from the remote.
    InWaitingMessage(BttStreamSink<TSubstream>),
}

impl<TSubstream, TUserData> SubstreamState<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// Consumes this state and tries to close the substream.
    ///
    /// If the substream is not ready to be closed, returns it back.
    fn try_close(self) -> AsyncSink<Self> {
        unimplemented!()
    }
}

impl<TSubstream, TUserData> ProtocolsHandler for BittorrentHandler<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
    TUserData: Clone,
{
    type InEvent = BittorrentHandlerIn<TUserData>;
    type OutEvent = BittorrentHandlerEvent<TUserData>;
    type Error = error::Error;
    type Substream = TSubstream;
    type InboundProtocol = BittorrentProtocolConfig;
    type OutboundProtocol = BittorrentProtocolConfig;
    // Message of the request to send to the remote, and user data if we expect an answer.
    type OutboundOpenInfo = (PeerMessage, Option<TUserData>);

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol> {
        SubstreamProtocol::new(self.config.clone())
    }

    fn inject_fully_negotiated_inbound(
        &mut self,
        protocol: <Self::InboundProtocol as InboundUpgrade<TSubstream>>::Output,
    ) {
        self.substreams
            .push(SubstreamState::InWaitingMessage(protocol));
    }

    fn inject_fully_negotiated_outbound(
        &mut self,
        protocol: <Self::OutboundProtocol as OutboundUpgrade<TSubstream>>::Output,
        (msg, user_data): Self::OutboundOpenInfo,
    ) {
        self.substreams
            .push(SubstreamState::OutPendingSend(protocol, msg, user_data));
    }

    fn inject_event(&mut self, message: BittorrentHandlerIn<TUserData>) {
        let _ = match message {
            _ => (),
        };

        unimplemented!()
    }

    #[inline]
    fn inject_dial_upgrade_error(
        &mut self,
        (_, user_data): Self::OutboundOpenInfo,
        error: ProtocolsHandlerUpgrErr<io::Error>,
    ) {
        // continue trying
        if let Some(user_data) = user_data {
            self.substreams
                .push(SubstreamState::OutReportError(error.into(), user_data));
        }
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        self.keep_alive
    }

    fn poll(
        &mut self,
    ) -> Poll<
        ProtocolsHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::OutEvent>,
        error::Error,
    > {
        // We remove each element from `substreams` one by one and add them back.
        for n in (0..self.substreams.len()).rev() {
            let mut substream = self.substreams.swap_remove(n);
            loop {
                match advance_substream(substream, self.config.clone()) {
                    (Some(new_state), Some(event), _) => {
                        self.substreams.push(new_state);
                        return Ok(Async::Ready(event));
                    }
                    (None, Some(event), _) => {
                        return Ok(Async::Ready(event));
                    }
                    (Some(new_state), None, false) => {
                        self.substreams.push(new_state);
                        break;
                    }
                    (Some(new_state), None, true) => {
                        substream = new_state;
                        continue;
                    }
                    (None, None, _) => {
                        break;
                    }
                }
            }
        }

        if self.substreams.is_empty() {
            self.keep_alive = KeepAlive::Until(Instant::now() + Duration::from_secs(20));
        } else {
            self.keep_alive = KeepAlive::Yes;
        }

        Ok(Async::NotReady)
    }
}
/// Event to send to the handler.
#[derive(Debug)]
pub enum BittorrentHandlerIn<TUserData> {
    /// The initial contact to a peer
    HandshakeIn {
        peer_id: ShaHash,
        info_hash: ShaHash,
    },
    /// Request to retrieve a piece from the peers.
    PieceReq {
        /// The id of the block.
        key: TorrentId,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    HandshakeOut {
        info_hash: ShaHash,
    },
    Bitfield {
        info_hash: ShaHash,
        peer_id: ShaHash,
    },
    PieceRes {
        piece: Piece,
        user_data: TUserData,
    },
    CancelPiece {
        request: PeerRequest,
        user_data: TUserData,
    },
    HaveIn {
        index: u32,
        user_data: TUserData,
    },
    HaveOut {
        index: u32,
        user_data: TUserData,
    },
    KeepAlive,
}

/// Event produced by the Bittorrent handler.
#[derive(Debug)]
pub enum BittorrentHandlerEvent<TUserData> {
    /// The initial contact to a peer
    HandshakeOut {
        /// the peer to send the handshake
        peer: TorrentPeer,
    },
    /// The initial contact to a peer
    HandshakeIn {
        /// the peer a handshake was received
        handshake: Handshake,
    },
    BitfieldIn {
        /// bitfield representing the pieces that have been successfully downloaded
        index_field: BitField,
    },
    ChokeIn {
        inner: ChokeType,
    },
    ChokeOut {
        inner: ChokeType,
    },
    InterestIn {
        inner: InterestType,
    },
    InterestOut {
        inner: InterestType,
    },
    HaveIn {
        inner: u32,
    },
    HaveOut {
        inner: u32,
    },
    /// An error happened when torrenting.
    TorrentErr {
        /// The error that happened.
        error: BittorrentHandlerTorrentErr,
        /// The user data passed to the query.
        user_data: TUserData,
    },
    TorrentFinished {
        path: PathBuf,
    },
    TorrentSubfileFinished {
        path: PathBuf,
    },
    AddTorrentSeed {
        seed: TorrentSeed,
    },
    AddTorrentLeech {
        meta: MetaInfo,
    },
    /// successfully sent a `KeepAlive` message
    KeptAlive,
    /// received a `KeepAlive` message
    KeepAlive,
}

/// Error that can happen while torrenting.
#[derive(Debug)]
pub enum BittorrentHandlerTorrentErr {
    /// Error while trying to perform the query.
    Upgrade(ProtocolsHandlerUpgrErr<io::Error>),
    /// Received an answer that doesn't correspond to the request.
    UnexpectedMessage,
    /// I/O error in the substream.
    Io(io::Error),
}

impl fmt::Display for BittorrentHandlerTorrentErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BittorrentHandlerTorrentErr::Upgrade(err) => {
                write!(f, "Error while performing Bittorrent action: {}", err)
            }
            BittorrentHandlerTorrentErr::UnexpectedMessage => write!(
                f,
                "Remote answered our Bittorrent request with the wrong message type"
            ),
            BittorrentHandlerTorrentErr::Io(err) => {
                write!(f, "I/O error during a Bittorrent action: {}", err)
            }
        }
    }
}

impl std::error::Error for BittorrentHandlerTorrentErr {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BittorrentHandlerTorrentErr::Upgrade(err) => Some(err),
            BittorrentHandlerTorrentErr::UnexpectedMessage => None,
            BittorrentHandlerTorrentErr::Io(err) => Some(err),
        }
    }
}

impl From<ProtocolsHandlerUpgrErr<io::Error>> for BittorrentHandlerTorrentErr {
    #[inline]
    fn from(err: ProtocolsHandlerUpgrErr<io::Error>) -> Self {
        BittorrentHandlerTorrentErr::Upgrade(err)
    }
}

/// Advances one substream.
///
/// Returns the new state for that substream, an event to generate, and whether the substream
/// should be polled again.
fn advance_substream<TSubstream, TUserData>(
    state: SubstreamState<TSubstream, TUserData>,
    upgrade: BittorrentProtocolConfig,
) -> (
    Option<SubstreamState<TSubstream, TUserData>>,
    Option<
        ProtocolsHandlerEvent<
            BittorrentProtocolConfig,
            (PeerMessage, Option<TUserData>),
            BittorrentHandlerEvent<TUserData>,
        >,
    >,
    bool,
)
where
    TSubstream: AsyncRead + AsyncWrite,
{
    let _ = match state {
        SubstreamState::OutPendingOpen(msg, user_data) => {
            let ev = ProtocolsHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(upgrade),
                info: (msg, user_data),
            };
            (None, Some(ev), false)
        }
        SubstreamState::OutWaitingAnswer(mut substream, user_data) => match substream.poll() {
            Ok(Async::Ready(Some(msg))) => {
                let new_state = SubstreamState::OutClosing(substream);
                let event = process_btt_out(msg, user_data);
                (
                    Some(new_state),
                    Some(ProtocolsHandlerEvent::Custom(event)),
                    true,
                )
            }
            Ok(Async::NotReady) => (
                Some(SubstreamState::OutWaitingAnswer(substream, user_data)),
                None,
                false,
            ),
            Err(error) => {
                let event = BittorrentHandlerEvent::TorrentErr {
                    error: BittorrentHandlerTorrentErr::Io(error),
                    user_data,
                };
                (None, Some(ProtocolsHandlerEvent::Custom(event)), false)
            }
            Ok(Async::Ready(None)) => {
                let event = BittorrentHandlerEvent::TorrentErr {
                    error: BittorrentHandlerTorrentErr::Io(io::ErrorKind::UnexpectedEof.into()),
                    user_data,
                };
                (None, Some(ProtocolsHandlerEvent::Custom(event)), false)
            }
        },

        SubstreamState::InWaitingMessage(mut substream) => match substream.poll() {
            Ok(Async::Ready(Some(msg))) => {
                if let Ok(ev) = process_btt_in(msg) {
                    (
                        Some(SubstreamState::InWaitingUser(substream)),
                        Some(ProtocolsHandlerEvent::Custom(ev)),
                        false,
                    )
                } else {
                    (Some(SubstreamState::InClosing(substream)), None, true)
                }
            }
            Ok(Async::NotReady) => (
                Some(SubstreamState::InWaitingMessage(substream)),
                None,
                false,
            ),
            Ok(Async::Ready(None)) => {
                trace!("Inbound substream: EOF");
                (None, None, false)
            }
            Err(e) => {
                trace!("Inbound substream error: {:?}", e);
                (None, None, false)
            }
        },
        SubstreamState::InWaitingUser(substream) => {
            (Some(SubstreamState::InWaitingUser(substream)), None, false)
        }

        _ => unreachable!(),
    };

    // TODO
    unimplemented!()
}

/// Processes a Bittorrent message that's expected to be a request from a remote.
fn process_btt_in<TUserData>(
    event: PeerMessage,
) -> Result<BittorrentHandlerEvent<TUserData>, io::Error> {
    match event {
        PeerMessage::Handshake { handshake } => {
            Ok(BittorrentHandlerEvent::HandshakeIn { handshake })
        }
        PeerMessage::Bitfield { index_field } => {
            Ok(BittorrentHandlerEvent::BitfieldIn { index_field })
        }
        _ => unreachable!(),
    }
}

/// Process a Bittorrent message that's supposed to be a response to one of our requests.
fn process_btt_out<TUserData>(
    event: PeerMessage,
    user_data: TUserData,
) -> BittorrentHandlerEvent<TUserData> {
    let _ = match event {
        //        PeerMessage::Piece { piece } => BittorrentHandlerEvent::PieceIn { piece, user_data },
        //        PeerMessage::Have { index } => BittorrentHandlerEvent::Have { index, user_data },
        _ => unreachable!(),
    };

    unimplemented!()
}
