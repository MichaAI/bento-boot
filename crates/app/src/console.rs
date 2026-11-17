//! The text console under the picture. While we draw, the active VT is in
//! graphics mode (the kernel stops drawing text over us) and its keyboard
//! is off (keystrokes, a passphrase included, reach only us through
//! libinput, not a console password agent or a shell behind us).

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

const KDSETMODE: libc::c_ulong = 0x4B3A;
const KDGKBMODE: libc::c_ulong = 0x4B44;
const KDSKBMODE: libc::c_ulong = 0x4B45;
const VT_GETSTATE: libc::c_ulong = 0x5603;
const KD_TEXT: libc::c_int = 0;
const KD_GRAPHICS: libc::c_int = 1;
const K_UNICODE: libc::c_int = 3;
const K_OFF: libc::c_int = 4;

/// Left behind when the screen hands over to a display manager, so the
/// clean-up after us does not flash the text console in between.
pub const RETAIN_MARKER: &str = "/run/bento-boot/retain";

#[repr(C)]
#[derive(Default)]
struct VtStat {
    active: u16,
    signal: u16,
    state: u16,
}

fn ioctl_int(f: &File, req: libc::c_ulong, arg: libc::c_int) -> io::Result<()> {
    // SAFETY: the requests used here take an int argument by value.
    let r = unsafe { libc::ioctl(f.as_raw_fd(), req, arg) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn active_vt() -> io::Result<File> {
    let tty0 = OpenOptions::new().read(true).write(true).open("/dev/tty0")?;
    let mut st = VtStat::default();
    // SAFETY: VT_GETSTATE writes a `struct vt_stat`, which VtStat mirrors.
    let r = unsafe { libc::ioctl(tty0.as_raw_fd(), VT_GETSTATE, &raw mut st) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    OpenOptions::new().read(true).write(true).open(format!("/dev/tty{}", st.active))
}

pub struct Console {
    tty: File,
    keyboard: libc::c_int,
    released: bool,
}

impl Console {
    pub fn take() -> io::Result<Self> {
        let tty = active_vt()?;
        let mut keyboard: libc::c_int = K_UNICODE;
        // SAFETY: KDGKBMODE writes an int.
        unsafe { libc::ioctl(tty.as_raw_fd(), KDGKBMODE, &raw mut keyboard) };
        ioctl_int(&tty, KDSETMODE, KD_GRAPHICS)?;
        let _ = ioctl_int(&tty, KDSKBMODE, K_OFF);
        let _ = std::fs::remove_file(RETAIN_MARKER);
        Ok(Self { tty, keyboard, released: false })
    }

    /// Gives the keyboard back. With `keep_graphics` the VT stays in
    /// graphics mode for the display manager to take over without a flash
    /// of text.
    pub fn release(mut self, keep_graphics: bool) {
        let _ = ioctl_int(&self.tty, KDSKBMODE, self.keyboard);
        if keep_graphics {
            let _ = std::fs::write(RETAIN_MARKER, b"");
        } else {
            let _ = ioctl_int(&self.tty, KDSETMODE, KD_TEXT);
        }
        self.released = true;
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        if !self.released {
            let _ = ioctl_int(&self.tty, KDSKBMODE, self.keyboard);
            let _ = ioctl_int(&self.tty, KDSETMODE, KD_TEXT);
        }
    }
}

/// For `ExecStopPost=`: whatever happened to us, the console is usable
/// again. Keeps graphics mode only if we left it on purpose.
pub fn restore() -> io::Result<()> {
    let tty = active_vt()?;
    let _ = ioctl_int(&tty, KDSKBMODE, K_UNICODE);
    if !Path::new(RETAIN_MARKER).exists() {
        ioctl_int(&tty, KDSETMODE, KD_TEXT)?;
    }
    Ok(())
}
