//! Noticing the switch from the initrd and following PID 1 into the real
//! root.
//!
//! When systemd switches root it moves `/proc`, `/sys`, `/dev` and `/run`
//! into the new root first, so in the old root, where we still are, `/proc`
//! is an empty directory by the time we look. A handle on `/proc` opened
//! while still in the initrd keeps pointing at the moved mount: through it
//! we see PID 1's root and can `chroot` into it.

use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

static PROC: OnceLock<Option<File>> = OnceLock::new();

/// Opens `/proc` for later; call it before the switch can happen.
pub fn hold_proc() {
    PROC.get_or_init(|| File::open("/proc").ok());
}

fn proc_fd() -> io::Result<libc::c_int> {
    hold_proc();
    PROC.get()
        .and_then(Option::as_ref)
        .map(AsRawFd::as_raw_fd)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no /proc"))
}

/// `/etc/initrd-release` exists only in an initrd (os-release(5)).
#[must_use]
pub fn in_initrd() -> bool {
    Path::new("/etc/initrd-release").exists()
}

fn same_root() -> io::Result<bool> {
    let ours = fs::metadata("/")?;
    let name = CString::new("1/root").expect("no NUL");
    // SAFETY: an all-zero stat is a valid out-parameter.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid directory fd, NUL-terminated path, writable stat.
    if unsafe { libc::fstatat(proc_fd()?, name.as_ptr(), &raw mut st, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ours.dev() == st.st_dev && ours.ino() == st.st_ino)
}

/// Whether PID 1 now runs in another root than ours: it switched from the
/// initrd and we have not followed yet.
#[must_use]
pub fn pid1_moved() -> bool {
    same_root().is_ok_and(|same| !same)
}

/// Waits until PID 1 runs in another root than ours, then moves into it.
/// Needs `CAP_SYS_CHROOT`.
pub fn wait_for_new_root(timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        match same_root() {
            Ok(false) => break,
            Ok(true) | Err(_) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(50)),
            Ok(true) => return Err(io::Error::new(io::ErrorKind::TimedOut, "PID 1 stayed in our root")),
            Err(e) => return Err(e),
        }
    }
    let target = CString::new("1/root").expect("no NUL");
    let slash = CString::new("/").expect("no NUL");
    let proc = proc_fd()?;
    // SAFETY: a valid directory fd and NUL-terminated paths. chroot and
    // chdir change the whole process's root and working directory, which
    // is what following PID 1 means.
    unsafe {
        if libc::fchdir(proc) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::chroot(target.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::chdir(slash.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
