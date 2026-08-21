//! A small typed protocol encoded on top of [`Frame`](crate::frame::Frame) payloads.
//!
//! Every message starts with a 1-byte tag identifying its kind, followed by
//! a kind-specific body. Strings are length-prefixed with a 2-byte
//! big-endian length so they stay well clear of the 16 MiB frame cap.

use crate::frame::Frame;
use std::fmt;

const TAG_PING: u8 = 0x01;
const TAG_PONG: u8 = 0x02;
const TAG_SET: u8 = 0x03;
const TAG_GET: u8 = 0x04;
const TAG_VALUE: u8 = 0x05;
const TAG_NOT_FOUND: u8 = 0x06;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Message {
    Ping,
    Pong,
    Set { key: String, value: Vec<u8> },
    Get { key: String },
    Value { value: Vec<u8> },
    NotFound,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Empty,
    UnknownTag(u8),
    Truncated,
    InvalidUtf8,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Empty => write!(f, "empty message body"),
            ProtocolError::UnknownTag(t) => write!(f, "unknown message tag {t:#04x}"),
            ProtocolError::Truncated => write!(f, "message body truncated"),
            ProtocolError::InvalidUtf8 => write!(f, "key bytes are not valid utf-8"),
        }
    }
}

impl std::error::Error for ProtocolError {}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn write_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    out.extend_from_slice(b);
}

fn read_string(buf: &[u8], pos: &mut usize) -> Result<String, ProtocolError> {
    if buf.len() < *pos + 2 {
        return Err(ProtocolError::Truncated);
    }
    let len = u16::from_be_bytes([buf[*pos], buf[*pos + 1]]) as usize;
    *pos += 2;
    if buf.len() < *pos + len {
        return Err(ProtocolError::Truncated);
    }
    let s = std::str::from_utf8(&buf[*pos..*pos + len])
        .map_err(|_| ProtocolError::InvalidUtf8)?
        .to_string();
    *pos += len;
    Ok(s)
}

fn read_bytes(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, ProtocolError> {
    if buf.len() < *pos + 4 {
        return Err(ProtocolError::Truncated);
    }
    let len = u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]) as usize;
    *pos += 4;
    if buf.len() < *pos + len {
        return Err(ProtocolError::Truncated);
    }
    let v = buf[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(v)
}

impl Message {
    pub fn encode(&self) -> Frame {
        let mut body = Vec::new();
        match self {
            Message::Ping => body.push(TAG_PING),
            Message::Pong => body.push(TAG_PONG),
            Message::Set { key, value } => {
                body.push(TAG_SET);
                write_string(&mut body, key);
                write_bytes(&mut body, value);
            }
            Message::Get { key } => {
                body.push(TAG_GET);
                write_string(&mut body, key);
            }
            Message::Value { value } => {
                body.push(TAG_VALUE);
                write_bytes(&mut body, value);
            }
            Message::NotFound => body.push(TAG_NOT_FOUND),
        }
        Frame::new(body)
    }

    pub fn decode(frame: &Frame) -> Result<Self, ProtocolError> {
        let buf = &frame.payload;
        if buf.is_empty() {
            return Err(ProtocolError::Empty);
        }
        let tag = buf[0];
        let mut pos = 1usize;
        match tag {
            TAG_PING => Ok(Message::Ping),
            TAG_PONG => Ok(Message::Pong),
            TAG_SET => {
                let key = read_string(buf, &mut pos)?;
                let value = read_bytes(buf, &mut pos)?;
                Ok(Message::Set { key, value })
            }
            TAG_GET => {
                let key = read_string(buf, &mut pos)?;
                Ok(Message::Get { key })
            }
            TAG_VALUE => {
                let value = read_bytes(buf, &mut pos)?;
                Ok(Message::Value { value })
            }
            TAG_NOT_FOUND => Ok(Message::NotFound),
            other => Err(ProtocolError::UnknownTag(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_pong_roundtrip() {
        assert_eq!(Message::decode(&Message::Ping.encode()).unwrap(), Message::Ping);
        assert_eq!(Message::decode(&Message::Pong.encode()).unwrap(), Message::Pong);
    }

    #[test]
    fn set_get_value_not_found_roundtrip() {
        let set = Message::Set {
            key: "k".to_string(),
            value: b"v".to_vec(),
        };
        assert_eq!(Message::decode(&set.encode()).unwrap(), set);

        let get = Message::Get { key: "k".to_string() };
        assert_eq!(Message::decode(&get.encode()).unwrap(), get);

        let value = Message::Value { value: b"v".to_vec() };
        assert_eq!(Message::decode(&value.encode()).unwrap(), value);

        assert_eq!(Message::decode(&Message::NotFound.encode()).unwrap(), Message::NotFound);
    }

    #[test]
    fn empty_body_errors() {
        let frame = Frame::new(vec![]);
        assert_eq!(Message::decode(&frame).unwrap_err(), ProtocolError::Empty);
    }

    #[test]
    fn unknown_tag_errors() {
        let frame = Frame::new(vec![0xEE]);
        assert_eq!(Message::decode(&frame).unwrap_err(), ProtocolError::UnknownTag(0xEE));
    }

    #[test]
    fn truncated_body_errors_cleanly() {
        // TAG_SET with a key length claiming 10 bytes but none present.
        let frame = Frame::new(vec![TAG_SET, 0x00, 0x0A]);
        assert_eq!(Message::decode(&frame).unwrap_err(), ProtocolError::Truncated);
    }

    #[test]
    fn invalid_utf8_key_errors_cleanly() {
        let mut body = vec![TAG_GET];
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0xFF, 0xFE]);
        let frame = Frame::new(body);
        assert_eq!(Message::decode(&frame).unwrap_err(), ProtocolError::InvalidUtf8);
    }
}
