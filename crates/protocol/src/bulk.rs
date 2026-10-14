//! Bulk-connection framing: `u32_be length + protobuf BulkFrame`.
//!
//! Separate from control framing so a file chunk can be larger than any
//! control frame, and so a bulk connection can never carry input.

use std::io::{self, Read, Write};

use bytes::{BufMut, BytesMut};
use prost::Message;

use crate::v1::BulkFrame;
use crate::v1::bulk_frame::Body;

/// Data per `FileChunk` (report §10.1 starting value, D).
pub const CHUNK_SIZE: usize = 256 * 1024;
/// A chunk plus protobuf overhead, or one manifest page.
pub const MAX_BULK_FRAME: usize = 1024 * 1024;

/// Writes one bulk frame. `scratch` is reused between calls so a transfer
/// does not allocate per chunk; taking the frame by reference lets the caller
/// reuse a chunk's data buffer too.
pub fn write_bulk(w: &mut impl Write, frame: &BulkFrame, scratch: &mut BytesMut) -> io::Result<()> {
    let len = frame.encoded_len();
    if len > MAX_BULK_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bulk frame of {len} bytes exceeds {MAX_BULK_FRAME}"),
        ));
    }
    scratch.clear();
    scratch.reserve(4 + len);
    scratch.put_u32(len as u32);
    frame.encode(scratch).expect("reserved capacity");
    w.write_all(scratch)
}

/// Reads one bulk frame. A length over the limit or a frame without a body
/// is `InvalidData`; a clean close before the length is `UnexpectedEof`.
pub fn read_bulk(r: &mut impl Read, scratch: &mut Vec<u8>) -> io::Result<Body> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_BULK_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bulk frame of {len} bytes exceeds {MAX_BULK_FRAME}"),
        ));
    }
    scratch.resize(len, 0);
    r.read_exact(scratch)?;
    BulkFrame::decode(&scratch[..])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        .body
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bulk frame has no body"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::{FileChunk, TransferDone};

    #[test]
    fn round_trips_a_full_chunk() {
        let mut wire = Vec::new();
        let mut scratch = BytesMut::new();
        let data = vec![7u8; CHUNK_SIZE];
        let chunk = BulkFrame {
            body: Some(Body::Chunk(FileChunk { data: data.clone() })),
        };
        let done = BulkFrame {
            body: Some(Body::Done(TransferDone {})),
        };
        write_bulk(&mut wire, &chunk, &mut scratch).unwrap();
        write_bulk(&mut wire, &done, &mut scratch).unwrap();

        let mut r = &wire[..];
        let mut buf = Vec::new();
        assert!(matches!(read_bulk(&mut r, &mut buf).unwrap(), Body::Chunk(c) if c.data == data));
        assert!(matches!(
            read_bulk(&mut r, &mut buf).unwrap(),
            Body::Done(_)
        ));
        assert_eq!(
            read_bulk(&mut r, &mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rejects_oversized_length_before_reading_body() {
        let wire = ((MAX_BULK_FRAME + 1) as u32).to_be_bytes();
        let err = read_bulk(&mut &wire[..], &mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_frame_without_body() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&0u32.to_be_bytes());
        let err = read_bulk(&mut &wire[..], &mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
