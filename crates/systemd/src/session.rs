//! One connection to one manager.

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use bento_boot_model::{Event, JobKind, JobState, UnitFacts};

use crate::Message;
use crate::dbus::{self, Connection, Value};
use crate::names::{job_id_from_path, unit_from_path};

const DEST: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const JOB: &str = "org.freedesktop.systemd1.Job";
const PROPS: &str = "org.freedesktop.DBus.Properties";
const PRIVATE: &str = "/run/systemd/private";
const SYSTEM_BUS: &str = "/run/dbus/system_bus_socket";

/// Why a unit failed, as the manager tells it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cause {
    /// `Result` of the unit type: `exit-code`, `signal`, `core-dump`,
    /// `timeout`, `start-limit-hit`, `oom-kill`, `resources`, …
    pub result: String,
    /// `ExecMainCode`: 1 exited, 2 killed, 3 dumped core (`CLD_*`).
    pub code: i32,
    /// Exit status or signal number.
    pub status: i32,
    /// What the service itself last said with `sd_notify(STATUS=…)`.
    pub status_text: String,
}

pub enum End {
    /// `StartupFinished`, or the boot was already over when we came.
    Finished,
    Disconnected,
    ReceiverGone,
}

/// The interface holding `Result` for a unit type.
fn type_interface(unit: &str) -> Option<&'static str> {
    Some(match unit.rsplit_once('.')?.1 {
        "service" => "org.freedesktop.systemd1.Service",
        "mount" => "org.freedesktop.systemd1.Mount",
        "socket" => "org.freedesktop.systemd1.Socket",
        "swap" => "org.freedesktop.systemd1.Swap",
        "automount" => "org.freedesktop.systemd1.Automount",
        "timer" => "org.freedesktop.systemd1.Timer",
        "path" => "org.freedesktop.systemd1.Path",
        _ => return None,
    })
}

fn valid_timeout(t: u64) -> Option<u64> {
    (t > 0 && t != u64::MAX).then_some(t)
}

fn text(props: &Value, key: &str) -> Option<String> {
    props.get(key).and_then(Value::str).map(str::to_owned)
}

pub struct Session {
    conn: Connection,
    private: bool,
    fetched: HashSet<String>,
}

impl Session {
    pub fn connect() -> std::io::Result<Self> {
        let (conn, private) = match Connection::connect(Path::new(PRIVATE), None) {
            Ok(c) => (c, true),
            Err(_) => {
                let mut c = Connection::connect(Path::new(SYSTEM_BUS), Some(DEST))?;
                // On the bus, signals only come with a match rule.
                c.call(
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "AddMatch",
                    &["type='signal',sender='org.freedesktop.systemd1'"],
                )?;
                (c, false)
            }
        };
        Ok(Self { conn, private, fetched: HashSet::new() })
    }

    fn get_all(&mut self, path: &str, iface: &str) -> std::io::Result<Value> {
        let body = self.conn.call(path, PROPS, "GetAll", &[iface])?;
        body.into_iter().next().ok_or_else(|| std::io::Error::other("empty GetAll reply"))
    }

    fn get(&mut self, path: &str, iface: &str, prop: &str) -> std::io::Result<Value> {
        let body = self.conn.call(path, PROPS, "Get", &[iface, prop])?;
        body.into_iter().next().ok_or_else(|| std::io::Error::other("empty Get reply"))
    }

    fn unit_path(&mut self, unit: &str) -> std::io::Result<String> {
        let body = self.conn.call(PATH, MANAGER, "GetUnit", &[unit])?;
        body.first().and_then(Value::str).map(str::to_owned).ok_or_else(|| std::io::Error::other("no unit path"))
    }

    /// Whether the manager already reached its default target.
    pub fn finished(&mut self) -> Option<bool> {
        self.get(PATH, MANAGER, "FinishTimestampMonotonic").ok()?.u64().map(|n| n > 0)
    }

