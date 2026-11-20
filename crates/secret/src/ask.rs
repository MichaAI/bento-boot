//! The `ask.*` files of systemd's password agent protocol
//! (<https://systemd.io/PASSWORD_AGENTS/>), parsed strictly.
//!
//! The file is a small ini with an `[Ask]` section. Only what an agent
//! needs is read; a known key given twice, an over-long file or a socket
//! anywhere but directly in `/run/systemd/ask-password/` is an error rather
//! than a guess.

/// systemd writes a few hundred bytes; anything this large is not one of
/// its files.
pub const MAX_ASK_FILE: usize = 4096;

pub const ASK_DIR: &[u8] = b"/run/systemd/ask-password/";

/// `sun_path` is 108 bytes including the terminating NUL.
const MAX_SOCKET_PATH: usize = 107;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    TooLong,
    NoAskSection,
    BadLine,
    Duplicate,
    NoSocket,
    BadSocket,
    BadNumber,
    BadBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask<'a> {
    pub socket: &'a [u8],
    pub message: &'a [u8],
    pub id: &'a [u8],
    pub pid: Option<u32>,
    /// `CLOCK_MONOTONIC` microseconds after which nobody waits any more;
    /// `None` or 0 for never.
    pub not_after: Option<u64>,
    pub echo: bool,
    pub silent: bool,
    pub accept_cached: bool,
}

/// A socket path an agent may answer to: directly inside
/// [`ASK_DIR`], a plain file name, and short enough for `sun_path`.
#[must_use]
pub fn socket_path_ok(p: &[u8]) -> bool {
    if p.len() > MAX_SOCKET_PATH || p.len() <= ASK_DIR.len() || !p.starts_with(ASK_DIR) {
        return false;
    }
    // systemd can append "/sck.*" to a directory that already ends in
    // '/'. Repeated separators still refer directly to ASK_DIR; only the
    // basename must be free of separators and traversal components.
    let mut name = &p[ASK_DIR.len()..];
    while let Some(rest) = name.strip_prefix(b"/") {
        name = rest;
    }
    !name.is_empty()
        && name != b"."
        && name != b".."
        && name.iter().all(|&b| b != b'/' && b != 0 && b > b' ' && b != 0x7f)
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

fn number(s: &[u8]) -> Result<u64, ParseError> {
    if s.is_empty() || s.len() > 20 {
        return Err(ParseError::BadNumber);
    }
    let mut n: u64 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return Err(ParseError::BadNumber);
        }
        n = n.checked_mul(10).and_then(|n| n.checked_add(u64::from(b - b'0'))).ok_or(ParseError::BadNumber)?;
    }
    Ok(n)
}

fn boolean(s: &[u8]) -> Result<bool, ParseError> {
    match s {
        b"1" | b"yes" | b"true" | b"on" => Ok(true),
        b"0" | b"no" | b"false" | b"off" => Ok(false),
        _ => Err(ParseError::BadBool),
    }
}

fn set<T>(slot: &mut Option<T>, value: T) -> Result<(), ParseError> {
    if slot.is_some() {
        return Err(ParseError::Duplicate);
    }
    *slot = Some(value);
    Ok(())
}

