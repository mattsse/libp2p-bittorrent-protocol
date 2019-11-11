use std::convert::TryInto;
use std::io::{self, Read, Write};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::util::ShaHash;

use crate::bitfield::BitField;
use crate::error::Error;
use crate::piece::Piece;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRequest {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// specifying the requested length.
    pub length: u32,
}

/// All of the remaining messages in the protocol take the form of <length
/// prefix><message ID><payload>. The length prefix is a four byte big-endian
/// value. The message ID is a single decimal byte. integers in the peer wire
/// protocol are encoded as four byte big-endian values
#[derive(Debug, Clone, PartialEq, Eq)]
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
        /// bitfield representing the pieces that have been successfully
        /// downloaded
        index_field: BitField,
    },
    Request {
        request: PeerRequest,
    },
    Cancel {
        request: PeerRequest,
    },
    Piece {
        piece: Piece,
    },
    Handshake {
        handshake: Handshake,
    },
    /// he port message is sent by newer versions of the Mainline that
    /// implements a DHT tracker. The listen port is the port this peer's
    /// DHT node is listening on. This peer should be inserted in the local
    /// routing table (if DHT tracker is supported).
    Port {
        port: u16,
    },
}

impl PeerMessage {
    pub const CHOKE_ID: u8 = 0;
    pub const UNCHOKE_ID: u8 = 1;
    pub const INTERESTED_ID: u8 = 2;
    pub const NOT_INTERESTED_ID: u8 = 3;
    pub const HAVE_ID: u8 = 4;
    pub const BITFIELD_ID: u8 = 5;
    pub const REQUEST_ID: u8 = 6;
    pub const PIECE_ID: u8 = 7;
    pub const CANCEL_ID: u8 = 8;
    pub const PORT_ID: u8 = 9;

    pub const HANDSHAKE_ID: u8 = 19;

    pub const KEEP_ALIVE_ID: [u8; 4] = [0, 0, 0, 0];

    /// length in bytes that should be reserved for serialising the message.
    ///
    /// Besides `PeerMessage::Handshake` all messages are prefixed by its length
    /// (4 byte big endian). Besides `PeerMessage::KeepAlive` and
    /// `PeerMessage::Handshake` every message is identified by single decimal
    /// byte id
    pub fn len(&self) -> usize {
        4 + match self {
            PeerMessage::KeepAlive => 0,
            PeerMessage::Choke
            | PeerMessage::UnChoke
            | PeerMessage::Interested
            | PeerMessage::NotInterested => 1,
            PeerMessage::Have { .. } => 1 + 4,
            PeerMessage::Bitfield { index_field } => {
                let mut len = index_field.len() / 8;
                let rem = index_field.len() % 8;
                if rem > 0 {
                    len += 1;
                }
                1 + len
            }
            PeerMessage::Request {
                request: peer_request,
            }
            | PeerMessage::Cancel {
                request: peer_request,
            } => 1 + 12,
            PeerMessage::Piece { piece } => 1 + 8 + piece.block.len(),
            PeerMessage::Handshake { handshake } => 45 + 19,
            PeerMessage::Port { .. } => 1 + 4,
        }
    }

    pub fn as_bytes(&self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.len());
        match self {
            PeerMessage::KeepAlive => buf.write_u32::<BigEndian>(0)?,
            PeerMessage::Choke => {
                buf.write_u32::<BigEndian>(1)?;
                buf.write_u8(0)?;
            }
            PeerMessage::UnChoke => {
                buf.write_u32::<BigEndian>(1)?;
                buf.write_u8(1)?;
            }
            PeerMessage::Interested => {
                buf.write_u32::<BigEndian>(1)?;
                buf.write_u8(2)?;
            }
            PeerMessage::NotInterested => {
                buf.write_u32::<BigEndian>(1)?;
                buf.write_u8(3)?;
            }
            PeerMessage::Have { index } => {
                buf.write_u32::<BigEndian>(5)?;
                buf.write_u8(4)?;
                buf.write_u32::<BigEndian>(*index)?;
            }
            PeerMessage::Bitfield { index_field } => {
                buf.write_u32::<BigEndian>(1 + index_field.len() as u32)?;
                buf.write_u8(5)?;
                // sparse bits are zero
                buf.write_all(&index_field.to_bytes())?;
            }
            PeerMessage::Request {
                request: peer_request,
            } => {
                buf.write_u32::<BigEndian>(13)?;
                buf.write_u8(6)?;
                buf.write_u32::<BigEndian>(peer_request.index)?;
                buf.write_u32::<BigEndian>(peer_request.begin)?;
                buf.write_u32::<BigEndian>(peer_request.length)?;
            }
            PeerMessage::Piece { piece } => {
                buf.write_u32::<BigEndian>(9 + piece.block.len() as u32)?;
                buf.write_u8(7)?;
                buf.write_u32::<BigEndian>(piece.index)?;
                buf.write_u32::<BigEndian>(piece.begin)?;
                buf.write_all(&piece.block)?;
            }
            PeerMessage::Cancel {
                request: peer_request,
            } => {
                buf.write_u32::<BigEndian>(13)?;
                buf.write_u8(8)?;
                buf.write_u32::<BigEndian>(peer_request.index)?;
                buf.write_u32::<BigEndian>(peer_request.begin)?;
                buf.write_u32::<BigEndian>(peer_request.length)?;
            }
            PeerMessage::Port { port } => {
                buf.write_u32::<BigEndian>(3)?;
                buf.write_u8(9)?;
                buf.write_u16::<BigEndian>(*port)?;
            }
            PeerMessage::Handshake { handshake } => {
                buf.write(b"\x13")?;
                buf.write_all(&Handshake::BITTORRENT_IDENTIFIER)?;
                buf.write_all(&handshake.reserved)?;
                buf.write_all(handshake.info_hash.as_ref())?;
                buf.write_all(handshake.peer_id.as_ref())?;
            }
        }

        Ok(buf)
    }
}

/// The handshake is a required message and must be the first message
/// transmitted by the client. `handshake:
/// `<pstrlen><pstr><reserved><info_hash><peer_id>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// eight (8) reserved bytes. All current implementations use all zeroes.
    /// Each bit in these bytes can be used to change the behavior of the
    /// protocol. An email from Bram suggests that trailing bits should be
    /// used first, so that leading bits may be used to change the meaning of
    /// trailing bits.
    pub reserved: [u8; 8],
    /// 20-byte SHA1 hash of the info key in the metainfo file.
    /// This is the same info_hash that is transmitted in tracker requests.
    pub info_hash: ShaHash,
    /// 20-byte string used as a unique ID for the client
    pub peer_id: ShaHash,
}

impl Handshake {
    pub fn new(info_hash: ShaHash, peer_id: ShaHash) -> Self {
        Self {
            reserved: [0, 0, 0, 0, 0, 0, 0, 0],
            info_hash,
            peer_id,
        }
    }
}

impl Handshake {
    pub const BITTORRENT_IDENTIFIER_STR: &'static str = "BitTorrent protocol";

    pub const BITTORRENT_IDENTIFIER: [u8; 19] = [
        66, 105, 116, 84, 111, 114, 114, 101, 110, 116, 32, 112, 114, 111, 116, 111, 99, 111, 108,
    ];

    pub fn new_with_random_id<T: Into<ShaHash>>(info_hash: T) -> Self {
        Self {
            reserved: [0u8; 8],
            info_hash: info_hash.into(),
            peer_id: ShaHash::random(),
        }
    }
}
