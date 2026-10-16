// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire format of Calibre's smart device ("wireless device") protocol.
//!
//! Every control message is `<decimal length>[opcode, {payload}]`, where the
//! length counts the bytes of the JSON array. Book contents travel as raw
//! bytes on the same stream directly after the message that announces them.
//! See `docs/research/calibre-wireless-protocol.md`.

use std::fmt;
use std::io::{self, BufRead, Read, Write};

use serde_json::{json, Value};

/// Largest control message we accept. Book metadata (including a base64
/// cover thumbnail) is the biggest legitimate payload, well under this.
pub const MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

/// Longest length prefix we accept before the opening `[`.
const MAX_PREFIX_DIGITS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Ok,
    SetCalibreDeviceInfo,
    SetCalibreDeviceName,
    GetDeviceInformation,
    TotalSpace,
    FreeSpace,
    GetBookCount,
    SendBooklists,
    SendBook,
    GetInitializationInfo,
    BookDone,
    Noop,
    DeleteBook,
    GetBookFileSegment,
    GetBookMetadata,
    SendBookMetadata,
    DisplayMessage,
    CalibreBusy,
    SetLibraryInfo,
    Error,
    GetCollections,
    UpdateCollections,
}

impl Opcode {
    const ALL: [(Opcode, u64); 22] = [
        (Opcode::Ok, 0),
        (Opcode::SetCalibreDeviceInfo, 1),
        (Opcode::SetCalibreDeviceName, 2),
        (Opcode::GetDeviceInformation, 3),
        (Opcode::TotalSpace, 4),
        (Opcode::FreeSpace, 5),
        (Opcode::GetBookCount, 6),
        (Opcode::SendBooklists, 7),
        (Opcode::SendBook, 8),
        (Opcode::GetInitializationInfo, 9),
        (Opcode::BookDone, 11),
        (Opcode::Noop, 12),
        (Opcode::DeleteBook, 13),
        (Opcode::GetBookFileSegment, 14),
        (Opcode::GetBookMetadata, 15),
        (Opcode::SendBookMetadata, 16),
        (Opcode::DisplayMessage, 17),
        (Opcode::CalibreBusy, 18),
        (Opcode::SetLibraryInfo, 19),
        (Opcode::Error, 20),
        (Opcode::GetCollections, 21),
        (Opcode::UpdateCollections, 22),
    ];

    pub fn from_code(code: u64) -> Option<Opcode> {
        Self::ALL
            .iter()
            .find(|(_, c)| *c == code)
            .map(|(op, _)| *op)
    }

    pub fn code(self) -> u64 {
        Self::ALL
            .iter()
            .find(|(op, _)| *op == self)
            .map(|(_, c)| *c)
            .unwrap()
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// One decoded control message. `code` is kept raw so that unknown opcodes
/// from newer Calibre versions can be logged instead of failing the session.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub code: u64,
    pub payload: Value,
}

impl Message {
    pub fn opcode(&self) -> Option<Opcode> {
        Opcode::from_code(self.code)
    }

    fn name(&self) -> String {
        match self.opcode() {
            Some(op) => op.to_string(),
            None => format!("UNKNOWN({})", self.code),
        }
    }
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Reads one message. Returns `Ok(None)` on a clean end of stream between
/// messages.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Message>> {
    let mut digits = Vec::with_capacity(MAX_PREFIX_DIGITS);
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            if digits.is_empty() {
                return Ok(None);
            }
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let b = buf[0];
        if b == b'[' {
            break;
        }
        if !b.is_ascii_digit() || digits.len() == MAX_PREFIX_DIGITS {
            return Err(invalid(format!("bad length prefix byte {b:#04x}")));
        }
        digits.push(b);
        reader.consume(1);
    }
    if digits.is_empty() {
        return Err(invalid("missing length prefix"));
    }
    let len: usize = std::str::from_utf8(&digits)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("unparsable length prefix"))?;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(invalid(format!("frame length {len} out of range")));
    }

    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let value: Value =
        serde_json::from_slice(&body).map_err(|e| invalid(format!("bad JSON frame: {e}")))?;
    let Value::Array(mut parts) = value else {
        return Err(invalid("frame is not a JSON array"));
    };
    if parts.len() != 2 {
        return Err(invalid(format!(
            "frame has {} elements, expected 2",
            parts.len()
        )));
    }
    let payload = parts.pop().unwrap();
    let code = parts[0]
        .as_u64()
        // Tolerate integral floats, which some JSON encoders produce.
        .or_else(|| {
            parts[0]
                .as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64)
        })
        .ok_or_else(|| invalid("opcode is not a non-negative integer"))?;
    Ok(Some(Message { code, payload }))
}

