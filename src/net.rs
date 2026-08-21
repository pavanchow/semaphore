//! TCP server and client speaking the Semaphore protocol.

use crate::frame::{Decoder, FrameError};
use crate::protocol::{Message, ProtocolError};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug)]
pub enum NetError {
    Io(io::Error),
    Frame(FrameError),
    Protocol(ProtocolError),
    /// Connection closed before a reply arrived.
    ConnectionClosed,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Io(e) => write!(f, "io error: {e}"),
            NetError::Frame(e) => write!(f, "frame error: {e}"),
            NetError::Protocol(e) => write!(f, "protocol error: {e}"),
            NetError::ConnectionClosed => write!(f, "connection closed before reply"),
        }
    }
}

impl std::error::Error for NetError {}
impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        NetError::Io(e)
    }
}
impl From<FrameError> for NetError {
    fn from(e: FrameError) -> Self {
        NetError::Frame(e)
    }
}
impl From<ProtocolError> for NetError {
    fn from(e: ProtocolError) -> Self {
        NetError::Protocol(e)
    }
}

/// Reads exactly one complete `Message` from a blocking stream, using a
/// small read buffer fed through a `Decoder` so partial TCP reads are
/// reassembled correctly.
fn read_message(stream: &mut TcpStream) -> Result<Message, NetError> {
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Err(NetError::ConnectionClosed);
        }
        let frames = decoder.push(&buf[..n])?;
        if let Some(frame) = frames.into_iter().next() {
            return Ok(Message::decode(&frame)?);
        }
    }
}

fn write_message(stream: &mut TcpStream, msg: &Message) -> Result<(), NetError> {
    let encoded = msg.encode().encode();
    stream.write_all(&encoded)?;
    Ok(())
}

/// In-memory key-value store shared across connections.
pub type Store = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// Run the Semaphore server on `addr`, blocking forever. Each connection is
/// served on its own thread against a shared in-memory map.
pub fn serve<A: ToSocketAddrs>(addr: A) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    for stream in listener.incoming() {
        let stream = stream?;
        let store = Arc::clone(&store);
        thread::spawn(move || {
            let _ = handle_connection(stream, store);
        });
    }
    Ok(())
}

/// Like [`serve`] but returns the bound listener's local address, useful for
/// binding to an ephemeral port (`:0`) in tests.
pub fn serve_on_ephemeral(addr: &str) -> io::Result<(std::net::SocketAddr, thread::JoinHandle<()>)> {
    let listener = TcpListener::bind(addr)?;
    let local_addr = listener.local_addr()?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let store = Arc::clone(&store);
                    thread::spawn(move || {
                        let _ = handle_connection(stream, store);
                    });
                }
                Err(_) => continue,
            }
        }
    });
    Ok((local_addr, handle))
}

fn handle_connection(mut stream: TcpStream, store: Store) -> Result<(), NetError> {
    loop {
        let msg = match read_message(&mut stream) {
            Ok(m) => m,
            Err(NetError::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e),
        };
        let reply = match msg {
            Message::Ping => Message::Pong,
            Message::Set { key, value } => {
                store.lock().unwrap().insert(key, value);
                Message::Pong
            }
            Message::Get { key } => match store.lock().unwrap().get(&key) {
                Some(value) => Message::Value { value: value.clone() },
                None => Message::NotFound,
            },
            other => other, // Pong/Value/NotFound sent by a client is echoed back verbatim.
        };
        write_message(&mut stream, &reply)?;
    }
}

/// A client connection to a Semaphore server.
pub struct Client {
    stream: TcpStream,
}

impl Client {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        Ok(Client { stream })
    }

    pub fn ping(&mut self) -> Result<(), NetError> {
        write_message(&mut self.stream, &Message::Ping)?;
        match read_message(&mut self.stream)? {
            Message::Pong => Ok(()),
            _ => Err(NetError::ConnectionClosed),
        }
    }

    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), NetError> {
        write_message(
            &mut self.stream,
            &Message::Set {
                key: key.to_string(),
                value: value.to_vec(),
            },
        )?;
        match read_message(&mut self.stream)? {
            Message::Pong => Ok(()),
            _ => Err(NetError::ConnectionClosed),
        }
    }

    pub fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, NetError> {
        write_message(&mut self.stream, &Message::Get { key: key.to_string() })?;
        match read_message(&mut self.stream)? {
            Message::Value { value } => Ok(Some(value)),
            Message::NotFound => Ok(None),
            _ => Err(NetError::ConnectionClosed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn server_client_set_get_ping() {
        let (addr, _handle) = serve_on_ephemeral("127.0.0.1:0").unwrap();
        thread::sleep(Duration::from_millis(50));

        let mut client = Client::connect(addr).unwrap();
        client.ping().unwrap();

        client.set("hello", b"world").unwrap();
        let got = client.get("hello").unwrap();
        assert_eq!(got, Some(b"world".to_vec()));

        let missing = client.get("nope").unwrap();
        assert_eq!(missing, None);
    }
}
