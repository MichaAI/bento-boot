//! Following PID 1 over D-Bus (`org.freedesktop.systemd1(5)`).
//!
//! In the initrd there is no bus daemon, so this talks to the manager
//! directly on its private socket, `/run/systemd/private`, which only root
//! may open. Elsewhere it falls back to the system bus. After `Subscribe`
//! the manager sends `JobNew`, `JobRemoved`, `StartupFinished` and
//! `PropertiesChanged` for units and jobs; everything is turned into
//! [`bento_boot_model::Event`]s.
//!
//! When the initrd's manager switches to the real root it re-executes and
//! the connection drops. [`follow`] then waits until PID 1's root is no
//! longer ours, moves into it with `chroot("/proc/1/root")`, and connects
//! to the new manager.

pub mod dbus;
pub mod journal;
pub mod kmsg;
mod names;
pub(crate) mod root;
mod session;

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use bento_boot_model::{Event, Stage};

pub use names::{job_id_from_path, unit_from_path};
pub use root::{in_initrd, wait_for_new_root};
pub use session::Cause;

/// What the link reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Boot(Event),
    /// Why a unit failed, read from its service properties.
    Cause {
        unit: String,
        cause: Cause,
    },
    /// The unit's last messages in the journal, oldest first.
    Journal {
        unit: String,
        lines: Vec<String>,
    },
    /// Could not reach PID 1; the text says why.
    Unreachable(String),
}

/// Whether the boot is already over, e.g. when the unit was started by
/// hand or by `nixos-rebuild switch` on a running system.
#[must_use]
pub fn startup_finished() -> Option<bool> {
    session::Session::connect().ok()?.finished()
}

/// Follows the boot until the manager reports it finished. Runs on the
/// calling thread; also returns when the receiver goes away.
pub fn follow(tx: &Sender<Message>) {
    // Before anything can switch: see the note in `root`.
    root::hold_proc();
    let mut failures = 0u32;
    loop {
        let initrd = in_initrd();
        let end = match session::Session::connect() {
            Ok(mut s) => {
                failures = 0;
                eprintln!("bento-boot: connected to PID 1 ({})", if initrd { "initrd" } else { "real root" });
                let stage = if initrd { Stage::Initrd } else { Stage::Main };
                if tx.send(Message::Boot(Event::Stage(stage))).is_err() {
                    return;
                }
                Some(s.run(tx))
            }
            Err(e) => {
                failures += 1;
                // Right after the switch the new manager takes a few
                // seconds to listen again; only a longer silence is news.
                if failures == 50 || failures.is_multiple_of(100) {
                    let _ = tx.send(Message::Unreachable(e.to_string()));
                }
                if failures > 600 {
                    return; // a minute of nothing
                }
                None
            }
        };
        match end {
            Some(session::End::ReceiverGone) => return,
            Some(session::End::Finished) => return,
            // The initrd's manager went away: it is switching to the real
            // root. Follow it there.
            Some(session::End::Disconnected) if initrd => {
                if tx.send(Message::Boot(Event::Stage(Stage::SwitchingRoot))).is_err() {
                    return;
                }
                if let Err(e) = wait_for_new_root(Duration::from_secs(300)) {
                    let _ = tx.send(Message::Unreachable(format!("switching root: {e}")));
                    return;
                }
                eprintln!("bento-boot: followed PID 1 into the real root");
            }
            // A re-exec of the manager, or not up yet: try again shortly.
            _ => thread::sleep(Duration::from_millis(100)),
        }
    }
}