/// Writes one message and flushes.
pub fn write_message<W: Write>(writer: &mut W, opcode: Opcode, payload: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(&json!([opcode.code(), payload]))
        .map_err(|e| invalid(format!("cannot encode frame: {e}")))?;
    writer.write_all(body.len().to_string().as_bytes())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// A protocol connection: buffered reader, writer and an optional transcript.
///
/// The transcript is JSON lines of every control message in both directions,
/// with long strings (cover thumbnails) elided and raw book bytes recorded
/// only by length. It is used to build test fixtures from real sessions.
pub struct Connection<R, W> {
    reader: R,
    writer: W,
    transcript: Option<Box<dyn Write + Send>>,
}

impl<R: BufRead, W: Write> Connection<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Connection {
            reader,
            writer,
            transcript: None,
        }
    }

    pub fn with_transcript(mut self, transcript: Box<dyn Write + Send>) -> Self {
        self.transcript = Some(transcript);
        self
    }

    pub fn recv(&mut self) -> io::Result<Option<Message>> {
        let msg = read_message(&mut self.reader)?;
        if let Some(m) = &msg {
            log::debug!("<- {} {}", m.name(), summarize(&m.payload));
            self.record(json!({"dir": "recv", "op": m.name(), "payload": elide(&m.payload)}));
        }
        Ok(msg)
    }

    pub fn send(&mut self, opcode: Opcode, payload: Value) -> io::Result<()> {
        log::debug!("-> {} {}", opcode, summarize(&payload));
        self.record(json!({"dir": "send", "op": opcode.to_string(), "payload": elide(&payload)}));
        write_message(&mut self.writer, opcode, &payload)
    }

    /// Copies exactly `len` raw bytes from the stream into `out`.
    pub fn recv_raw(&mut self, len: u64, out: &mut dyn Write) -> io::Result<()> {
        self.record(json!({"dir": "recv-raw", "len": len}));
        let copied = io::copy(&mut (&mut self.reader).take(len), out)?;
        if copied != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }

    /// Sends exactly `len` raw bytes read from `input`.
    pub fn send_raw(&mut self, len: u64, input: &mut dyn Read) -> io::Result<()> {
        self.record(json!({"dir": "send-raw", "len": len}));
        let copied = io::copy(&mut input.take(len), &mut self.writer)?;
        if copied != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.writer.flush()
    }

    fn record(&mut self, entry: Value) {
        if let Some(t) = &mut self.transcript {
            if writeln!(t, "{entry}").and_then(|_| t.flush()).is_err() {
                log::warn!("transcript write failed; disabling transcript");
                self.transcript = None;
            }
        }
    }
}

/// Replaces long strings (base64 thumbnails, HTML comments) with a marker.
fn elide(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > 200 => Value::String(format!("<{} chars elided>", s.len())),
        Value::Array(a) => Value::Array(a.iter().map(elide).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), elide(v))).collect()),
        other => other.clone(),
    }
}

/// Short, single-line rendering of a payload for debug logs.
fn summarize(v: &Value) -> String {
    let s = elide(v).to_string();
    if s.len() > 300 {
        let mut end = 300;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(op: u64, payload: &str) -> Vec<u8> {
        let body = format!("[{op}, {payload}]");
        format!("{}{}", body.len(), body).into_bytes()
    }

    #[test]
    fn opcode_codes_round_trip() {
        for (op, code) in Opcode::ALL {
            assert_eq!(Opcode::from_code(code), Some(op));
            assert_eq!(op.code(), code);
        }
        assert_eq!(Opcode::from_code(10), None);
    }

    #[test]
    fn reads_consecutive_frames() {
        let mut data = frame(9, r#"{"serverProtocolVersion": 1}"#);
        data.extend(frame(12, "{}"));
        let mut r = Cursor::new(data);
        let m = read_message(&mut r).unwrap().unwrap();
        assert_eq!(m.opcode(), Some(Opcode::GetInitializationInfo));
        assert_eq!(m.payload["serverProtocolVersion"], 1);
        let m = read_message(&mut r).unwrap().unwrap();
        assert_eq!(m.opcode(), Some(Opcode::Noop));
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn length_counts_utf8_bytes() {
        let mut out = Vec::new();
        write_message(&mut out, Opcode::Ok, &json!({"t": "é"})).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        let (len, body) = s.split_at(s.find('[').unwrap());
        assert_eq!(len.parse::<usize>().unwrap(), body.len());
        let m = read_message(&mut Cursor::new(out)).unwrap().unwrap();
        assert_eq!(m.payload["t"], "é");
    }

    #[test]
    fn raw_bytes_follow_a_frame_in_the_same_buffer() {
        let mut data = frame(8, r#"{"length": 5}"#);
        data.extend(b"HELLO");
        data.extend(frame(12, "{}"));
        let mut conn = Connection::new(Cursor::new(data), Vec::new());
        let m = conn.recv().unwrap().unwrap();
        assert_eq!(m.opcode(), Some(Opcode::SendBook));
        let mut book = Vec::new();
        conn.recv_raw(5, &mut book).unwrap();
        assert_eq!(book, b"HELLO");
        assert_eq!(conn.recv().unwrap().unwrap().opcode(), Some(Opcode::Noop));
    }

    #[test]
    fn rejects_malformed_frames() {
        for bad in [
            &b"abc"[..],
            b"5{}",
            b"[0, {}]",
            b"99999999999[",
            b"0[",
            b"3[1]",
        ] {
            assert!(
                read_message(&mut Cursor::new(bad.to_vec())).is_err(),
                "{bad:?}"
            );
        }
        assert!(read_message(&mut Cursor::new(b"12[0, {}".to_vec())).is_err());
    }

    #[test]
    fn unknown_opcode_is_not_an_error() {
        let m = read_message(&mut Cursor::new(frame(99, "{}")))
            .unwrap()
            .unwrap();
        assert_eq!(m.code, 99);
        assert_eq!(m.opcode(), None);
    }

    #[test]
    fn transcript_elides_long_strings() {
        let long = "x".repeat(500);
        assert_eq!(
            elide(&json!({"thumbnail": [1, 2, long]}))["thumbnail"][2],
            "<500 chars elided>"
        );
    }
}
