use crate::protobuf_structs::btt as proto;
use crate::protobuf_structs::btt::TrackerRequest;
use crate::protocol::BttPeer;
use sha1::Sha1;
use std::convert::TryInto;
use std::io;
use std::ops::Add;
use std::{convert::TryFrom, time::Duration};
use wasm_timer::Instant;

// TODO should tracker be its own protocol?

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum BttEventType {
    Started = 1,
    Completed = 2,
    Stopped = 3,
}

impl BttEventType {
    /// convert from protobuf event type
    pub(crate) fn proto_to_event(event: proto::TrackerRequest_EventType) -> Option<Self> {
        match event {
            proto::TrackerRequest_EventType::EMPTY => None,
            proto::TrackerRequest_EventType::STARTED => Some(BttEventType::Started),
            proto::TrackerRequest_EventType::COMPLETED => Some(BttEventType::Completed),
            proto::TrackerRequest_EventType::STOPPED => Some(BttEventType::Stopped),
        }
    }
}

impl Into<proto::TrackerRequest_EventType> for BttEventType {
    fn into(self) -> proto::TrackerRequest_EventType {
        match self {
            BttEventType::Started => proto::TrackerRequest_EventType::STARTED,
            BttEventType::Completed => proto::TrackerRequest_EventType::COMPLETED,
            BttEventType::Stopped => proto::TrackerRequest_EventType::STOPPED,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BttTrackerRequestMsg {
    pub info_hash: Sha1,

    pub peer: BttPeer,

    pub uploaded: u64,

    pub downloaded: u64,

    pub left: u64,

    pub numwant: u32,

    pub event: Option<BttEventType>,
}

impl TryFrom<proto::TrackerRequest> for BttTrackerRequestMsg {
    type Error = io::Error;

    fn try_from(mut value: TrackerRequest) -> Result<Self, Self::Error> {
        let peer = BttPeer::try_from(&mut value.take_peer())?;
        let event = BttEventType::proto_to_event(value.event);

        Ok(Self {
            info_hash: Sha1::from(value.info_hash),
            uploaded: value.uploaded,
            downloaded: value.downloaded,
            left: value.left,
            numwant: value.numwant,
            peer,
            event,
        })
    }
}

impl Into<proto::TrackerRequest> for BttTrackerRequestMsg {
    fn into(self) -> proto::TrackerRequest {
        let mut req = proto::TrackerRequest::new();
        req.set_info_hash(self.info_hash.digest().bytes().to_vec());
        req.set_peer(self.peer.into());
        req.set_downloaded(self.downloaded);
        req.set_uploaded(self.uploaded);
        req.set_left(self.left);
        req.set_numwant(self.numwant);
        req.set_event(
            self.event
                .map(Into::into)
                .unwrap_or(proto::TrackerRequest_EventType::EMPTY),
        );
        req
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BttTrackerResponseMsg {
    Failure {
        reason: String,
    },
    Success {
        /// amount of seeders
        complete: u32,
        /// amount of leechers
        incomplete: u32,
        /// time after when the downloader should send rerequest
        interval: Instant,
        /// Minimum announce interval
        min_interval: Option<Duration>,
        /// matching peers
        peers: Vec<BttPeer>,
    },
}

impl TryFrom<proto::TrackerResponse> for BttTrackerResponseMsg {
    type Error = io::Error;

    fn try_from(mut value: proto::TrackerResponse) -> Result<Self, Self::Error> {
        if proto::TrackerResponse_TrackerResponseType::FAILURE == value.field_type {
            Ok(BttTrackerResponseMsg::Failure {
                reason: value.take_failure_reason(),
            })
        } else {
            let interval = Instant::now().add(Duration::from_secs(u64::from(value.interval)));
            let min_interval = if value.min_interval == 0 {
                None
            } else {
                Some(Duration::from_secs(u64::from(value.min_interval)))
            };

            let peers: Result<Vec<_>, _> = value
                .take_peers()
                .iter_mut()
                .map(BttPeer::try_from)
                .collect();

            Ok(BttTrackerResponseMsg::Success {
                complete: value.complete,
                incomplete: value.incomplete,
                peers: peers?,
                interval,
                min_interval,
            })
        }
    }
}

impl Into<proto::TrackerResponse> for BttTrackerResponseMsg {
    fn into(self) -> proto::TrackerResponse {
        let mut resp = proto::TrackerResponse::new();
        match self {
            BttTrackerResponseMsg::Failure { reason } => {
                resp.set_field_type(proto::TrackerResponse_TrackerResponseType::FAILURE);
                resp.set_failure_reason(reason);
            }
            BttTrackerResponseMsg::Success {
                complete,
                incomplete,
                interval,
                min_interval,
                peers,
            } => {
                resp.set_field_type(proto::TrackerResponse_TrackerResponseType::SUCCESS);
                resp.set_complete(complete);
                resp.set_incomplete(incomplete);
                resp.set_interval((interval - Instant::now()).as_secs() as u32);
                resp.set_min_interval(min_interval.map(|x| x.as_secs() as u32).unwrap_or(0));
                resp.set_peers(peers.into_iter().map(BttPeer::into).collect());
            }
        }
        resp
    }
}
