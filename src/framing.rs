//! cTrader Open API wire framing: every message is a 4-byte big-endian length
//! followed by that many bytes of an encoded `ProtoMessage`.

/// Largest frame accepted from the server. Historical tick and trendbar
/// responses are the biggest messages and stay far below this; a larger
/// length means the stream is out of sync, and allocating it could exhaust
/// memory.
pub const MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

/// Prefixes an encoded message with its length.
pub fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + message.len());
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// Reads the length from a frame header, refusing implausible lengths.
pub fn frame_len(header: [u8; 4]) -> Result<usize, String> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(format!("frame of {len} bytes exceeds the {MAX_FRAME_LEN}-byte limit"));
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openapi::ProtoMessage;
    use prost::Message;

    #[test]
    fn round_trip() {
        let msg = ProtoMessage {
            payload_type: 51,
            payload: None,
            client_msg_id: Some("heartbeat".into()),
        };
        let framed = frame(&msg.encode_to_vec());
        let len = frame_len(framed[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, framed.len() - 4);
        assert_eq!(ProtoMessage::decode(&framed[4..]).unwrap(), msg);
    }

    #[test]
    fn length_is_big_endian() {
        assert_eq!(&frame(&[7; 258])[..4], &[0, 0, 1, 2]);
        assert_eq!(frame_len([0, 0, 1, 2]), Ok(258));
    }

    #[test]
    fn oversized_frames_are_refused() {
        assert!(frame_len((MAX_FRAME_LEN as u32).to_be_bytes()).is_ok());
        assert!(frame_len((MAX_FRAME_LEN as u32 + 1).to_be_bytes()).is_err());
        assert!(frame_len([0xff; 4]).is_err());
    }
}
