//! Just enough of the D-Bus wire protocol to talk to systemd, from the
//! specification (<https://dbus.freedesktop.org/doc/dbus-specification.html>).
//!
//! Why not a full D-Bus library: on its private socket systemd stamps every
//! message with the sender `org.freedesktop.systemd1`, a well-known name,
//! where libraries expect a unique one (`:1.42`) and give up. This client
//! does not care who the sender is, runs on one thread without an executor,
//! and has a timeout on every call.
//!
//! It speaks: EXTERNAL authentication, method calls with string arguments,
//! and decoding of any reply or signal body into [`Value`]s.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Double(f64),
    /// Strings, object paths and signatures.
    Str(String),
    Array(Vec<Value>),
    Struct(Vec<Value>),
    Variant(Box<Value>),
    UnixFd(u32),
}

impl Value {
    #[must_use]
    pub fn str(&self) -> Option<&str> {
        match self.unwrap_variant() {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn u64(&self) -> Option<u64> {
        match self.unwrap_variant() {
            Self::U64(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn u32(&self) -> Option<u32> {
        match self.unwrap_variant() {
            Self::U32(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn i32(&self) -> Option<i32> {
        match self.unwrap_variant() {
            Self::I32(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn items(&self) -> &[Value] {
        match self.unwrap_variant() {
            Self::Array(v) | Self::Struct(v) => v,
            _ => &[],
        }
    }

    /// `as` as strings.
    #[must_use]
    pub fn strings(&self) -> Vec<String> {
        self.items().iter().filter_map(|v| v.str().map(str::to_owned)).collect()
    }

    /// Looks `key` up in an `a{sv}`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.items().iter().find_map(|e| match e.items() {
            [k, v] if k.str() == Some(key) => Some(v),
            _ => None,
        })
    }

    fn unwrap_variant(&self) -> &Self {
        let mut v = self;
        while let Self::Variant(inner) = v {
            v = inner;
        }
        v
    }
}

const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;

/// A received message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub kind: u8,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error: Option<String>,
    pub reply_serial: Option<u32>,
    pub body: Vec<Value>,
}

impl Message {
    #[must_use]
    pub fn is_signal(&self) -> bool {
        self.kind == SIGNAL
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

// ---- encoding (only what calls need: strings in, little endian)

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn pad(&mut self, align: usize) {
        while !self.buf.len().is_multiple_of(align) {
            self.buf.push(0);
        }
    }

    fn u32(&mut self, n: u32) {
        self.pad(4);
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    fn string(&mut self, s: &str) {
        self.u32(u32::try_from(s.len()).unwrap_or(u32::MAX));
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, s: &str) {
        self.buf.push(u8::try_from(s.len()).unwrap_or(u8::MAX));
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// One header field `(yv)` holding a string-like value of type `sig`.
    fn field(&mut self, code: u8, sig: &str, value: &str) {
        self.pad(8);
        self.buf.push(code);
        self.signature(sig);
        if sig == "g" {
            self.signature(value);
        } else {
            self.string(value);
        }
    }
}

fn encode_call(serial: u32, dest: Option<&str>, path: &str, iface: &str, member: &str, args: &[&str]) -> Vec<u8> {
    let mut body = Writer { buf: Vec::new() };
    for a in args {
        body.string(a);
    }
    let mut m = Writer { buf: vec![b'l', METHOD_CALL, 0, 1] };
    m.u32(u32::try_from(body.buf.len()).unwrap_or(u32::MAX));
    m.u32(serial);
    m.u32(0); // fields length, filled in below
    let fields_start = m.buf.len();
    m.field(1, "o", path);
    m.field(2, "s", iface);
    m.field(3, "s", member);
    if let Some(d) = dest {
        m.field(6, "s", d);
    }
    if !args.is_empty() {
        m.field(8, "g", &"s".repeat(args.len()));
    }
    let fields_len = u32::try_from(m.buf.len() - fields_start).unwrap_or(u32::MAX);
    m.buf[12..16].copy_from_slice(&fields_len.to_le_bytes());
    m.pad(8);
    m.buf.extend_from_slice(&body.buf);
    m.buf
}

// ---- decoding

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    big: bool,
    depth: u32,
}

impl Reader<'_> {
    fn align(&mut self, n: usize) -> io::Result<()> {
        let p = self.pos.div_ceil(n) * n;
        if p > self.buf.len() {
            return Err(bad("short message"));
        }
        self.pos = p;
        Ok(())
    }

    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or_else(|| bad("short message"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn fixed<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.align(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        if self.big {
            a.reverse();
        }
        Ok(a)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn string(&mut self, len: usize) -> io::Result<String> {
        let s = String::from_utf8_lossy(self.take(len)?).into_owned();
        self.take(1)?; // NUL
        Ok(s)
    }

    /// Decodes one value of the single complete type at the start of `sig`,
    /// returning it and the rest of the signature.
    fn value<'s>(&mut self, sig: &'s [u8]) -> io::Result<(Value, &'s [u8])> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(bad("nesting too deep"));
        }
        let (&c, rest) = sig.split_first().ok_or_else(|| bad("empty signature"))?;
        let out = match c {
            b'y' => (Value::Byte(self.take(1)?[0]), rest),
            b'b' => (Value::Bool(self.u32()? != 0), rest),
            b'n' => (Value::I16(i16::from_le_bytes(self.fixed()?)), rest),
            b'q' => (Value::U16(u16::from_le_bytes(self.fixed()?)), rest),
            b'i' => (Value::I32(i32::from_le_bytes(self.fixed()?)), rest),
            b'u' => (Value::U32(self.u32()?), rest),
            b'h' => (Value::UnixFd(self.u32()?), rest),
            b'x' => (Value::I64(i64::from_le_bytes(self.fixed()?)), rest),
            b't' => (Value::U64(u64::from_le_bytes(self.fixed()?)), rest),
            b'd' => (Value::Double(f64::from_le_bytes(self.fixed()?)), rest),
            b's' | b'o' => {
                let n = self.u32()? as usize;
                (Value::Str(self.string(n)?), rest)
            }
            b'g' => {
                let n = usize::from(self.take(1)?[0]);
                (Value::Str(self.string(n)?), rest)
            }
            b'v' => {
                let n = usize::from(self.take(1)?[0]);
                let inner = self.take(n)?.to_vec();
                self.take(1)?;
                let (v, left) = self.value(&inner)?;
                if !left.is_empty() {
                    return Err(bad("variant holds more than one type"));
                }
                (Value::Variant(Box::new(v)), rest)
            }
            b'a' => {
                let len = self.u32()? as usize;
                let elem_end = complete_type(rest)?;
                let elem = &rest[..elem_end];
                self.align(alignment(elem[0]))?;
                let end =
                    self.pos.checked_add(len).filter(|&e| e <= self.buf.len()).ok_or_else(|| bad("array too long"))?;
                let mut items = Vec::new();
                while self.pos < end {
                    let (v, _) = self.value(elem)?;
                    items.push(v);
                }
                (Value::Array(items), &rest[elem_end..])
            }
            b'(' | b'{' => {
                let close = if c == b'(' { b')' } else { b'}' };
                self.align(8)?;
                let mut fields = Vec::new();
                let mut s = rest;
                while s.first() != Some(&close) {
                    let (v, r) = self.value(s)?;
                    fields.push(v);
                    s = r;
                }
                (Value::Struct(fields), &s[1..])
            }
            _ => return Err(bad("unknown type")),
        };
        self.depth -= 1;
        Ok(out)
    }
}

fn alignment(c: u8) -> usize {
    match c {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

/// Length of the first complete type in `sig`.
fn complete_type(sig: &[u8]) -> io::Result<usize> {
    match sig.first() {
        Some(b'a') => Ok(1 + complete_type(&sig[1..])?),
        Some(&open @ (b'(' | b'{')) => {
            let close = if open == b'(' { b')' } else { b'}' };
            let mut i = 1;
            while sig.get(i) != Some(&close) {
                if i >= sig.len() {
                    return Err(bad("unclosed struct"));
                }
                i += complete_type(&sig[i..])?;
            }
            Ok(i + 1)
        }
        Some(_) => Ok(1),
        None => Err(bad("empty signature")),
    }
}

/// Decodes one whole message (header and body) from `buf`.
pub fn decode(buf: &[u8]) -> io::Result<Message> {
    let big = match buf.first() {
        Some(b'l') => false,
        Some(b'B') => true,
        _ => return Err(bad("bad endianness")),
    };
    let mut r = Reader { buf, pos: 1, big, depth: 0 };
    let kind = r.take(1)?[0];
    r.take(2)?;
    let body_len = r.u32()? as usize;
    r.u32()?; // serial
    let (fields, _) = r.value(b"a(yv)")?;
    r.align(8)?;
    let body_start = r.pos;
    let mut m =
        Message { kind, path: None, interface: None, member: None, error: None, reply_serial: None, body: Vec::new() };
    let mut sig = String::new();
    for f in fields.items() {
        let [Value::Byte(code), v] = f.items() else { continue };
        match code {
            1 => m.path = v.str().map(str::to_owned),
            2 => m.interface = v.str().map(str::to_owned),
            3 => m.member = v.str().map(str::to_owned),
            4 => m.error = v.str().map(str::to_owned),
            5 => m.reply_serial = v.u32(),
            8 => sig = v.str().unwrap_or_default().to_owned(),
            _ => {}
        }
    }
    let body = buf.get(body_start..body_start + body_len).ok_or_else(|| bad("short body"))?;
    // Alignment in the body counts from its start, which is 8-aligned.
    let mut br = Reader { buf: body, pos: 0, big, depth: 0 };
    let mut s = sig.as_bytes();
    while !s.is_empty() {
        let (v, rest) = br.value(s)?;
        m.body.push(v);
        s = rest;
    }
    Ok(m)
}

/// A blocking connection.
pub struct Connection {
    stream: BufReader<UnixStream>,
    dest: Option<&'static str>,
    serial: u32,
    /// Signals that arrived while waiting for a reply.
    queued: VecDeque<Message>,
}

impl Connection {
    /// Connects and authenticates as our own uid. With `dest`, it is a bus
    /// (not a peer) and calls carry the destination.
    pub fn connect(path: &Path, dest: Option<&'static str>) -> io::Result<Self> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        // SAFETY: geteuid cannot fail.
        let uid = unsafe { libc::geteuid() };
        let hex: String = uid.to_string().bytes().map(|b| format!("{b:02x}")).collect();
        stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.get_mut().set_read_timeout(Some(Duration::from_secs(5)))?;
        (&mut reader).take(512).read_line(&mut line)?;
        if !line.starts_with("OK ") {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("auth refused: {}", line.trim())));
        }
        reader.get_mut().write_all(b"BEGIN\r\n")?;
        let mut c = Self { stream: reader, dest, serial: 0, queued: VecDeque::new() };
        if dest.is_some() {
            c.call("/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[])?;
        }
        Ok(c)
    }

    /// Reads one message, waiting at most `timeout` (`None`: forever).
    fn read(&mut self, timeout: Option<Duration>) -> io::Result<Message> {
        self.stream.get_mut().set_read_timeout(timeout)?;
        let mut head = [0u8; 16];
        self.stream.read_exact(&mut head)?;
        let big = head[0] == b'B';
        let word = |b: &[u8]| {
            let a: [u8; 4] = b.try_into().unwrap_or([0; 4]);
            if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) }
        };
        let body = word(&head[4..8]) as usize;
        let fields = word(&head[12..16]) as usize;
        let total = (16 + fields).div_ceil(8) * 8 + body;
        if total > 64 * 1024 * 1024 {
            return Err(bad("message too large"));
        }
        let mut buf = head.to_vec();
        buf.resize(total, 0);
        self.stream.read_exact(&mut buf[16..])?;
        decode(&buf)
    }

    /// Calls a method with string arguments and waits for its reply;
    /// signals that come in between are kept for [`Connection::next_signal`].
    pub fn call(&mut self, path: &str, iface: &str, member: &str, args: &[&str]) -> io::Result<Vec<Value>> {
        self.serial = self.serial.wrapping_add(1).max(1);
        let serial = self.serial;
        let msg = encode_call(serial, self.dest, path, iface, member, args);
        self.stream.get_mut().write_all(&msg)?;
        loop {
            let m = self.read(Some(Duration::from_secs(5)))?;
            match m.kind {
                METHOD_RETURN if m.reply_serial == Some(serial) => return Ok(m.body),
                ERROR if m.reply_serial == Some(serial) => {
                    let text = m.body.first().and_then(Value::str).unwrap_or_default();
                    return Err(io::Error::other(format!("{}: {text}", m.error.unwrap_or_default())));
                }
                SIGNAL => self.queued.push_back(m),
                _ => {}
            }
        }
    }

    /// The next signal, waiting as long as it takes.
    pub fn next_signal(&mut self) -> io::Result<Message> {
        if let Some(m) = self.queued.pop_front() {
            return Ok(m);
        }
        loop {
            let m = self.read(None)?;
            if m.is_signal() {
                return Ok(m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_decodes_back() {
        let bytes = encode_call(
            7,
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "GetUnit",
            &["sshd.service"],
        );
        let m = decode(&bytes).unwrap();
        assert_eq!(m.kind, METHOD_CALL);
        assert_eq!(m.path.as_deref(), Some("/org/freedesktop/systemd1"));
        assert_eq!(m.member.as_deref(), Some("GetUnit"));
        assert_eq!(m.body, [Value::Str("sshd.service".into())]);
    }

    /// A hand-made signal body `a{sv}` with a string, a u64 and an `as`.
    #[test]
    fn dictionaries_and_variants() {
        let mut w = Writer { buf: Vec::new() };
        let mut entries = Writer { buf: Vec::new() };
        // Entries are 8-aligned relative to the body; the array length
        // comes first, at offset 0, so entries start at 8.
        entries.buf.resize(8, 0);
        entries.string("ActiveState");
        entries.signature("s");
        entries.string("active");
        entries.pad(8);
        entries.string("ActiveEnterTimestampMonotonic");
        entries.signature("t");
        entries.pad(8);
        entries.buf.extend_from_slice(&1234u64.to_le_bytes());
        entries.pad(8);
        entries.string("After");
        entries.signature("as");
        entries.u32(0);
        let mut list = Writer { buf: Vec::new() };
        list.string("a.target");
        let at = entries.buf.len();
        entries.buf.extend_from_slice(&list.buf);
        let n = u32::try_from(list.buf.len()).unwrap();
        entries.buf[at - 4..at].copy_from_slice(&n.to_le_bytes());
        w.u32(u32::try_from(entries.buf.len() - 8).unwrap());
        w.buf.extend_from_slice(&entries.buf[4..]);
        let mut r = Reader { buf: &w.buf, pos: 0, big: false, depth: 0 };
        let (v, rest) = r.value(b"a{sv}").unwrap();
        assert!(rest.is_empty());
        assert_eq!(v.get("ActiveState").and_then(Value::str), Some("active"));
        assert_eq!(v.get("ActiveEnterTimestampMonotonic").and_then(Value::u64), Some(1234));
        assert_eq!(v.get("After").map(Value::strings), Some(vec!["a.target".to_owned()]));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut seed = 1u64;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 96) as usize;
            let mut buf: Vec<u8> = (0..len).map(|i| (seed >> (i % 56)) as u8).collect();
            if let Some(b) = buf.first_mut() {
                *b = b'l';
            }
            let _ = decode(&buf);
        }
    }

    #[test]
    fn complete_types() {
        assert_eq!(complete_type(b"a(usssoo)x").unwrap(), 9);
        assert_eq!(complete_type(b"a{sv}").unwrap(), 5);
        assert!(complete_type(b"(ss").is_err());
    }
}
