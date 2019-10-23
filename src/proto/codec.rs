use crate::pieces::BLOCK_SIZE_MAX;
use crate::proto::message::PeerMessage;
use bytes::{BigEndian, Buf, BytesMut, IntoBuf};
use std::io;
use tokio_codec::{BytesCodec, Decoder, LinesCodec};
use tokio_io::codec::Encoder;

pub struct BttBytesCodec {
    /// maximum permitted number of bytes per frame
    max: usize,
}

impl Default for BttBytesCodec {
    fn default() -> Self {
        Self {
            // length(4) + identifier(1) + payload (index(4) + offset(4) + max allowed blocksize)
            max: 4 + 1 + 8 + BLOCK_SIZE_MAX,
        }
    }
}

impl Decoder for BttBytesCodec {
    type Item = PeerMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        unimplemented!()
    }
}

impl Encoder for BttBytesCodec {
    type Item = PeerMessage;
    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        //        dst.extend_from_slice()

        unimplemented!()
    }
}