    pub fn run(&mut self, tx: &Sender<Message>) -> End {
        match self.start(tx) {
            Ok(Some(end)) => return end,
            Ok(None) => {}
            Err(e) => {
                eprintln!("bento-boot: cannot read manager snapshot: {e}");
                return End::Disconnected;
            }
        }
        let initrd = crate::in_initrd();
        loop {
            // Do not wait for an idle connection: boot signals can still
            // be queued when PID 1 has already switched roots.
            if initrd && crate::root::pid1_moved() {
                return End::Disconnected;
            }
            match self.conn.next_signal_within(Duration::from_secs(1)) {
                Ok(Some(m)) => {
                    if let Some(end) = self.signal(&m, tx) {
                        return end;
                    }
                }
                // systemd carries its private connections across the
                // re-exec into the real root, but no signals come on them
                // any more: notice the move by PID 1's root instead.
                Ok(None) if initrd && crate::root::pid1_moved() => return End::Disconnected,
                Ok(None) => {}
                Err(e) => {
                    eprintln!("bento-boot: manager connection lost: {e}");
                    return End::Disconnected;
                }
            }
        }
    }

    fn send(tx: &Sender<Message>, e: Event) -> Result<(), End> {
        tx.send(Message::Boot(e)).map_err(|_| End::ReceiverGone)
    }

    fn start(&mut self, tx: &Sender<Message>) -> std::io::Result<Option<End>> {
        // Private peers are subscribed by systemd automatically. Only
        // clients of the shared system bus need an explicit Subscribe.
        if !self.private {
            self.conn.call(PATH, MANAGER, "Subscribe", &[])?;
        }

        let target = self.conn.call(PATH, MANAGER, "GetDefaultTarget", &[])?;
        let target = target.first().and_then(Value::str).unwrap_or_default().to_owned();
        if Self::send(tx, Event::DefaultTarget(target.clone())).is_err() {
            return Ok(Some(End::ReceiverGone));
        }

        // a(usssoo): id, unit, type, state, job path, unit path
        let jobs = self.conn.call(PATH, MANAGER, "ListJobs", &[])?;
        let rows: Vec<Value> = jobs.first().map(|v| v.items().to_vec()).unwrap_or_default();
        eprintln!("bento-boot: manager snapshot: {} jobs", rows.len());
        // Publish every job before fetching per-unit properties. A slow
        // GetAll must not leave the UI with just the first dot of a tree.
        for row in &rows {
            let [id, unit, kind, state, _job, _unit_path] = row.items() else { continue };
            let (Some(id), Some(unit)) = (id.u32(), unit.str()) else { continue };
            let e = Event::JobNew {
                id,
                unit: unit.to_owned(),
                kind: JobKind::from_systemd(kind.str().unwrap_or_default()),
                state: JobState::from_systemd(state.str().unwrap_or_default()),
            };
            if Self::send(tx, e).is_err() {
                return Ok(Some(End::ReceiverGone));
            }
        }
        for row in rows {
            let [_id, unit, _kind, _state, _job, unit_path] = row.items() else { continue };
            if let Some(unit) = unit.str() {
                self.fetch_unit(unit, unit_path.str(), tx);
            }
        }
        self.fetch_unit(&target, None, tx);

        if !crate::in_initrd() && self.finished() == Some(true) {
            let _ = Self::send(tx, Event::StartupFinished);
            return Ok(Some(End::Finished));
        }
        Ok(None)
    }

    /// Reads a unit's orderings, description and state, once per unit.
    fn fetch_unit(&mut self, unit: &str, path: Option<&str>, tx: &Sender<Message>) {
        if !self.fetched.insert(unit.to_owned()) {
            return;
        }
        let path = match path {
            Some(p) => p.to_owned(),
            None => match self.unit_path(unit) {
                Ok(p) => p,
                Err(_) => return,
            },
        };
        let Ok(p) = self.get_all(&path, UNIT) else { return };
        let mut timeout = p.get("JobTimeoutUSec").and_then(Value::u64).and_then(valid_timeout);
        if timeout.is_none() && unit.ends_with(".service") {
            timeout = self
                .get(&path, "org.freedesktop.systemd1.Service", "TimeoutStartUSec")
                .ok()
                .and_then(|v| v.u64())
                .and_then(valid_timeout);
        }
        let facts = UnitFacts {
            description: text(&p, "Description").unwrap_or_default(),
            after: p.get("After").map(Value::strings).unwrap_or_default(),
            before: p.get("Before").map(Value::strings).unwrap_or_default(),
            timeout: timeout.unwrap_or(0),
        };
        let _ = Self::send(tx, Event::UnitFacts { unit: unit.to_owned(), facts });
        self.unit_state(unit, &p, tx);
    }

