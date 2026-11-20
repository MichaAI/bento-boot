//! The file-system side of the agent: watching the ask directory and
//! answering on the asker's socket. It polls the directory instead of
//! using inotify; a request shows up within one poll, and the code stays
//! free of `unsafe` outside [`crate::memory`].

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use crate::ask::{self, MAX_ASK_FILE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The `ask.*` file; it disappears once the request is answered or
    /// abandoned.
    pub file: PathBuf,
    pub socket: PathBuf,
    pub message: String,
    pub id: String,
    pub pid: Option<u32>,
    pub not_after: Option<u64>,
    /// Do not even show how many characters were typed.
    pub silent: bool,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Asked(Request),
    Gone(PathBuf),
}

pub struct Agent {
    dir: PathBuf,
    known: BTreeMap<PathBuf, Request>,
    rejected: BTreeMap<PathBuf, String>,
    directory_error: Option<io::ErrorKind>,
}

/// `CLOCK_MONOTONIC` in microseconds.
#[must_use]
pub fn monotonic_usec() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    u64::try_from(ts.tv_sec).unwrap_or(0) * 1_000_000 + u64::try_from(ts.tv_nsec).unwrap_or(0) / 1_000
}

fn alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) else {
        return true;
    };
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn expired(not_after: Option<u64>) -> bool {
    not_after.is_some_and(|t| monotonic_usec() > t)
}

/// Reads an `ask.*` file without following links, only if root wrote it.
fn read_ask_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut f =
        OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)?;
    let meta = f.metadata()?;
    if !meta.file_type().is_file() || meta.uid() != 0 || meta.len() > MAX_ASK_FILE as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an ask file written by root"));
    }
    let mut buf = Vec::with_capacity(MAX_ASK_FILE + 1);
    (&mut f).take(MAX_ASK_FILE as u64 + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

fn request(file: &Path) -> io::Result<Request> {
    let bytes = read_ask_file(file)?;
    let a = ask::parse(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
    Ok(Request {
        file: file.to_owned(),
        socket: PathBuf::from(OsStr::from_bytes(a.socket)),
        message: String::from_utf8_lossy(a.message).chars().filter(|c| !c.is_control()).collect(),
        id: String::from_utf8_lossy(a.id).into_owned(),
        pid: a.pid,
        not_after: a.not_after,
        silent: a.silent,
        echo: a.echo,
    })
}

impl Agent {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), known: BTreeMap::new(), rejected: BTreeMap::new(), directory_error: None }
    }

    #[must_use]
    pub fn system() -> Self {
        Self::new(OsStr::from_bytes(ask::ASK_DIR))
    }

    /// Changes since the last scan: new requests, and requests that were
    /// answered elsewhere, timed out or lost their asker.
    pub fn scan(&mut self) -> Vec<Change> {
        let mut present = Vec::new();
        match fs::read_dir(&self.dir) {
            Ok(entries) => {
                self.directory_error = None;
                for e in entries.flatten() {
                    if e.file_name().as_bytes().starts_with(b"ask.") {
                        present.push(e.path());
                    }
                }
            }
            Err(e) => {
                // The directory is created lazily by the first asker.
                if e.kind() != io::ErrorKind::NotFound && self.directory_error != Some(e.kind()) {
                    eprintln!("bento-boot: cannot scan password requests: {e}");
                }
                self.directory_error = Some(e.kind());
            }
        }
        present.sort();
        self.rejected.retain(|p, _| present.binary_search(p).is_ok());

        let mut changes = Vec::new();
        let gone: Vec<PathBuf> = self
            .known
            .iter()
            .filter(|(p, r)| present.binary_search(p).is_err() || !alive(r.pid) || expired(r.not_after))
            .map(|(p, _)| p.clone())
            .collect();
        for p in gone {
            self.known.remove(&p);
            changes.push(Change::Gone(p));
        }
        for p in present {
            if self.known.contains_key(&p) {
                continue;
            }
            match request(&p) {
                Ok(r) if alive(r.pid) && !expired(r.not_after) => {
                    self.rejected.remove(&p);
                    self.known.insert(p, r.clone());
                    changes.push(Change::Asked(r));
                }
                Ok(_) => {}
                Err(e) => {
                    let reason = e.to_string();
                    if self.rejected.get(&p) != Some(&reason) {
                        eprintln!("bento-boot: cannot read password request {p:?}: {reason}");
                        self.rejected.insert(p, reason);
                    }
                }
            }
        }
        changes
    }
}

/// Sends `datagram` (`+passphrase` or `-`) to the asker, only to a socket
/// root created where the protocol says it lives.
pub fn send(req: &Request, datagram: &[u8]) -> io::Result<()> {
    if !ask::socket_path_ok(req.socket.as_os_str().as_bytes()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket outside the ask directory"));
    }
    let meta = fs::symlink_metadata(&req.socket)?;
    if !meta.file_type().is_socket() || meta.uid() != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a socket owned by root"));
    }
    let sock = UnixDatagram::unbound()?;
    let sent = sock.send_to(datagram, &req.socket)?;
    if sent != datagram.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "short datagram"));
    }
    Ok(())
}

/// Tells the asker no passphrase is coming.
pub fn cancel(req: &Request) -> io::Result<()> {
    send(req, b"-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_sees_new_and_gone_requests() {
        let dir = std::env::temp_dir().join(format!("bento-ask-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut agent = Agent::new(&dir);
        assert!(agent.scan().is_empty());

        let file = dir.join("ask.abc");
        fs::write(&file, b"[Ask]\nSocket=/run/systemd/ask-password/sck.1\nMessage=Disk \x07root\n").unwrap();
        fs::write(dir.join("sck.1"), b"").unwrap();
        let changes = agent.scan();
        // The test runs as whoever runs it; only root's files count.
        if nix_uid_is_root() {
            assert!(matches!(&changes[..], [Change::Asked(r)] if r.message == "Disk root"));
            fs::remove_file(&file).unwrap();
            assert_eq!(agent.scan(), [Change::Gone(file)]);
        } else {
            assert!(changes.is_empty());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    fn nix_uid_is_root() -> bool {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() == 0 }
    }
}
