//! Length-prefixed binary framing.
//!
//! Wire format: a 4-byte big-endian unsigned length prefix, followed by
//! exactly that many payload bytes.
//!
//! ```text
//! +--------------------+-------------------------+
//! | length (4 bytes BE) | payload (length bytes)  |
//! +--------------------+-------------------------+
//! ```

use std::fmt;

/// Maximum allowed payload length: 16 MiB.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Declared length prefix exceeded `MAX_FRAME_LEN`.
    TooLarge(u32),
    /// The stream ended before a full frame could be read.
    UnexpectedEof,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooLarge(len) => {
                write!(f, "frame length {len} exceeds max {MAX_FRAME_LEN}")
            }
            FrameError::UnexpectedEof => write!(f, "stream ended before frame completed"),
        }
    }
}

impl std::error::Error for FrameError {}

/// A single length-prefixed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(payload: Vec<u8>) -> Self {
        Frame { payload }
    }

    /// Encode this frame as `[len:4][payload]`.
    pub fn encode(&self) -> Vec<u8> {
        let len = self.payload.len() as u32;
        let mut out = Vec::with_capacity(4 + self.payload.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// State machine driving incremental frame reassembly from arbitrary,
/// possibly-split, chunks of bytes.
#[derive(Debug)]
enum State {
    /// Waiting for the 4-byte length prefix; holds bytes seen so far.
    ReadingLen(Vec<u8>),
    /// Length known, waiting for `remaining` more payload bytes.
    ReadingPayload { buf: Vec<u8>, remaining: u32 },
}

/// Feed bytes in one at a time or in arbitrary chunks; `push` yields a
/// complete `Frame` whenever one finishes reassembling, regardless of how
/// the input bytes were split across calls.
#[derive(Debug)]
pub struct Decoder {
    state: State,
    max_len: u32,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Decoder {
            state: State::ReadingLen(Vec::with_capacity(4)),
            max_len: MAX_FRAME_LEN,
        }
    }

    pub fn with_max_len(max_len: u32) -> Self {
        Decoder {
            state: State::ReadingLen(Vec::with_capacity(4)),
            max_len,
        }
    }

    /// Feed a chunk of bytes into the decoder. Returns every frame that
    /// completed as a result of this chunk, in order. Consumed input is
    /// tracked internally; call repeatedly as more bytes arrive.
    pub fn push(&mut self, mut input: &[u8]) -> Result<Vec<Frame>, FrameError> {
        let mut done = Vec::new();
        while !input.is_empty() {
            match &mut self.state {
                State::ReadingLen(buf) => {
                    let need = 4 - buf.len();
                    let take = need.min(input.len());
                    buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if buf.len() == 4 {
                        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                        if len > self.max_len {
                            // Reset state so the decoder can be reused after the caller
                            // handles the error, without ever allocating `len` bytes.
                            self.state = State::ReadingLen(Vec::with_capacity(4));
                            return Err(FrameError::TooLarge(len));
                        }
                        if len == 0 {
                            done.push(Frame::new(Vec::new()));
                            self.state = State::ReadingLen(Vec::with_capacity(4));
                        } else {
                            self.state = State::ReadingPayload {
                                buf: Vec::with_capacity(len as usize),
                                remaining: len,
                            };
                        }
                    }
                }
                State::ReadingPayload { buf, remaining } => {
                    let take = (*remaining as usize).min(input.len());
                    buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    *remaining -= take as u32;
                    if *remaining == 0 {
                        let payload = std::mem::take(buf);
                        done.push(Frame::new(payload));
                        self.state = State::ReadingLen(Vec::with_capacity(4));
                    }
                }
            }
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty() {
        let frame = Frame::new(vec![]);
        let encoded = frame.encode();
        assert_eq!(encoded, vec![0, 0, 0, 0]);
        let mut dec = Decoder::new();
        let frames = dec.push(&encoded).unwrap();
        assert_eq!(frames, vec![frame]);
    }

    #[test]
    fn roundtrip_small() {
        let frame = Frame::new(b"hello".to_vec());
        let encoded = frame.encode();
        let mut dec = Decoder::new();
        let frames = dec.push(&encoded).unwrap();
        assert_eq!(frames, vec![frame]);
    }

    #[test]
    fn roundtrip_large() {
        let payload = vec![0xABu8; 500_000];
        let frame = Frame::new(payload);
        let encoded = frame.encode();
        let mut dec = Decoder::new();
        let frames = dec.push(&encoded).unwrap();
        assert_eq!(frames, vec![frame]);
    }

    #[test]
    fn streaming_reassembly_one_byte_at_a_time() {
        let frame = Frame::new(b"streamed payload".to_vec());
        let encoded = frame.encode();
        let mut dec = Decoder::new();
        let mut got = Vec::new();
        for b in &encoded {
            got.extend(dec.push(&[*b]).unwrap());
        }
        assert_eq!(got, vec![frame]);
    }

    #[test]
    fn multiple_frames_in_one_chunk() {
        let f1 = Frame::new(b"one".to_vec());
        let f2 = Frame::new(b"two".to_vec());
        let mut combined = f1.encode();
        combined.extend(f2.encode());
        let mut dec = Decoder::new();
        let frames = dec.push(&combined).unwrap();
        assert_eq!(frames, vec![f1, f2]);
    }

    #[test]
    fn oversize_length_prefix_rejected_without_allocating() {
        let mut dec = Decoder::with_max_len(1024);
        let huge_len: u32 = 0xFFFF_FFFF;
        let err = dec.push(&huge_len.to_be_bytes()).unwrap_err();
        assert_eq!(err, FrameError::TooLarge(huge_len));
    }

    #[test]
    fn decoder_recovers_after_oversize_rejection() {
        let mut dec = Decoder::with_max_len(1024);
        let huge_len: u32 = 5000;
        assert!(dec.push(&huge_len.to_be_bytes()).is_err());
        let frame = Frame::new(b"ok".to_vec());
        let frames = dec.push(&frame.encode()).unwrap();
        assert_eq!(frames, vec![frame]);
    }
}
