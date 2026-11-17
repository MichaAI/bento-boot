//! Reading the journal files of this boot: the last lines of one unit for
//! the failure tile, and the tail of everything for the log drawer.
//! In the initrd the journal is in `/run/log/journal`; after the switch
//! the real root's `/var/log/journal` joins it.

use std::path::PathBuf;

use sdjournal::{EntryRef, Journal};

fn open() -> Option<Journal> {
    let dirs: Vec<PathBuf> =
        ["/run/log/journal", "/var/log/journal"].iter().map(PathBuf::from).filter(|p| p.is_dir()).collect();
    if dirs.is_empty() {
        return None;
    }
    Journal::open_dirs(&dirs).ok()
}

/// `_BOOT_ID` of this boot: the kernel's boot id without dashes.
fn boot_id() -> Option<Vec<u8>> {
    let s = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(s.trim().bytes().filter(|&b| b != b'-').collect())
}

fn text(e: &EntryRef, field: &str) -> Option<String> {
    e.get(field).map(|v| String::from_utf8_lossy(v).chars().filter(|c| !c.is_control()).collect())
}

/// Up to `n` of the unit's own last messages in this boot, oldest first.
/// Messages PID 1 writes about the unit ("Starting…", "Failed with
/// result…") are left out: the failure reason already says that.
#[must_use]
pub fn unit_lines(unit: &str, n: usize) -> Vec<String> {
    let (Some(journal), Some(boot)) = (open(), boot_id()) else {
        return Vec::new();
    };
    let mut q = journal.query();
    q.match_exact("_SYSTEMD_UNIT", unit.as_bytes()).match_exact("_BOOT_ID", &boot).reverse(true).limit(n * 4);
    let Ok(iter) = q.iter() else { return Vec::new() };
    let mut lines: Vec<String> = iter
        .filter_map(Result::ok)
        .filter(|e| e.get("_PID") != Some(b"1"))
        .filter_map(|e| text(&e, "MESSAGE"))
        .filter(|m| !m.trim().is_empty())
        .take(n)
        .collect();
    lines.reverse();
    lines
}

/// The last `n` messages of this boot, oldest first, as `who: message`.
/// `None` when there is no journal to read.
#[must_use]
pub fn tail(n: usize) -> Option<Vec<String>> {
    let journal = open()?;
    let boot = boot_id()?;
    let mut q = journal.query();
    q.match_exact("_BOOT_ID", &boot).reverse(true).limit(n);
    let iter = q.iter().ok()?;
    let mut lines: Vec<String> = iter
        .filter_map(Result::ok)
        .filter_map(|e| {
            let msg = text(&e, "MESSAGE")?;
            let who = if e.get("_TRANSPORT") == Some(b"kernel") {
                "kernel".to_owned()
            } else {
                text(&e, "SYSLOG_IDENTIFIER").or_else(|| text(&e, "_COMM")).unwrap_or_else(|| "?".to_owned())
            };
            Some(format!("{who}: {msg}"))
        })
        .collect();
    lines.reverse();
    Some(lines)
}
