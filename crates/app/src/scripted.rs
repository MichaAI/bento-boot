//! A made-up boot for `bento-boot preview`: an initrd that asks for a disk
//! passphrase (a wrong one first), the switch to the real root, a slow
//! network wait, one unit that breaks and one that fails because of it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use bento_boot_model::{Event, JobKind, JobState, Stage, UnitFacts, Usec};
use bento_boot_secret::agent::{Change, Request};
use bento_boot_systemd::{Cause, Message};

use crate::controller::Asker;

pub enum Step {
    Msg(Message),
    Ask(Request),
    AskGone(PathBuf),
    Key(&'static str),
}

/// Password requests the script hands out, and what was answered.
#[derive(Clone, Default)]
pub struct ScriptedAsker {
    pending: Rc<RefCell<VecDeque<Change>>>,
    pub answers: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl ScriptedAsker {
    pub fn push(&self, c: Change) {
        self.pending.borrow_mut().push_back(c);
    }
}

impl Asker for ScriptedAsker {
    fn scan(&mut self) -> Vec<Change> {
        self.pending.borrow_mut().drain(..).collect()
    }

    fn send(&mut self, _req: &Request, datagram: &[u8]) -> io::Result<()> {
        self.answers.borrow_mut().push(datagram.to_vec());
        Ok(())
    }
}

struct Script {
    steps: Vec<(Usec, Step)>,
    next_id: u32,
}

const S: Usec = 1_000_000;

impl Script {
    fn at(&mut self, t: f64, step: Step) {
        self.steps.push(((t * S as f64) as Usec, step));
    }

    fn ev(&mut self, t: f64, e: Event) {
        self.at(t, Step::Msg(Message::Boot(e)));
    }

    /// A start job for `unit`, ordered after `after`.
    fn job(&mut self, t: f64, unit: &str, desc: &str, after: &[&str], timeout_s: u64) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.ev(t, Event::JobNew { id, unit: unit.into(), kind: JobKind::Start, state: JobState::Waiting });
        let facts = UnitFacts {
            description: desc.into(),
            after: after.iter().map(|s| (*s).to_owned()).collect(),
            before: Vec::new(),
            timeout: timeout_s * S,
        };
        self.ev(t, Event::UnitFacts { unit: unit.into(), facts });
        id
    }

    fn run(&mut self, t: f64, id: u32, unit: &str) {
        self.ev(t, Event::JobState { id, state: JobState::Running });
        let at = (t * S as f64) as Usec;
        self.ev(
            t,
            Event::UnitState {
                unit: unit.into(),
                active_state: "activating".into(),
                started_at: Some(at),
                finished_at: None,
            },
        );
    }

    fn done(&mut self, t: f64, id: u32, unit: &str) {
        let at = (t * S as f64) as Usec;
        self.ev(
            t,
            Event::UnitState {
                unit: unit.into(),
                active_state: "active".into(),
                started_at: None,
                finished_at: Some(at),
            },
        );
        self.ev(t, Event::JobRemoved { id, unit: unit.into(), result: "done".into() });
    }

