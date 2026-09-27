//! Turning a byte stream into messages.
//!
//! A four-byte big-endian length, then that many bytes of JSON. QUIC gives ordered, reliable
//! bytes within a stream but no message boundaries, so something has to draw them.
//!
//! **JSON, deliberately.** The bulk of what this fleet moves — bundles, transcripts, patches
//! — travels as raw blob bytes on streams of its own and never passes through here. What does
//! pass through is control traffic: handshakes, gossip deltas, assignments. At that volume
//! legibility is worth more than density, and the ability to read a captured frame in a
//! terminal is worth a great deal when two nodes disagree. [`crate::Version`] is what makes
//! this reversible: a compact codec is a version bump, not a rewrite.
//!
//! The parser is incremental and owns no I/O. The transport reads bytes however it likes and
//! feeds them in; frames come out when they are complete. That keeps this crate testable
//! against a byte-at-a-time drip feed, which is the case a hand-rolled framer gets wrong.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The largest single message this protocol will read.
///
/// Control messages are kilobytes; a megabyte is generous. The limit exists because a length
/// prefix is attacker-controlled input: without a cap, four bytes on the wire become a 4 GiB
/// allocation, and the cheapest denial of service in any framed protocol is a large number.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

const LENGTH_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {size} bytes exceeds the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },
    #[error("message could not be encoded: {reason}")]
    Encode { reason: String },
    #[error("message could not be decoded: {reason}")]
    Decode { reason: String },
}

/// Encode a message as one frame, length prefix included.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(message).map_err(|e| FrameError::Encode {
        reason: e.to_string(),
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            size: body.len(),
            limit: MAX_FRAME_BYTES,
        });
    }
    // The cast is safe because of the check above, and the check is the reason the cast is
    // written rather than the length being trusted.
    let length = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge {
        size: body.len(),
        limit: MAX_FRAME_BYTES,
    })?;

    let mut out = Vec::with_capacity(LENGTH_BYTES + body.len());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode one frame body — the bytes [`FrameReader::next_frame`] hands back.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    serde_json::from_slice(body).map_err(|e| FrameError::Decode {
        reason: e.to_string(),
    })
}

/// Accumulates bytes and yields whole frames.
///
/// Holds at most one partial frame plus whatever arrived with it, so a peer cannot make this
/// grow without bound by sending a large length and then nothing: the length is checked
/// against the limit the moment it is readable, before a single body byte is buffered.
#[derive(Debug)]
pub struct FrameReader {
    buffer: Vec<u8>,
    limit: usize,
}

impl Default for FrameReader {
    fn default() -> Self {
        FrameReader::new()
    }
}

impl FrameReader {
    #[must_use]
    pub fn new() -> FrameReader {
        FrameReader::with_limit(MAX_FRAME_BYTES)
    }

    #[must_use]
    pub fn with_limit(limit: usize) -> FrameReader {
        FrameReader {
            buffer: Vec::new(),
            limit,
        }
    }

    /// Feed in whatever was read from the wire, of any size, including none.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next complete frame's body, or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buffer.len() < LENGTH_BYTES {
            return Ok(None);
        }
        let mut length = [0u8; LENGTH_BYTES];
        length.copy_from_slice(&self.buffer[..LENGTH_BYTES]);
        let size = u32::from_be_bytes(length) as usize;

        if size > self.limit {
            return Err(FrameError::TooLarge {
                size,
                limit: self.limit,
            });
        }
        if self.buffer.len() < LENGTH_BYTES + size {
            return Ok(None);
        }

        let body = self.buffer[LENGTH_BYTES..LENGTH_BYTES + size].to_vec();
        self.buffer.drain(..LENGTH_BYTES + size);
        Ok(Some(body))
    }

    /// Bytes held but not yet a complete frame. For diagnostics; a stream that ends here
    /// ended mid-message.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }

    /// Take the buffered bytes and stop framing.
    ///
    /// For the one case where a stream stops carrying messages and starts carrying a payload:
    /// a blob follows its header, usually in the same read (ADR-0016). Without this those
    /// bytes would sit in the framer waiting for a length that is never coming.
    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Msg {
        who: String,
        n: u32,
    }

    fn msg(who: &str, n: u32) -> Msg {
        Msg {
            who: who.to_string(),
            n,
        }
    }

    #[test]
    fn a_message_survives_the_round_trip() {
        let original = msg("laptop", 7);
        let frame = encode(&original).expect("encode");
        let mut reader = FrameReader::new();
        reader.feed(&frame);
        let body = reader.next_frame().expect("read").expect("complete");
        assert_eq!(decode::<Msg>(&body).expect("decode"), original);
        assert_eq!(reader.pending(), 0);
    }

    #[test]
    fn frames_arriving_one_byte_at_a_time_still_parse() {
        // The case a hand-rolled framer gets wrong, and the reason this is an incremental
        // parser rather than a function over a whole buffer.
        let frame = encode(&msg("phone", 1)).expect("encode");
        let mut reader = FrameReader::new();
        for byte in &frame[..frame.len() - 1] {
            reader.feed(&[*byte]);
            assert_eq!(reader.next_frame().expect("read"), None, "not yet");
        }
        reader.feed(&frame[frame.len() - 1..]);
        assert!(reader.next_frame().expect("read").is_some());
    }

    #[test]
    fn several_frames_in_one_read_all_come_back_in_order() {
        // QUIC hands over whatever has arrived, which is frequently more than one message.
        let mut wire = Vec::new();
        for n in 0..3 {
            wire.extend(encode(&msg("desktop", n)).expect("encode"));
        }
        let mut reader = FrameReader::new();
        reader.feed(&wire);

        for n in 0..3 {
            let body = reader.next_frame().expect("read").expect("complete");
            assert_eq!(decode::<Msg>(&body).expect("decode"), msg("desktop", n));
        }
        assert_eq!(reader.next_frame().expect("read"), None);
    }

    #[test]
    fn an_oversized_length_is_refused_before_anything_is_buffered() {
        // Four bytes of attacker input must not become a four-gigabyte allocation.
        let mut reader = FrameReader::with_limit(64);
        reader.feed(&u32::MAX.to_be_bytes());
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::TooLarge { limit: 64, .. })
        ));
    }

    #[test]
    fn a_reader_refuses_a_frame_larger_than_it_will_hold() {
        // Sender and receiver enforce the same limit from opposite sides; a message that one
        // will send and the other will not read is a stall with no error in it anywhere.
        let big = msg(&"x".repeat(200), 0);
        let mut reader = FrameReader::with_limit(64);
        reader.feed(&encode(&big).expect("encode"));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_body_that_is_not_the_expected_message_is_a_decode_error_not_a_panic() {
        let mut reader = FrameReader::new();
        reader.feed(&encode(&"just a string").expect("encode"));
        let body = reader.next_frame().expect("read").expect("complete");
        assert!(matches!(
            decode::<Msg>(&body),
            Err(FrameError::Decode { .. })
        ));
    }
}
