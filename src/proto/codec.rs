use crate::bitfield::BitField;
use crate::error::Error;
use crate::piece::{Piece, BLOCK_SIZE_MAX};
use crate::proto::message::{Handshake, PeerMessage, PeerRequest};
use byteorder::{BigEndian, ByteOrder};
use bytes::{Buf, BufMut, BytesMut, IntoBuf};
use std::convert::TryInto;
use std::io::{self, Read};
use tokio_codec::{Decoder, Encoder};

pub struct PeerWireCodec {
    /// maximum permitted number of bytes per frame
    max: usize,
}

impl PeerWireCodec {
    pub fn new_with_max_length(max: usize) -> Self {
        Self { max }
    }
}

impl Default for PeerWireCodec {
    fn default() -> Self {
        Self {
            // length(4) + identifier(1) + payload (index(4) + offset(4) + max allowed blocksize)
            max: 4 + 1 + 8 + BLOCK_SIZE_MAX,
        }
    }
}

impl Decoder for PeerWireCodec {
    type Item = PeerMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }
        if src[0] == PeerMessage::HANDSHAKE_ID {
            // handshake
            if src.len() < 68 {
                return Ok(None);
            }
            if &src[1..20] != Handshake::BITTORRENT_IDENTIFIER {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported Handshake Protocol identifier",
                ));
            }
            let mut msg = src.split_to(68);
            msg.advance(20);
            Ok(Some(PeerMessage::Handshake {
                handshake: Handshake {
                    reserved: msg[0..8].try_into().unwrap(),
                    info_hash: msg[8..28].try_into().unwrap(),
                    peer_id: msg[28..48].try_into().unwrap(),
                },
            }))
        } else {
            // read length
            match BigEndian::read_u32(&src[0..4]) {
                0 => {
                    src.advance(4);
                    Ok(Some(PeerMessage::KeepAlive))
                }
                1 => {
                    if src.len() < 5 {
                        return Ok(None);
                    }

                    let msg = match src[4] {
                        PeerMessage::CHOKE_ID => Ok(Some(PeerMessage::Choke)),
                        PeerMessage::UNCHOKE_ID => Ok(Some(PeerMessage::UnChoke)),
                        PeerMessage::INTERESTED_ID => Ok(Some(PeerMessage::Interested)),
                        PeerMessage::NOT_INTERESTED_ID => Ok(Some(PeerMessage::NotInterested)),
                        i => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Unexpected Peer Message with length 1 and id {}", i),
                            ));
                        }
                    };
                    src.advance(5);
                    msg
                }
                5 => {
                    if src.len() < 9 {
                        return Ok(None);
                    }
                    if src[4] == PeerMessage::HAVE_ID {
                        let msg = src.split_to(9);
                        Ok(Some(PeerMessage::Have {
                            index: BigEndian::read_u32(&msg[5..9]),
                        }))
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Unexpected Peer Message with length 5 and id {}", src[4]),
                        ))
                    }
                }
                length => {
                    let msg_length = 4 + length as usize;
                    if src.len() < msg_length {
                        return Ok(None);
                    }

                    match src[4] {
                        PeerMessage::BITFIELD_ID => {
                            let msg = src.split_to(msg_length);
                            Ok(Some(PeerMessage::Bitfield {
                                index_field: BitField::from_bytes(&msg[5..]),
                            }))
                        }
                        PeerMessage::REQUEST_ID => {
                            let msg = src.split_to(msg_length);
                            Ok(Some(PeerMessage::Request {
                                request: PeerRequest {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    length: BigEndian::read_u32(&msg[13..17]),
                                },
                            }))
                        }
                        PeerMessage::PIECE_ID => {
                            let msg = src.split_to(msg_length);
                            Ok(Some(PeerMessage::Piece {
                                piece: Piece {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    block: msg[13..].to_vec(),
                                },
                            }))
                        }
                        PeerMessage::CANCEL_ID => {
                            let msg = src.split_to(msg_length);
                            Ok(Some(PeerMessage::Cancel {
                                request: PeerRequest {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    length: BigEndian::read_u32(&msg[13..17]),
                                },
                            }))
                        }
                        PeerMessage::PORT_ID => {
                            let msg = src.split_to(msg_length);
                            Ok(Some(PeerMessage::Port {
                                port: BigEndian::read_u16(&msg[5..]),
                            }))
                        }
                        i => Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Unexpected Peer Message with length 1 and id {}", i),
                        )),
                    }
                }
            }
        }
    }
}

impl Encoder for PeerWireCodec {
    type Item = PeerMessage;
    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let data = item.as_bytes()?;
        dst.reserve(data.len());
        dst.put(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::ShaHash;
    use libp2p_core::PeerId;

    macro_rules! peer_wire_msg_ende {
        ($( $msg:expr ),*) => {
            let mut codec = PeerWireCodec::default();
            $(
            {
                let mut buf = BytesMut::with_capacity($msg.len());
                codec.encode($msg.clone(), &mut buf).unwrap();
                assert_eq!(Some($msg), codec.decode(&mut buf).unwrap());
            }
            )*
        };
    }

    #[test]
    fn peer_wire_codec() {
        let handshake = PeerMessage::Handshake {
            handshake: Handshake::new_with_random_id(ShaHash::random()),
        };

        peer_wire_msg_ende!(
            PeerMessage::KeepAlive,
            PeerMessage::Choke,
            PeerMessage::UnChoke,
            PeerMessage::Interested,
            PeerMessage::NotInterested,
            PeerMessage::Have { index: 100 },
            PeerMessage::Bitfield {
                index_field: BitField::from_bytes(&[0b10100000, 0b00010010])
            },
            PeerMessage::Request {
                peer_request: PeerRequest {
                    index: 1,
                    begin: 2,
                    length: 16384
                }
            },
            PeerMessage::Piece {
                piece: Piece {
                    index: 1,
                    begin: 2,
                    block: std::iter::repeat(1).take(16384).collect()
                }
            },
            PeerMessage::Cancel {
                peer_request: PeerRequest {
                    index: 1,
                    begin: 2,
                    length: 16384
                }
            },
            PeerMessage::Port { port: 8080 },
            handshake
        );
    }
}
