//! Keeping the passphrase out of swap and core dumps.

use std::io;

/// No core dumps and no ptrace from non-root for this process.
pub fn harden_process() -> io::Result<()> {
    // SAFETY: plain prctl with integer arguments.
    let r = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Locks the pages under `bytes` into RAM and leaves them out of core
/// dumps. The memory must stay where it is for as long as it is in use,
/// so call it on something boxed that is never moved out of its box.
pub fn lock(bytes: &[u8]) -> io::Result<()> {
    // SAFETY: sysconf has no preconditions.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    let start = bytes.as_ptr() as usize & !(page - 1);
    let end = (bytes.as_ptr() as usize + bytes.len()).div_ceil(page) * page;
    let ptr = start as *mut libc::c_void;
    // SAFETY: the range covers whole pages that contain `bytes`, which is
    // mapped memory of this process; mlock and madvise do not touch the
    // contents.
    unsafe {
        if libc::mlock(ptr, end - start) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::madvise(ptr, end - start, libc::MADV_DONTDUMP) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
