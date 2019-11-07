use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use futures::prelude::*;
use libp2p_core::{
    either::EitherOutput,
    upgrade::{self, InboundUpgrade, Negotiated, OutboundUpgrade},
};
use libp2p_swarm::{
    KeepAlive, ProtocolsHandler, ProtocolsHandlerEvent, ProtocolsHandlerUpgrErr, SubstreamProtocol,
};
use tokio_io::{AsyncRead, AsyncWrite};
use wasm_timer::Instant;

use crate::behavior::{Bittorrent, SeedLeechConfig};
use crate::bitfield::BitField;
use crate::disk::torrent::TorrentSeed;
use crate::error;
use crate::peer::torrent::{Torrent, TorrentId, TorrentPeer};
use crate::peer::{BttPeer, ChokeType, InterestType};
use crate::piece::Piece;
use crate::proto::message::{Handshake, PeerMessage, PeerRequest};
use crate::proto::{BittorrentProtocolConfig, BttStreamSink};
use crate::torrent::MetaInfo;
use crate::util::ShaHash;

/// Protocol handler that handles Bittorrent communications with the remote.
///
/// The handler will automatically open a Bittorrent substream with the remote
/// for each request we make.
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
    /// Next unique ID of a connection.
    next_connec_unique_id: UniqueConnecId,
}

impl<TSubstream, TUserData> BittorrentHandler<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// Create a `BittorrentHandler` that only allows leeching from remote but
    /// denying incoming piece request.
    pub fn leech_only() -> Self {
        BittorrentHandler::with_seed_leech_config(SeedLeechConfig::Leech)
    }

    /// Create a `BittorrentHandler` that only allows seeding to remotes but
    /// doesn't request any pieces.
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
            next_connec_unique_id: UniqueConnecId(0),
        }
    }
}

/// State of an active substream, opened either by us or by the remote.
enum SubstreamState<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// We haven't started opening the outgoing substream yet.
    /// Contains the request we want to send, and the user data if we expect an
    /// answer.
    OutPendingOpen(PeerMessage, Option<TUserData>),
    /// Waiting to send a message to the remote.
    OutPendingSend(BttStreamSink<TSubstream>, PeerMessage, Option<TUserData>),
    /// Waiting to flush the substream so that the data arrives to the remote.
    OutPendingFlush(BttStreamSink<TSubstream>, Option<TUserData>),
    /// Waiting for an answer back from the remote.
    OutWaitingAnswer(BttStreamSink<TSubstream>, TUserData, Instant),
    /// An error happened on the substream and we should report the error to the
    /// user.
    OutReportError(BittorrentHandlerTorrentErr, TUserData),
    /// The substream is being closed.
    OutClosing(BttStreamSink<TSubstream>),
    /// Waiting for a request from the remote.
    InWaitingMessage(UniqueConnecId, BttStreamSink<TSubstream>),
    /// Waiting for the user to send a `BittorrentHandlerIn` event containing
    /// the response.
    InWaitingUser(UniqueConnecId, BttStreamSink<TSubstream>),
    /// Waiting to send an answer back to the remote.
    InPendingSend(UniqueConnecId, BttStreamSink<TSubstream>, PeerMessage),
    /// Waiting to flush an answer back to the remote.
    InPendingFlush(UniqueConnecId, BttStreamSink<TSubstream>),
    /// The substream is being closed.
    InClosing(BttStreamSink<TSubstream>),
}

