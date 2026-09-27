//! One bidirectional stream, and the frames that go over it.
//!
//! The two halves are separate because QUIC's are: `quinn` hands back a send stream and a
//! receive stream, and pretending otherwise would mean gluing them together here and taking
//! them apart again there. The memory implementation splits a duplex pipe to match.
//!
//! Framing goes through `offload_proto`'s incremental parser rather than a `read_exact` of a
//! length — one implementation of the size limit, and it is the one with the tests.

use crate::TransportError;
use offload_proto::{FrameReader, MAX_FRAME_BYTES};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// How much to ask the OS for at a time. Control messages are small; this is a buffer size,
/// not a limit — [`MAX_FRAME_BYTES`] is the limit.
const READ_CHUNK: usize = 8 * 1024;

pub struct Stream {
    send: Box<dyn AsyncWrite + Send + Unpin>,
    recv: Box<dyn AsyncRead + Send + Unpin>,
    reader: FrameReader,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("buffered", &self.reader.pending())
            .finish()
    }
}

impl Stream {
    #[must_use]
    pub fn new(
        send: Box<dyn AsyncWrite + Send + Unpin>,
        recv: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Stream {
        Stream {
            send,
            recv,
            reader: FrameReader::new(),
        }
    }

    /// Write one message.
    pub async fn send<T: Serialize + Sync>(&mut self, message: &T) -> Result<(), TransportError> {
        let frame = offload_proto::frame::encode(message)?;
        self.send
            .write_all(&frame)
            .await
            .map_err(|e| TransportError::io("writing a frame", &e))?;
        self.send
            .flush()
            .await
            .map_err(|e| TransportError::io("flushing a frame", &e))
    }

    /// Read one message, waiting for as many reads as it takes.
    ///
    /// A clean end of stream before a frame completes is an error rather than `None`: every
    /// caller here is mid-exchange, and "the peer went away halfway through" is exactly the
    /// thing that must not be mistaken for "no more messages".
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T, TransportError> {
        loop {
            if let Some(body) = self.reader.next_frame()? {
                return Ok(offload_proto::frame::decode(&body)?);
            }
            let mut chunk = [0u8; READ_CHUNK];
            let read = self
                .recv
                .read(&mut chunk)
                .await
                .map_err(|e| TransportError::io("reading a frame", &e))?;
            if read == 0 {
                return Err(TransportError::Closed {
                    peer: "peer".into(),
                    reason: if self.reader.pending() == 0 {
                        "stream ended before the message".into()
                    } else {
                        format!(
                            "stream ended {} bytes into a message",
                            self.reader.pending()
                        )
                    },
                });
            }
            self.reader.feed(&chunk[..read]);
        }
    }

    /// Write raw bytes, outside the framing.
    ///
    /// For blob payloads only (ADR-0016): they follow a framed header that said how many are
    /// coming, and they get a stream to themselves so that a large one cannot delay anything
    /// else. Anything with structure should be a message.
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.send
            .write_all(bytes)
            .await
            .map_err(|e| TransportError::io("writing bytes", &e))?;
        self.send
            .flush()
            .await
            .map_err(|e| TransportError::io("flushing bytes", &e))
    }

    /// Read exactly `size` raw bytes.
    ///
    /// The caller states the size because the sender already said it in a header this crate
    /// has no opinion about — and because reading to the end of a stream would let a peer
    /// decide how much memory we spend. A short stream is an error rather than a truncated
    /// blob: content addressing would catch it, but "the peer went away" is a better message
    /// than "the hash did not match".
    pub async fn recv_bytes(&mut self, size: usize) -> Result<Vec<u8>, TransportError> {
        // Whatever the framer has already buffered belongs to this payload: the header and
        // the first bytes of the blob almost always arrive in the same read.
        let mut out = self.reader.take_pending();
        out.truncate(size);
        out.reserve(size.saturating_sub(out.len()));

        while out.len() < size {
            let mut chunk = vec![0u8; READ_CHUNK.min(size - out.len())];
            let read = self
                .recv
                .read(&mut chunk)
                .await
                .map_err(|e| TransportError::io("reading bytes", &e))?;
            if read == 0 {
                return Err(TransportError::Closed {
                    peer: "peer".into(),
                    reason: format!("stream ended after {} of {size} bytes", out.len()),
                });
            }
            out.extend_from_slice(&chunk[..read]);
        }
        Ok(out)
    }

    /// Signal that nothing more will be sent, so the peer's next read ends cleanly rather
    /// than waiting for a message that is not coming.
    pub async fn finish(&mut self) -> Result<(), TransportError> {
        self.send
            .shutdown()
            .await
            .map_err(|e| TransportError::io("finishing a stream", &e))
    }
}

/// The largest message this transport will read, re-exported so callers do not have to reach
/// past it into the protocol crate.
pub const MAX_MESSAGE_BYTES: usize = MAX_FRAME_BYTES;
