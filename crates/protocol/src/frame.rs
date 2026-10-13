//! Reliable-stream framing: `u32_be length + protobuf Envelope` (report §7.4).

use bytes::{Buf, BufMut, BytesMut};
use prost::Message;
use thiserror::Error;

use crate::v1::Envelope;
use crate::{PROTOCOL_MAJOR, PROTOCOL_MINOR};

/// Control frames stay small; manifests page and file chunks use bulk streams.
pub const MAX_CONTROL_FRAME: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_CONTROL_FRAME} byte limit")]
    TooLarge(usize),
    #[error("malformed envelope: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("unsupported protocol major {0}")]
    UnsupportedMajor(u32),
    #[error("envelope has no payload")]
    MissingPayload,
}

/// Appends one length-prefixed frame to `out`.
pub fn encode_frame(envelope: &Envelope, out: &mut BytesMut) -> Result<(), FrameError> {
    let len = envelope.encoded_len();
    if len > MAX_CONTROL_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    out.reserve(4 + len);
    out.put_u32(len as u32);
    envelope.encode(out).expect("reserved capacity");
    Ok(())
}

/// Pops one complete frame from the front of `buf`, or returns `Ok(None)`
/// when more bytes are needed. The length is checked before buffering the
/// body, so a hostile peer cannot make us allocate an oversized frame.
pub fn decode_frame(buf: &mut BytesMut) -> Result<Option<Envelope>, FrameError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    if len > MAX_CONTROL_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    buf.advance(4);
    let body = buf.split_to(len);
    let envelope = Envelope::decode(body.freeze())?;
    if envelope.protocol_major != PROTOCOL_MAJOR {
        return Err(FrameError::UnsupportedMajor(envelope.protocol_major));
    }
    if envelope.payload.is_none() {
        return Err(FrameError::MissingPayload);
    }
    Ok(Some(envelope))
}

/// Envelope header with the current protocol version filled in.
pub fn envelope(request_id: u64, payload: crate::v1::envelope::Payload) -> Envelope {
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        request_id,
        payload: Some(payload),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1::{Ping, envelope::Payload};

    #[test]
    fn round_trips_across_partial_reads() {
        let mut wire = BytesMut::new();
        encode_frame(&envelope(1, Payload::Ping(Ping { nonce: 7 })), &mut wire).unwrap();
        encode_frame(&envelope(2, Payload::Ping(Ping { nonce: 8 })), &mut wire).unwrap();

        let mut rx = BytesMut::new();
        let mut got = Vec::new();
        for byte in wire {
            rx.put_u8(byte);
            while let Some(env) = decode_frame(&mut rx).unwrap() {
                got.push(env.request_id);
            }
        }
        assert_eq!(got, vec![1, 2]);
        assert!(rx.is_empty());
    }

    #[test]
    fn rejects_oversized_length_before_buffering_body() {
        let mut rx = BytesMut::new();
        rx.put_u32((MAX_CONTROL_FRAME + 1) as u32);
        assert!(matches!(
            decode_frame(&mut rx),
            Err(FrameError::TooLarge(_))
        ));
    }

    #[test]
    fn rejects_unknown_major() {
        let mut env = envelope(1, Payload::Ping(Ping { nonce: 0 }));
        env.protocol_major = PROTOCOL_MAJOR + 1;
        let mut wire = BytesMut::new();
        encode_frame(&env, &mut wire).unwrap();
        assert!(matches!(
            decode_frame(&mut wire),
            Err(FrameError::UnsupportedMajor(_))
        ));
    }

    #[test]
    fn rejects_empty_payload() {
        let mut env = envelope(1, Payload::Ping(Ping { nonce: 0 }));
        env.payload = None;
        let mut wire = BytesMut::new();
        encode_frame(&env, &mut wire).unwrap();
        assert!(matches!(
            decode_frame(&mut wire),
            Err(FrameError::MissingPayload)
        ));
    }
}
