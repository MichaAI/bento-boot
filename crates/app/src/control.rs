//! `bento-boot quit`: a root-only socket the quit unit uses to ask the
//! screen to fade out before the display manager starts, and to wait
//! until it has.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

pub const DIR: &str = "/run/bento-boot";
pub const SOCKET: &str = "/run/bento-boot/control";

pub struct Control {
    listener: UnixListener,
    waiting: Vec<UnixStream>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuitRequest {
    /// Leave the console in graphics mode for whoever comes next.
    pub retain: bool,
}

impl Control {
    pub fn bind() -> io::Result<Self> {
        std::fs::create_dir_all(DIR)?;
        std::fs::set_permissions(DIR, std::fs::Permissions::from_mode(0o700))?;
        let _ = std::fs::remove_file(SOCKET);
        let listener = UnixListener::bind(SOCKET)?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, waiting: Vec::new() })
    }

    pub fn poll(&mut self) -> Option<QuitRequest> {
        let (stream, _) = self.listener.accept().ok()?;
        stream.set_nonblocking(false).ok()?;
        stream.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
        let mut line = String::new();
        BufReader::new((&stream).take(64)).read_line(&mut line).ok()?;
        let req = match line.trim() {
            "quit" => QuitRequest { retain: false },
            "quit retain" => QuitRequest { retain: true },
            _ => return None,
        };
        self.waiting.push(stream);
        Some(req)
    }

    /// Tells everyone who asked that the screen is gone.
    pub fn finish(mut self) {
        for mut s in self.waiting.drain(..) {
            let _ = s.write_all(b"ok\n");
        }
        let _ = std::fs::remove_file(SOCKET);
    }
}

/// Client side. Succeeds at once when no screen is running.
pub fn quit(retain: bool, timeout: Duration) -> io::Result<()> {
    if !Path::new(SOCKET).exists() {
        return Ok(());
    }
    let mut s = match UnixStream::connect(SOCKET) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return Ok(()),
        Err(e) => return Err(e),
    };
    s.set_read_timeout(Some(timeout))?;
    s.write_all(if retain { b"quit retain\n" } else { b"quit\n" })?;
    let mut reply = String::new();
    // EOF (the screen exited) counts as done too.
    let _ = BufReader::new(s).read_line(&mut reply);
    Ok(())
}