/// Parses one `ask.*` file.
pub fn parse(input: &[u8]) -> Result<Ask<'_>, ParseError> {
    if input.len() > MAX_ASK_FILE {
        return Err(ParseError::TooLong);
    }
    let mut in_ask = false;
    let mut seen_ask = false;
    let mut socket = None;
    let mut message = None;
    let mut id = None;
    let mut pid = None;
    let mut not_after = None;
    let mut echo = None;
    let mut silent = None;
    let mut accept_cached = None;

    for raw in input.split(|&b| b == b'\n') {
        let line = trim(raw);
        if line.is_empty() || line[0] == b'#' || line[0] == b';' {
            continue;
        }
        if line[0] == b'[' {
            if line.last() != Some(&b']') {
                return Err(ParseError::BadLine);
            }
            in_ask = line == b"[Ask]";
            seen_ask |= in_ask;
            continue;
        }
        let Some(eq) = line.iter().position(|&b| b == b'=') else {
            return Err(ParseError::BadLine);
        };
        if !in_ask {
            continue;
        }
        let key = trim(&line[..eq]);
        let value = trim(&line[eq + 1..]);
        match key {
            b"Socket" => set(&mut socket, value)?,
            b"Message" => set(&mut message, value)?,
            b"Id" => set(&mut id, value)?,
            b"PID" => {
                let n = number(value)?;
                set(&mut pid, u32::try_from(n).map_err(|_| ParseError::BadNumber)?)?;
            }
            b"NotAfter" => set(&mut not_after, number(value)?)?,
            b"Echo" => set(&mut echo, boolean(value)?)?,
            b"Silent" => set(&mut silent, boolean(value)?)?,
            b"AcceptCached" => set(&mut accept_cached, boolean(value)?)?,
            _ => {}
        }
    }

    if !seen_ask {
        return Err(ParseError::NoAskSection);
    }
    let socket = socket.ok_or(ParseError::NoSocket)?;
    if !socket_path_ok(socket) {
        return Err(ParseError::BadSocket);
    }
    Ok(Ask {
        socket,
        message: message.unwrap_or(b""),
        id: id.unwrap_or(b""),
        pid: pid.filter(|&p| p > 0),
        not_after: not_after.filter(|&t| t > 0),
        echo: echo.unwrap_or(false),
        silent: silent.unwrap_or(false),
        accept_cached: accept_cached.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"[Ask]\n\
PID=412\n\
Socket=/run/systemd/ask-password/sck.3b1d9e8a2a6d4e4c\n\
AcceptCached=1\n\
Echo=0\n\
NotAfter=0\n\
Id=cryptsetup:/dev/disk/by-uuid/0f1e\n\
Message=Please enter passphrase for disk root (cryptroot):\n";

    #[test]
    fn parses_a_cryptsetup_request() {
        let a = parse(SAMPLE).unwrap();
        assert_eq!(a.pid, Some(412));
        assert_eq!(a.socket, b"/run/systemd/ask-password/sck.3b1d9e8a2a6d4e4c");
        assert!(a.message.starts_with(b"Please enter passphrase"));
        assert_eq!(a.not_after, None);
        assert!(a.accept_cached && !a.echo && !a.silent);
    }

    #[test]
    fn accepts_systemd_socket_with_repeated_directory_separator() {
        let input = b"[Ask]\nSocket=/run/systemd/ask-password//sck.123\n";
        assert_eq!(parse(input).unwrap().socket, b"/run/systemd/ask-password//sck.123");
    }

    #[test]
    fn rejects_sockets_elsewhere() {
        for bad in [
            &b"/tmp/sck.x"[..],
            b"/run/systemd/ask-password/",
            b"/run/systemd/ask-password/..",
            b"/run/systemd/ask-password///",
            b"/run/systemd/ask-password//..",
            b"/run/systemd/ask-password//../other",
            b"/run/systemd/ask-password//a/b",
            b"/run/systemd/ask-password/../../tmp/x",
            b"/run/systemd/ask-password/a/b",
            b"/run/systemd/ask-password/a b",
            b"/run/systemd/ask-password-x/sck",
        ] {
            assert!(!socket_path_ok(bad), "{}", String::from_utf8_lossy(bad));
        }
        let long = [ASK_DIR, &[b'a'; 90][..]].concat();
        assert!(!socket_path_ok(&long));
    }

    #[test]
    fn strictness() {
        assert_eq!(parse(b"Socket=/run/systemd/ask-password/s\n"), Err(ParseError::NoAskSection));
        assert_eq!(parse(b"[Ask]\nPID=1\n"), Err(ParseError::NoSocket));
        assert_eq!(
            parse(b"[Ask]\nSocket=/run/systemd/ask-password/a\nSocket=/run/systemd/ask-password/b\n"),
            Err(ParseError::Duplicate)
        );
        assert_eq!(parse(b"[Ask]\nSocket=/run/systemd/ask-password/a\nPID=99999999999\n"), Err(ParseError::BadNumber));
        assert_eq!(parse(b"[Ask]\nnonsense\n"), Err(ParseError::BadLine));
        assert_eq!(parse(&[b' '; MAX_ASK_FILE + 1]), Err(ParseError::TooLong));
        // Keys of other sections do not count.
        let other = b"[Other]\nSocket=/tmp/x\n[Ask]\nSocket=/run/systemd/ask-password/a\n";
        assert_eq!(parse(other).unwrap().socket, b"/run/systemd/ask-password/a");
    }
}
