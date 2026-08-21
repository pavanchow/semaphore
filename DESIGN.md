# Design

## Frame format

Every message on the wire is:

```
[ length : u32 big-endian ][ payload : length bytes ]
```

The length prefix describes only the payload, not itself. A `Frame` is
just a `Vec<u8>` payload. `Frame::encode` writes the 4-byte prefix then the
payload bytes. There is no magic number, version byte, or checksum: the
protocol is deliberately minimal, and correctness comes from the decoder
being strict about the length field.

A hard cap, `MAX_FRAME_LEN` (16 MiB), bounds the payload size. Any declared
length above the cap is rejected immediately, before any buffer for the
payload is allocated. This is the difference between a malicious or
corrupt length prefix costing a `u32` comparison versus costing an
attacker-chosen amount of memory.

## The streaming decoder state machine

TCP is a byte stream, not a message stream: a single `read()` can return
less than a full frame, more than one frame, or split a frame's length
prefix across two reads. The `Decoder` in `src/frame.rs` handles this with
an explicit two-state machine:

```
ReadingLen(buf: Vec<u8>)              -- accumulating the 4 length bytes
        |  buf.len() == 4
        v
ReadingPayload { buf, remaining }     -- accumulating `remaining` payload bytes
        |  remaining == 0
        v
  yield Frame, back to ReadingLen
```

`Decoder::push` takes an arbitrary byte slice (as small as one byte) and
feeds it into whichever state is active, looping until the input is
consumed. It returns every frame that completed as a result of that call,
so a single chunk containing several back-to-back frames yields all of
them in order. Because state lives in the `Decoder` between calls, feeding
bytes one at a time produces exactly the same frames as feeding them all
at once.

When a length prefix exceeds the cap, the decoder resets itself back to
`ReadingLen` before returning the error, so a caller can log the error and
keep using the same decoder for the next, hopefully well-formed, frame.

## Message types

`src/protocol.rs` layers a small typed protocol on top of frame payloads.
Each message is a 1-byte tag plus a tag-specific body:

- `PING` / `PONG`: no body.
- `SET { key, value }`: a `u16`-length-prefixed UTF-8 key, then a
  `u32`-length-prefixed byte value.
- `GET { key }`: a `u16`-length-prefixed UTF-8 key.
- `VALUE { value }`: a `u32`-length-prefixed byte value.
- `NOT_FOUND`: no body.

Decoding walks the body with a `pos` cursor and bounds-checks every read
before slicing, so truncated or malformed bodies return a `ProtocolError`
instead of panicking. An unrecognized tag byte is also a typed error
rather than a silent fallback.

## Server and client model

The server (`src/net.rs`) is a plain blocking TCP listener: `accept()` in
a loop, spawn an OS thread per connection, and share one in-memory
`HashMap<String, Vec<u8>>` behind a `Mutex` across all connections. Each
connection thread loops: read one message (via the streaming decoder over
a small fixed read buffer), react to it, write one reply, repeat until the
peer closes the socket.

The client wraps a single `TcpStream` and exposes `ping`, `set`, and `get`
as blocking request/reply calls: write one encoded message, block on
reading the next full message back.

There is no connection pooling, pipelining, or async runtime. The model
maps directly onto blocking `std::net` calls and OS threads, which keeps
the whole request lifecycle traceable in a debugger without needing to
reason about a task scheduler.

## Error handling

Every layer defines its own error enum (`FrameError`, `ProtocolError`,
`NetError`) rather than using strings or panicking. A truncated stream, an
oversize length prefix, and a malformed message body are all represented
as typed values the caller can match on, propagated up through `?` and
`From` conversions into `NetError` at the network layer.