    fn unit_state(&mut self, unit: &str, p: &Value, tx: &Sender<Message>) {
        let Some(active_state) = text(p, "ActiveState") else { return };
        let failed = active_state == "failed";
        let _ = Self::send(
            tx,
            Event::UnitState {
                unit: unit.to_owned(),
                active_state,
                started_at: p.get("InactiveExitTimestampMonotonic").and_then(Value::u64),
                finished_at: p.get("ActiveEnterTimestampMonotonic").and_then(Value::u64),
            },
        );
        if failed {
            self.cause(unit, tx);
        }
    }

    fn cause(&mut self, unit: &str, tx: &Sender<Message>) {
        let Some(iface) = type_interface(unit) else { return };
        let Ok(path) = self.unit_path(unit) else { return };
        let Ok(p) = self.get_all(&path, iface) else { return };
        let cause = Cause {
            result: text(&p, "Result").unwrap_or_default(),
            code: p.get("ExecMainCode").and_then(Value::i32).unwrap_or(0),
            status: p.get("ExecMainStatus").and_then(Value::i32).unwrap_or(0),
            status_text: text(&p, "StatusText").unwrap_or_default(),
        };
        let _ = tx.send(Message::Cause { unit: unit.to_owned(), cause });

        // The unit's last words may reach the journal a moment after the
        // failure; look a few times without holding up the signals.
        let tx = tx.clone();
        let unit = unit.to_owned();
        let _ = thread::Builder::new().name("journal".into()).spawn(move || {
            let mut shown = 0;
            for delay in [150, 400, 900] {
                thread::sleep(Duration::from_millis(delay));
                let lines = crate::journal::unit_lines(&unit, 4);
                if lines.len() > shown {
                    shown = lines.len();
                    if tx.send(Message::Journal { unit: unit.clone(), lines }).is_err() {
                        return;
                    }
                }
            }
        });
    }

    fn signal(&mut self, m: &dbus::Message, tx: &Sender<Message>) -> Option<End> {
        let (Some(member), Some(path)) = (m.member.as_deref(), m.path.as_deref()) else { return None };
        let sent = match member {
            "JobNew" => {
                let [id, job, unit] = &m.body[..] else { return None };
                let (Some(id), Some(job), Some(unit)) = (id.u32(), job.str(), unit.str()) else { return None };
                let (kind, state) = match self.get_all(job, JOB) {
                    Ok(p) => (
                        JobKind::from_systemd(&text(&p, "JobType").unwrap_or_default()),
                        JobState::from_systemd(&text(&p, "State").unwrap_or_default()),
                    ),
                    Err(_) => (JobKind::Start, JobState::Waiting),
                };
                let unit = unit.to_owned();
                let r = Self::send(tx, Event::JobNew { id, unit: unit.clone(), kind, state });
                self.fetch_unit(&unit, None, tx);
                r
            }
            "JobRemoved" => {
                let [id, _job, unit, result] = &m.body[..] else { return None };
                let (Some(id), Some(unit), Some(result)) = (id.u32(), unit.str(), result.str()) else { return None };
                let broke = result == "failed" || result == "timeout";
                let unit = unit.to_owned();
                let r = Self::send(tx, Event::JobRemoved { id, unit: unit.clone(), result: result.to_owned() });
                if broke {
                    self.cause(&unit, tx);
                }
                r
            }
            "StartupFinished" => {
                // Finishing the initrd is followed by another manager;
                // keep following it instead of ending the whole feed.
                if crate::in_initrd() {
                    return None;
                }
                let _ = Self::send(tx, Event::StartupFinished);
                return Some(End::Finished);
            }
            "PropertiesChanged" if m.interface.as_deref() == Some(PROPS) => {
                let [iface, changed, invalidated] = &m.body[..] else { return None };
                match iface.str() {
                    Some(JOB) => match (job_id_from_path(path), changed.get("State").and_then(Value::str)) {
                        (Some(id), Some(state)) => {
                            Self::send(tx, Event::JobState { id, state: JobState::from_systemd(state) })
                        }
                        _ => Ok(()),
                    },
                    Some(UNIT) => {
                        if let Some(unit) = unit_from_path(path) {
                            if changed.get("ActiveState").is_some() {
                                self.unit_state(&unit, changed, tx);
                            } else if invalidated.strings().iter().any(|k| k == "ActiveState")
                                && let Ok(p) = self.get_all(path, UNIT)
                            {
                                self.unit_state(&unit, &p, tx);
                            }
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        };
        sent.err()
    }
}
