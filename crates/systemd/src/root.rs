//! Noticing the switch from the initrd and following PID 1 into the real
//! root.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// `/etc/initrd-release` exists only in an initrd (os-release(5)).
#[must_use]
pub fn in_initrd() -> bool {
    Path::new("/etc/initrd-release").exists()
}

fn same_root() -> io::Result<bool> {
    let ours = fs::metadata("/")?;
    let pid1 = fs::metadata("/proc/1/root/")?;
    Ok(ours.dev() == pid1.dev() && ours.ino() == pid1.ino())
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
    let target = CString::new("/proc/1/root").expect("no NUL");
    let slash = CString::new("/").expect("no NUL");
    // SAFETY: both are valid NUL-terminated paths.
    unsafe {
        if libc::chroot(target.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::chdir(slash.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
