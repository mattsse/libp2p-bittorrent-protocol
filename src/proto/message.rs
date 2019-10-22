use crate::bitfield::BitField;
use crate::pieces::Piece;
use crate::util::ShaHash;
use byteorder::{BigEndian, WriteBytesExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRequest {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// specifying the requested length.
    pub length: u32,
}

/// All of the remaining messages in the protocol take the form of <length prefix><message ID><payload>.
/// The length prefix is a four byte big-endian value. The message ID is a single decimal byte.
/// integers in the peer wire protocol are encoded as four byte big-endian values
#[derive(Clone, PartialEq, Eq)]
pub enum PeerMessage {
    /// heartbeat generally 2 minute interval
    KeepAlive,
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield {
        /// bitfield representing the pieces that have been successfully downloaded
        index_field: BitField,
    },
    Request {
        peer_request: PeerRequest,
    },
    Cancel {
        peer_request: PeerRequest,
    },
    Piece {
        piece: Piece,
    },
    Handshake {
        handshake: Handshake,
    },
    /// he port message is sent by newer versions of the Mainline that implements a DHT tracker.
    /// The listen port is the port this peer's DHT node is listening on.
    /// This peer should be inserted in the local routing table (if DHT tracker is supported).
    Port {
        port: u32,
    },
}

impl PeerMessage {
    /// length that should be reserved for serialising the message.
    /// Besides `PeerMessage::Handshake` all messages are prefixed by its length (4 byte big endian).
    /// Besides `PeerMessage::KeepAlive` and `PeerMessage::Handshake` every message is identified by single decimal byte id
    fn reserve_bytes_len(&self) -> usize {
        4 + match self {
            PeerMessage::KeepAlive => 0,
            PeerMessage::Choke
            | PeerMessage::UnChoke
            | PeerMessage::Interested
            | PeerMessage::NotInterested => 1,
            PeerMessage::Have { .. } => 1 + 4,
            PeerMessage::Bitfield { index_field } => 1 + index_field.len(),
            PeerMessage::Request { peer_request } | PeerMessage::Cancel { peer_request } => 1 + 12,
            PeerMessage::Piece { piece } => 1 + 8 + piece.block.len(),
            PeerMessage::Handshake { handshake } => 45 + handshake.pstr.len(),
            PeerMessage::Port { .. } => 1 + 4,
        }
    }

    pub fn write_to_bytes(self) -> Result<Vec<u8>, ()> {
        let mut buf = Vec::with_capacity(self.reserve_bytes_len());

        Ok(buf)
    }
}

/// The handshake is a required message and must be the first message transmitted by the client.
/// `handshake: `<pstrlen><pstr><reserved><info_hash><peer_id>`
#[derive(Clone, PartialEq, Eq)]
pub struct Handshake {
    /// string identifier of the protocol
    pub pstr: String,
    /// eight (8) reserved bytes. All current implementations use all zeroes.
    /// Each bit in these bytes can be used to change the behavior of the protocol.
    /// An email from Bram suggests that trailing bits should be used first, so that leading bits may be used to change the meaning of trailing bits.
    pub reserved: [u8; 8],
    /// 20-byte SHA1 hash of the info key in the metainfo file.
    /// This is the same info_hash that is transmitted in tracker requests.
    pub info_hash: ShaHash,
    /// 20-byte string used as a unique ID for the client
    pub peer_id: ShaHash,
}
