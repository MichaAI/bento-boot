//! Kernel messages from `/dev/kmsg`, for the log drawer when there is no
//! journal yet. Each record is `prio,seq,usec,flags;text` (see the
//! kernel's `Documentation/ABI/testing/dev-kmsg`).

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;

pub struct Kmsg {
    file: Option<File>,
    lines: VecDeque<String>,
    keep: usize,
}

/// The text of one record, or `None` for continuation lines and noise.
#[must_use]
pub fn parse_record(rec: &[u8]) -> Option<String> {
    let rec = std::str::from_utf8(rec).ok()?;
    let (_, text) = rec.split_once(';')?;
    let first = text.lines().next()?.trim_end();
    (!first.is_empty()).then(|| first.chars().filter(|c| !c.is_control()).collect())
}

impl Kmsg {
    #[must_use]
    pub fn open(keep: usize) -> Self {
        let file =
            OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open("/dev/kmsg").ok();
        Self { file, lines: VecDeque::new(), keep }
    }

    /// Reads whatever arrived; each `read` returns one whole record.
    pub fn poll(&mut self) -> &VecDeque<String> {
        let Some(f) = self.file.as_mut() else { return &self.lines };
        let mut buf = [0u8; 8192];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(line) = parse_record(&buf[..n]) {
                        self.lines.push_back(format!("kernel: {line}"));
                        while self.lines.len() > self.keep {
                            self.lines.pop_front();
                        }
                    }
                }
                // EPIPE: records were overwritten before we read them; the
                // next read continues with the oldest one left.
                Err(e) if e.raw_os_error() == Some(libc::EPIPE) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.file = None;
                    break;
                }
            }
        }
        &self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records() {
        assert_eq!(
            parse_record(b"6,339,5140900,-;NET: Registered protocol family 10\n").as_deref(),
            Some("NET: Registered protocol family 10")
        );
        assert_eq!(
            parse_record(b"6,340,5140901,-;usb 1-1: new device\n SUBSYSTEM=usb\n").as_deref(),
            Some("usb 1-1: new device")
        );
        assert_eq!(parse_record(b"garbage"), None);
    }
}