    /// Runs from `start` to `end`.
    fn span(&mut self, start: f64, end: f64, id: u32, unit: &str) {
        self.run(start, id, unit);
        self.done(end, id, unit);
    }
}

fn ask(file: &str) -> Request {
    Request {
        file: PathBuf::from(format!("/run/systemd/ask-password/{file}")),
        socket: PathBuf::from("/run/systemd/ask-password/sck.preview"),
        message: "Пароль от диска root (cryptroot)".into(),
        id: "cryptsetup:/dev/disk/by-uuid/c0ffee".into(),
        pid: None,
        not_after: None,
        silent: false,
        echo: false,
    }
}

/// What the journal drawer shows in the preview.
#[must_use]
pub fn drawer_lines() -> Vec<String> {
    [
        "kernel: nvme nvme0: 16/0/0 default/read/poll queues",
        "kernel: EXT4-fs (dm-0): mounted filesystem with ordered data mode",
        "systemd: Switching root.",
        "systemd-journald: Journal started",
        "kernel: amdgpu 0000:03:00.0: [drm] fb0: amdgpudrmfb frame buffer device",
        "systemd-tmpfiles: Created /var/lib/docker",
        "dbus-daemon: [system] Successfully activated service 'org.freedesktop.systemd1'",
        "NetworkManager: <info>  [1763370011.4432] manager: NetworkManager state is now CONNECTING",
        "backup-check: ping nas.lan: 3 packets sent, 0 received",
        "backup-check: mount.nfs: Connection timed out",
        "systemd: backup-check.service: Main process exited, code=exited, status=2/INVALIDARGUMENT",
        "containerd: loading plugin \"io.containerd.snapshotter.v1.overlayfs\"",
        "NetworkManager: <info>  [1763370012.0021] device (enp5s0): state change: config -> ip-config",
        "kernel: r8169 0000:05:00.0 enp5s0: Link is Up - 1Gbps/Full - flow control rx/tx",
        "NetworkManager: <info>  [1763370012.9113] dhcp4 (enp5s0): state changed new lease, address=192.168.1.42",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The whole scenario, sorted by time.
#[must_use]
pub fn boot() -> Vec<(Usec, Step)> {
    let mut s = Script { steps: Vec::new(), next_id: 0 };

    // ---- initrd
    s.ev(0.0, Event::Stage(Stage::Initrd));
    s.ev(0.0, Event::DefaultTarget("initrd.target".into()));
    let initrd = s.job(
        0.0,
        "initrd.target",
        "Initrd Default Target",
        &["initrd-fs.target", "initrd-root-fs.target", "basic.target"],
        0,
    );
    let basic = s.job(0.0, "basic.target", "Basic System", &["sysinit.target"], 0);
    let sysinit = s.job(
        0.0,
        "sysinit.target",
        "System Initialization",
        &["systemd-udev-trigger.service", "systemd-modules-load.service", "cryptsetup.target"],
        0,
    );
    let trigger = s.job(0.0, "systemd-udev-trigger.service", "Coldplug All udev Devices", &[], 90);
    let modules = s.job(0.0, "systemd-modules-load.service", "Load Kernel Modules", &[], 90);
    let crypt_t =
        s.job(0.0, "cryptsetup.target", "Local Encrypted Volumes", &["systemd-cryptsetup@cryptroot.service"], 0);
    let crypt = s.job(
        0.05,
        "systemd-cryptsetup@cryptroot.service",
        "Cryptography Setup for cryptroot",
        &["dev-disk-by\\x2duuid-c0ffee.device"],
        0,
    );
    let disk = s.job(0.05, "dev-disk-by\\x2duuid-c0ffee.device", "Samsung SSD 990 PRO 2TB", &[], 90);
    let rootfs = s.job(0.1, "initrd-root-fs.target", "Initrd Root File System", &["sysroot.mount"], 0);
    let sysroot = s.job(0.1, "sysroot.mount", "/sysroot", &["systemd-fsck-root.service", "cryptsetup.target"], 90);
    let fsck = s.job(0.1, "systemd-fsck-root.service", "File System Check on Root Device", &["cryptsetup.target"], 0);
    let fs = s.job(0.1, "initrd-fs.target", "Initrd File Systems", &["initrd-root-fs.target"], 0);

    s.span(0.15, 0.9, modules, "systemd-modules-load.service");
    s.span(0.2, 1.3, trigger, "systemd-udev-trigger.service");
    s.span(0.3, 1.1, disk, "dev-disk-by\\x2duuid-c0ffee.device");
    s.run(1.2, crypt, "systemd-cryptsetup@cryptroot.service");

    // The disk asks; first a wrong passphrase, then the right one.
    s.at(1.6, Step::Ask(ask("ask.one")));
    for (i, k) in ["h", "u", "n", "t", "e", "r", "2"].iter().enumerate() {
        s.at(2.2 + i as f64 * 0.12, Step::Key(k));
    }
    s.at(3.2, Step::Key("\n"));
    s.at(3.7, Step::AskGone(PathBuf::from("/run/systemd/ask-password/ask.one")));
    s.at(3.75, Step::Ask(ask("ask.two")));
    for (i, k) in ["к", "о", "р", "о", "б", "к", "а", "🦊"].iter().enumerate() {
        s.at(4.3 + i as f64 * 0.11, Step::Key(k));
    }
    s.at(5.3, Step::Key("\n"));
    s.at(5.6, Step::AskGone(PathBuf::from("/run/systemd/ask-password/ask.two")));

    s.done(5.8, crypt, "systemd-cryptsetup@cryptroot.service");
    s.done(5.8, crypt_t, "cryptsetup.target");
    s.done(5.9, sysinit, "sysinit.target");
    s.done(5.95, basic, "basic.target");
    s.span(5.9, 6.6, fsck, "systemd-fsck-root.service");
    s.span(6.6, 6.9, sysroot, "sysroot.mount");
    s.done(6.95, rootfs, "initrd-root-fs.target");
    s.done(7.0, fs, "initrd-fs.target");
    s.done(7.05, initrd, "initrd.target");

    // ---- the real root
    s.ev(7.2, Event::Stage(Stage::SwitchingRoot));
    s.ev(7.8, Event::Stage(Stage::Main));
    s.ev(7.8, Event::DefaultTarget("graphical.target".into()));
    let t = 7.8;
    let graphical =
        s.job(t, "graphical.target", "Graphical Interface", &["multi-user.target", "display-manager.service"], 0);
    let multi = s.job(
        t,
        "multi-user.target",
        "Multi-User System",
        &["network-online.target", "docker.service", "sshd.service", "backup.service", "basic.target"],
        0,
    );
    let dm = s.job(t, "display-manager.service", "SDDM", &["systemd-user-sessions.service", "basic.target"], 0);
    let sessions =
        s.job(t, "systemd-user-sessions.service", "Permit User Sessions", &["remote-fs.target", "basic.target"], 90);
    let remote = s.job(t, "remote-fs.target", "Remote File Systems", &["mnt-nas.mount"], 0);
    let nas = s.job(t, "mnt-nas.mount", "/mnt/nas", &["network-online.target"], 90);
    let online = s.job(t, "network-online.target", "Network is Online", &["NetworkManager-wait-online.service"], 0);
    let wait =
        s.job(t, "NetworkManager-wait-online.service", "Network Manager Wait Online", &["NetworkManager.service"], 60);
    let nm = s.job(t, "NetworkManager.service", "Network Manager", &["basic.target", "dbus.service"], 90);
    let basic2 = s.job(t, "basic.target", "Basic System", &["sysinit.target", "sockets.target"], 0);
    let sysinit2 =
        s.job(t, "sysinit.target", "System Initialization", &["systemd-tmpfiles-setup.service", "local-fs.target"], 0);
    let tmpfiles =
        s.job(t, "systemd-tmpfiles-setup.service", "Create System Files and Directories", &["local-fs.target"], 90);
    let localfs = s.job(t, "local-fs.target", "Local File Systems", &["boot.mount", "home.mount"], 0);
    let boot_m = s.job(t, "boot.mount", "/boot", &[], 90);
    let home = s.job(t, "home.mount", "/home", &[], 90);
    let sockets = s.job(t, "sockets.target", "Socket Units", &["dbus.socket"], 0);
    let dbus_s = s.job(t, "dbus.socket", "D-Bus System Message Bus Socket", &[], 0);
    let dbus = s.job(t, "dbus.service", "D-Bus System Message Bus", &["dbus.socket", "basic.target"], 90);
    let docker = s.job(
        t,
        "docker.service",
        "Docker Application Container Engine",
        &["network-online.target", "containerd.service"],
        90,
    );
    let containerd = s.job(t, "containerd.service", "containerd container runtime", &["local-fs.target"], 90);
    let sshd = s.job(t, "sshd.service", "SSH Daemon", &["network.target", "basic.target"], 90);
    let backup = s.job(t, "backup.service", "Nightly backup check", &["backup-check.service", "mnt-nas.mount"], 0);
    let check = s.job(t, "backup-check.service", "Backup destination check", &["basic.target"], 90);

    s.span(8.0, 8.3, boot_m, "boot.mount");
    s.span(8.0, 8.5, home, "home.mount");
    s.done(8.55, localfs, "local-fs.target");
    s.span(8.6, 9.4, tmpfiles, "systemd-tmpfiles-setup.service");
    s.done(9.45, sysinit2, "sysinit.target");
    s.done(8.2, dbus_s, "dbus.socket");
    s.done(8.25, sockets, "sockets.target");
    s.done(9.5, basic2, "basic.target");
    s.span(9.55, 9.9, dbus, "dbus.service");
    s.span(8.6, 11.8, containerd, "containerd.service");
    s.span(9.95, 10.9, nm, "NetworkManager.service");
    s.span(9.6, 10.2, sshd, "sshd.service");
    s.run(10.95, wait, "NetworkManager-wait-online.service");
    s.run(9.6, check, "backup-check.service");
    s.ev(
        11.4,
        Event::UnitState {
            unit: "backup-check.service".into(),
            active_state: "failed".into(),
            started_at: None,
            finished_at: None,
        },
    );
    s.ev(11.4, Event::JobRemoved { id: check, unit: "backup-check.service".into(), result: "failed".into() });
    s.at(
        11.45,
        Step::Msg(Message::Cause {
            unit: "backup-check.service".into(),
            cause: Cause {
                result: "exit-code".into(), code: 1, status: 2, status_text: "NAS не отвечает".into()
            },
        }),
    );
    s.at(
        11.7,
        Step::Msg(Message::Journal {
            unit: "backup-check.service".into(),
            lines: vec![
                "checking nas.lan:/backups".into(),
                "ping nas.lan: 3 packets sent, 0 received".into(),
                "mount.nfs: Connection timed out".into(),
                "destination unreachable, giving up".into(),
            ],
        }),
    );
    // A peek at the journal drawer.
    s.at(12.3, Step::Key("\t"));
    s.at(13.9, Step::Key("\t"));
    s.ev(11.5, Event::JobRemoved { id: backup, unit: "backup.service".into(), result: "dependency".into() });
    s.done(14.2, wait, "NetworkManager-wait-online.service");
    s.done(14.25, online, "network-online.target");
    s.span(14.3, 14.9, nas, "mnt-nas.mount");
    s.done(14.95, remote, "remote-fs.target");
    s.span(15.0, 15.2, sessions, "systemd-user-sessions.service");
    s.span(12.0, 15.6, docker, "docker.service");
    s.done(15.65, multi, "multi-user.target");
    s.span(15.3, 15.9, dm, "display-manager.service");
    s.done(16.0, graphical, "graphical.target");
    s.ev(16.0, Event::StartupFinished);

    s.steps.sort_by_key(|(t, _)| *t);
    s.steps
}