impl<TSubstream, TUserData> SubstreamState<TSubstream, TUserData>
where
    TSubstream: AsyncRead + AsyncWrite,
{
    /// Consumes this state and tries to close the substream.
    ///
    /// If the substream is not ready to be closed, returns it back.
    fn try_close(self) -> AsyncSink<Self> {
        match self {
            SubstreamState::OutPendingOpen(_, _) | SubstreamState::OutReportError(_, _) => {
                AsyncSink::Ready
            }
            SubstreamState::OutPendingSend(mut stream, _, _)
            | SubstreamState::OutPendingFlush(mut stream, _)
            | SubstreamState::OutWaitingAnswer(mut stream, _, _)
            | SubstreamState::OutClosing(mut stream) => match stream.close() {
                Ok(Async::Ready(())) | Err(_) => AsyncSink::Ready,
                Ok(Async::NotReady) => AsyncSink::NotReady(SubstreamState::OutClosing(stream)),
            },
            SubstreamState::InWaitingMessage(_, mut stream)
            | SubstreamState::InWaitingUser(_, mut stream)
            | SubstreamState::InPendingSend(_, mut stream, _)
            | SubstreamState::InPendingFlush(_, mut stream)
            | SubstreamState::InClosing(mut stream) => match stream.close() {
                Ok(Async::Ready(())) | Err(_) => AsyncSink::Ready,
                Ok(Async::NotReady) => AsyncSink::NotReady(SubstreamState::InClosing(stream)),
            },
        }
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
    // Message of the request to send to the remote, and user data if we expect an
    // answer.
    type OutboundOpenInfo = (PeerMessage, Option<TUserData>);

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol> {
        SubstreamProtocol::new(self.config.clone())
    }

    fn inject_fully_negotiated_inbound(
        &mut self,
        protocol: <Self::InboundProtocol as InboundUpgrade<TSubstream>>::Output,
    ) {
        let connec_unique_id = self.next_connec_unique_id;
        self.next_connec_unique_id.0 += 1;
        // waiting for the handshake
        self.substreams
            .push(SubstreamState::InWaitingMessage(connec_unique_id, protocol));
    }

    fn inject_fully_negotiated_outbound(
        &mut self,
        protocol: <Self::OutboundProtocol as OutboundUpgrade<TSubstream>>::Output,
        (msg, user_data): Self::OutboundOpenInfo,
    ) {
        self.substreams
            .push(SubstreamState::OutPendingSend(protocol, msg, user_data));
    }

    /// If a client receives a handshake with an info_hash that it is not
    /// currently serving, then the client must drop the connection. open
    /// one substream per request
    fn inject_event(&mut self, message: BittorrentHandlerIn<TUserData>) {
        match message {
            BittorrentHandlerIn::Reset(request_id) => {
                let pos = self.substreams.iter().position(|state| match state {
                    SubstreamState::InWaitingUser(conn_id, _) => {
                        conn_id == &request_id.connec_unique_id
                    }
                    _ => false,
                });
                if let Some(pos) = pos {
                    let _ = self.substreams.remove(pos).try_close();
                }
            }
            BittorrentHandlerIn::HandshakeReq {
                handshake,
                user_data,
            } => {
                // initial request to the remote
                self.substreams.push(SubstreamState::OutPendingOpen(
                    PeerMessage::Handshake { handshake },
                    Some(user_data),
                ));
            }
            BittorrentHandlerIn::BitfieldReq {
                index_field,
                user_data,
            } => {
                let msg = PeerMessage::Bitfield { index_field };
                self.substreams
                    .push(SubstreamState::OutPendingOpen(msg, Some(user_data)));
            }
            BittorrentHandlerIn::Choke { inner } => {
                let msg = match inner {
                    ChokeType::Choked => PeerMessage::Choke,
                    ChokeType::UnChoked => PeerMessage::UnChoke,
                };
                self.substreams
                    .push(SubstreamState::OutPendingOpen(msg, None));
            }
            BittorrentHandlerIn::Interest { inner } => {
                let msg = match inner {
                    InterestType::Interested => PeerMessage::Interested,
                    InterestType::NotInterested => PeerMessage::NotInterested,
                };
                self.substreams
                    .push(SubstreamState::OutPendingOpen(msg, None));
            }
            BittorrentHandlerIn::GetPieceReq { request, user_data } => {
                self.substreams.push(SubstreamState::OutPendingOpen(
                    PeerMessage::Request { request },
                    Some(user_data),
                ));
            }
            BittorrentHandlerIn::GetPieceRes { piece, request_id } => {
                let pos = self.substreams.iter().position(|state| match state {
                    SubstreamState::InWaitingUser(conn_id, _) => {
                        conn_id == &request_id.connec_unique_id
                    }
                    _ => false,
                });
                if let Some(pos) = pos {
                    let (conn_id, substream) = match self.substreams.remove(pos) {
                        SubstreamState::InWaitingUser(conn_id, substream) => (conn_id, substream),
                        _ => unreachable!(),
                    };
                    self.substreams.push(SubstreamState::InPendingSend(
                        conn_id,
                        substream,
                        PeerMessage::Piece { piece },
                    ));
                }
            }
            BittorrentHandlerIn::CancelPiece {
                request,
                request_id,
            } => {
                let pos = self.substreams.iter().position(|state| match state {
                    SubstreamState::InWaitingUser(conn_id, _) => {
                        conn_id == &request_id.connec_unique_id
                    }
                    _ => false,
                });
                if let Some(pos) = pos {
                    let _ = self.substreams.remove(pos).try_close();
                } else {
                    // piece request might be already sent so we send a cancel
                    self.substreams.push(SubstreamState::OutPendingOpen(
                        PeerMessage::Cancel { request },
                        None,
                    ));
                }
            }
            BittorrentHandlerIn::Have { index } => {
                self.substreams.push(SubstreamState::OutPendingOpen(
                    PeerMessage::Have { index },
                    None,
                ));
            }
            BittorrentHandlerIn::KeepAlive => {
                self.substreams
                    .push(SubstreamState::OutPendingOpen(PeerMessage::KeepAlive, None));
            }
            BittorrentHandlerIn::HandshakeRes {
                handshake,
                request_id,
            } => {
                let pos = self.substreams.iter().position(|state| match state {
                    SubstreamState::InWaitingUser(conn_id, _) => {
                        conn_id == &request_id.connec_unique_id
                    }
                    _ => false,
                });
                if let Some(pos) = pos {
                    let (conn_id, substream) = match self.substreams.remove(pos) {
                        SubstreamState::InWaitingUser(conn_id, substream) => (conn_id, substream),
                        _ => unreachable!(),
                    };

                    self.substreams.push(SubstreamState::InPendingSend(
                        conn_id,
                        substream,
                        PeerMessage::Handshake { handshake },
                    ));
                }
            }
            BittorrentHandlerIn::BitfieldRes {
                index_field,
                request_id,
            } => {
                let pos = self.substreams.iter().position(|state| match state {
                    SubstreamState::InWaitingUser(conn_id, _) => {
                        conn_id == &request_id.connec_unique_id
                    }
                    _ => false,
                });
                if let Some(pos) = pos {
                    let (conn_id, substream) = match self.substreams.remove(pos) {
                        SubstreamState::InWaitingUser(conn_id, substream) => (conn_id, substream),
                        _ => unreachable!(),
                    };

                    self.substreams.push(SubstreamState::InPendingSend(
                        conn_id,
                        substream,
                        PeerMessage::Bitfield { index_field },
                    ));
                }
            }
        }
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
    /// Resets the (sub)stream associated with the given request ID,
    /// thus signaling an error to the remote.
    ///
    /// Explicitly resetting the (sub)stream associated with a request
    /// can be used as an alternative to letting requests simply time
    /// out on the remote peer, thus potentially avoiding some delay
    /// for the query on the remote.
    Reset(BittorrentRequestId),
    /// The initial contact to a peer
    HandshakeReq {
        handshake: Handshake,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    /// The initial contact to a peer
    HandshakeRes {
        handshake: Handshake,
        request_id: BittorrentRequestId,
    },
    /// The bitfield message may only be sent immediately after the handshaking
    /// sequence is completed, and before any other messages are sent. It is
    /// optional, and need not be sent if a client has no pieces
    BitfieldReq {
        index_field: BitField,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    BitfieldRes {
        index_field: BitField,
        /// Custom data. Passed back in the out event when the results arrive.
        request_id: BittorrentRequestId,
    },
    Choke {
        inner: ChokeType,
    },
    Interest {
        inner: InterestType,
    },
    /// Request to retrieve a piece from the peers.
    GetPieceReq {
        /// The id of the block.
        request: PeerRequest,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    GetPieceRes {
        piece: Piece,
        /// Identifier of the request that was made by the remote.
        ///
        /// It is a logic error to use an id of the handler of a different node.
        request_id: BittorrentRequestId,
    },
    CancelPiece {
        request: PeerRequest,
        request_id: BittorrentRequestId,
    },
    Have {
        /// index of a piece we got
        index: u32,
    },
    KeepAlive,
}

/// Event produced by the Bittorrent handler.
#[derive(Debug)]
pub enum BittorrentHandlerEvent<TUserData> {
    /// The initial contact to a peer
    HandshakeReq {
        handshake: Handshake,
        request_id: BittorrentRequestId,
    },
    /// The initial contact to a peer
    HandshakeRes {
        handshake: Handshake,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    /// The bitfield message may only be sent immediately after the handshaking
    /// sequence is completed, and before any other messages are sent. It is
    /// optional, and need not be sent if a client has no pieces
    BitfieldReq {
        index_field: BitField,
        request_id: BittorrentRequestId,
    },
    BitfieldRes {
        index_field: BitField,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    /// Request to retrieve a piece.
    GetPieceReq {
        /// The id of the block.
        request: PeerRequest,
        /// Identifier of the request that was made by the remote.
        ///
        /// It is a logic error to use an id of the handler of a different node.
        request_id: BittorrentRequestId,
    },
    GetPieceRes {
        piece: Piece,
        /// Custom data. Passed back in the out event when the results arrive.
        user_data: TUserData,
    },
    CancelPiece {
        request: PeerRequest,
        /// Custom data. Passed back in the out event when the results arrive.
        request_id: BittorrentRequestId,
    },
    Choke {
        inner: ChokeType,
    },
    Interest {
        inner: InterestType,
    },
    Have {
        index: u32,
    },
    /// An error happened when torrenting.
    TorrentErr {
        /// The error that happened.
        error: BittorrentHandlerTorrentErr,
        /// The user data passed to the req.
        user_data: Option<TUserData>,
    },
    KeepAlive {
        timestamp: Instant,
    },
}

/// Unique identifier for a request. Must be passed back in order to answer a
/// request from the remote.
///
/// We don't implement `Clone` on purpose, in order to prevent users from
/// answering the same request twice.
#[derive(Debug, PartialEq, Eq)]
pub struct BittorrentRequestId {
    /// Unique identifier for an incoming connection.
    connec_unique_id: UniqueConnecId,
}

/// Unique identifier for a connection.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct UniqueConnecId(u64);

/// Error that can happen while torrenting.
#[derive(Debug)]
pub enum BittorrentHandlerTorrentErr {
    /// Error while trying to perform the query.
    Upgrade(ProtocolsHandlerUpgrErr<io::Error>),
    /// Received an answer that doesn't correspond to the request.
    UnexpectedMessage(PeerMessage),
    /// I/O error in the substream.
    Io(io::Error),
}

impl fmt::Display for BittorrentHandlerTorrentErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BittorrentHandlerTorrentErr::Upgrade(err) => {
                write!(f, "Error while performing Bittorrent action: {}", err)
            }
            BittorrentHandlerTorrentErr::UnexpectedMessage(_) => write!(
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
            BittorrentHandlerTorrentErr::UnexpectedMessage(_) => None,
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
/// Returns the new state for that substream, an event to generate, and whether
/// the substream should be polled again.
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
    match state {
        SubstreamState::OutPendingOpen(msg, user_data) => {
            let ev = ProtocolsHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(upgrade),
                info: (msg, user_data),
            };
            (None, Some(ev), false)
        }
        SubstreamState::OutPendingSend(mut substream, msg, user_data) => {
            match substream.start_send(msg) {
                Ok(AsyncSink::Ready) => (
                    Some(SubstreamState::OutPendingFlush(substream, user_data)),
                    None,
                    true,
                ),
                Ok(AsyncSink::NotReady(msg)) => (
                    Some(SubstreamState::OutPendingSend(substream, msg, user_data)),
                    None,
                    false,
                ),
                Err(error) => {
                    let event = if let Some(user_data) = user_data {
                        Some(ProtocolsHandlerEvent::Custom(
                            BittorrentHandlerEvent::TorrentErr {
                                error: BittorrentHandlerTorrentErr::Io(error),
                                user_data: Some(user_data),
                            },
                        ))
                    } else {
                        None
                    };

                    (None, event, false)
                }
            }
        }
        SubstreamState::OutPendingFlush(mut substream, user_data) => {
            match substream.poll_complete() {
                Ok(Async::Ready(())) => {
                    if let Some(user_data) = user_data {
                        (
                            Some(SubstreamState::OutWaitingAnswer(
                                substream,
                                user_data,
                                Instant::now(),
                            )),
                            None,
                            true,
                        )
                    } else {
                        (Some(SubstreamState::OutClosing(substream)), None, true)
                    }
                }
                Ok(Async::NotReady) => (
                    Some(SubstreamState::OutPendingFlush(substream, user_data)),
                    None,
                    false,
                ),
                Err(error) => {
                    let event = if let Some(user_data) = user_data {
                        Some(ProtocolsHandlerEvent::Custom(
                            BittorrentHandlerEvent::TorrentErr {
                                error: BittorrentHandlerTorrentErr::Io(error),
                                user_data: Some(user_data),
                            },
                        ))
                    } else {
                        None
                    };

                    (None, event, false)
                }
            }
        }
        SubstreamState::OutWaitingAnswer(mut substream, user_data, instant) => {
            match substream.poll() {
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
                    Some(SubstreamState::OutWaitingAnswer(
                        substream, user_data, instant,
                    )),
                    None,
                    false,
                ),
                Err(error) => {
                    let event = BittorrentHandlerEvent::TorrentErr {
                        error: BittorrentHandlerTorrentErr::Io(error),
                        user_data: Some(user_data),
                    };
                    (None, Some(ProtocolsHandlerEvent::Custom(event)), false)
                }
                Ok(Async::Ready(None)) => {
                    let event = BittorrentHandlerEvent::TorrentErr {
                        error: BittorrentHandlerTorrentErr::Io(io::ErrorKind::UnexpectedEof.into()),
                        user_data: Some(user_data),
                    };
                    (None, Some(ProtocolsHandlerEvent::Custom(event)), false)
                }
            }
        }
        SubstreamState::OutReportError(error, user_data) => {
            let event = BittorrentHandlerEvent::TorrentErr {
                error,
                user_data: Some(user_data),
            };
            (None, Some(ProtocolsHandlerEvent::Custom(event)), false)
        }
        SubstreamState::OutClosing(mut stream) => match stream.close() {
            Ok(Async::Ready(())) => (None, None, false),
            Ok(Async::NotReady) => (Some(SubstreamState::OutClosing(stream)), None, false),
            Err(_) => (None, None, false),
        },
        SubstreamState::InWaitingMessage(id, mut substream) => match substream.poll() {
            Ok(Async::Ready(Some(msg))) => {
                let ev = process_btt_in(msg, id);
                (
                    Some(SubstreamState::InWaitingUser(id, substream)),
                    Some(ProtocolsHandlerEvent::Custom(ev)),
                    false,
                )
            }
            Ok(Async::NotReady) => (
                Some(SubstreamState::InWaitingMessage(id, substream)),
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
        SubstreamState::InWaitingUser(id, substream) => (
            Some(SubstreamState::InWaitingUser(id, substream)),
            None,
            false,
        ),
        SubstreamState::InPendingSend(id, mut substream, msg) => match substream.start_send(msg) {
            Ok(AsyncSink::Ready) => (
                Some(SubstreamState::InPendingFlush(id, substream)),
                None,
                true,
            ),
            Ok(AsyncSink::NotReady(msg)) => (
                Some(SubstreamState::InPendingSend(id, substream, msg)),
                None,
                false,
            ),
            Err(_) => (None, None, false),
        },
        SubstreamState::InPendingFlush(id, mut substream) => match substream.poll_complete() {
            Ok(Async::Ready(())) => (
                Some(SubstreamState::InWaitingMessage(id, substream)),
                None,
                true,
            ),
            Ok(Async::NotReady) => (
                Some(SubstreamState::InPendingFlush(id, substream)),
                None,
                false,
            ),
            Err(_) => (None, None, false),
        },
        SubstreamState::InClosing(mut stream) => match stream.close() {
            Ok(Async::Ready(())) => (None, None, false),
            Ok(Async::NotReady) => (Some(SubstreamState::InClosing(stream)), None, false),
            Err(_) => (None, None, false),
        },
    }
}

/// Processes a Bittorrent message that's expected to be a request from a
/// remote.
fn process_btt_in<TUserData>(
    event: PeerMessage,
    connec_unique_id: UniqueConnecId,
) -> BittorrentHandlerEvent<TUserData> {
    match event {
        PeerMessage::Handshake { handshake } => BittorrentHandlerEvent::HandshakeReq {
            handshake,
            request_id: BittorrentRequestId { connec_unique_id },
        },
        PeerMessage::Bitfield { index_field } => BittorrentHandlerEvent::BitfieldReq {
            index_field,
            request_id: BittorrentRequestId { connec_unique_id },
        },

        PeerMessage::KeepAlive => BittorrentHandlerEvent::KeepAlive {
            timestamp: Instant::now(),
        },
        PeerMessage::Choke => BittorrentHandlerEvent::Choke {
            inner: ChokeType::Choked,
        },
        PeerMessage::UnChoke => BittorrentHandlerEvent::Choke {
            inner: ChokeType::UnChoked,
        },
        PeerMessage::Interested => BittorrentHandlerEvent::Interest {
            inner: InterestType::Interested,
        },
        PeerMessage::NotInterested => BittorrentHandlerEvent::Interest {
            inner: InterestType::NotInterested,
        },
        PeerMessage::Have { index } => BittorrentHandlerEvent::Have { index },
        PeerMessage::Request { request } => BittorrentHandlerEvent::GetPieceReq {
            request,
            request_id: BittorrentRequestId { connec_unique_id },
        },
        PeerMessage::Cancel { request } => BittorrentHandlerEvent::CancelPiece {
            request,
            request_id: BittorrentRequestId { connec_unique_id },
        },
        msg @ PeerMessage::Piece { .. } => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: None,
        },
        PeerMessage::Port { port } => {
            // TODO since we rely on the `PeerId` there is no need for this
            unimplemented!()
        }
    }
}

/// Process a Bittorrent message that's supposed to be a response to one of our
/// requests. Since we open a new substream for each message, receiving a
/// message or an request shouldn't happen and we return an error instead
fn process_btt_out<TUserData>(
    msg: PeerMessage,
    user_data: TUserData,
) -> BittorrentHandlerEvent<TUserData> {
    match msg {
        msg @ PeerMessage::KeepAlive => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::Choke => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::UnChoke => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::Interested => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::NotInterested => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::Have { .. } => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        PeerMessage::Bitfield { index_field } => BittorrentHandlerEvent::BitfieldRes {
            index_field,
            user_data,
        },
        msg @ PeerMessage::Request { .. } => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        msg @ PeerMessage::Cancel { .. } => BittorrentHandlerEvent::TorrentErr {
            error: BittorrentHandlerTorrentErr::UnexpectedMessage(msg),
            user_data: Some(user_data),
        },
        PeerMessage::Piece { piece } => BittorrentHandlerEvent::GetPieceRes { piece, user_data },
        PeerMessage::Handshake { handshake } => BittorrentHandlerEvent::HandshakeRes {
            handshake,
            user_data,
        },
        PeerMessage::Port { .. } => unimplemented!(),
    }
}
