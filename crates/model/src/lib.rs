//! Boot state as seen through systemd's manager API.
//!
//! Everything here is pure: events go in through [`Boot::apply`], and the
//! screen asks for a [`Tree`] of what is still pending and an
//! [`orbit::layout`] of that tree. No I/O, so it is tested without systemd.

pub mod orbit;
mod tree;

use std::collections::{BTreeMap, HashMap};

pub use tree::{NodeState, Tree, TreeNode};

/// Microseconds of `CLOCK_MONOTONIC`, the clock systemd's
/// `…TimestampMonotonic` properties use. It starts with the kernel.
pub type Usec = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Start,
    Stop,
    Restart,
    Reload,
    Other,
}

impl JobKind {
    /// `JobType` as systemd names it (`start`, `stop`, `verify-active`, …).
    #[must_use]
    pub fn from_systemd(s: &str) -> Self {
        match s {
            "start" | "verify-active" => Self::Start,
            "stop" => Self::Stop,
            "restart" | "try-restart" => Self::Restart,
            "reload" | "reload-or-start" => Self::Reload,
            _ => Self::Other,
        }
    }

    /// Stop jobs wait on the units ordered *before* them, every other job on
    /// the units ordered *after* them (systemd.unit(5), `Before=`/`After=`).
    #[must_use]
    pub fn is_stop(self) -> bool {
        self == Self::Stop
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Waiting,
    Running,
}

impl JobState {
    #[must_use]
    pub fn from_systemd(s: &str) -> Self {
        if s == "running" { Self::Running } else { Self::Waiting }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u32,
    pub unit: String,
    pub kind: JobKind,
    pub state: JobState,
    /// When this job was first seen, for units whose own timestamps are
    /// not known yet.
    pub seen_at: Usec,
}

/// How a unit failed, after the `result` of `JobRemoved`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    /// The unit itself broke (`failed`).
    Broke,
    /// `timeout`.
    Timeout,
    /// Only because a unit it needs failed (`dependency`).
    Dependency,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub unit: String,
    pub kind: FailKind,
    /// Human reason, filled in later from the unit's `Result` and exit
    /// status; empty until then.
    pub reason: String,
    pub at: Usec,
}

/// What the manager says about one unit. Filled lazily: a unit is first
/// seen by name in a job, its properties arrive later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitFacts {
    pub description: String,
    pub after: Vec<String>,
    pub before: Vec<String>,
    /// `JobTimeoutUSec` or, for services, the start timeout; 0 or
    /// `u64::MAX` mean none.
    pub timeout: Usec,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unit {
    pub facts: Option<UnitFacts>,
    /// `ActiveState` (`activating`, `active`, `failed`, …).
    pub active_state: String,
    /// When the current activation started: `InactiveExitTimestampMonotonic`.
    pub started_at: Option<Usec>,
}

impl Unit {
    #[must_use]
    pub fn label<'a>(&'a self, name: &'a str) -> &'a str {
        // Mounts, swaps and devices read better by what they are
        // ("/boot", "Swap Partition") than by their escaped unit names.
        let by_description = name.ends_with(".mount") || name.ends_with(".device") || name.ends_with(".swap");
        match &self.facts {
            Some(f) if by_description && !f.description.is_empty() => &f.description,
            _ => name,
        }
    }
}

/// A unit that ran and finished, for the "slowest" tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub unit: String,
    pub started: Usec,
    pub finished: Usec,
}

impl Run {
    #[must_use]
    pub fn duration(&self) -> Usec {
        self.finished.saturating_sub(self.started)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Before the first answer from PID 1.
    Connecting,
    Initrd,
    /// Between the initrd's manager going away and the real root's
    /// answering.
    SwitchingRoot,
    Main,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Stage(Stage),
    DefaultTarget(String),
    JobNew {
        id: u32,
        unit: String,
        kind: JobKind,
        state: JobState,
    },
    JobState {
        id: u32,
        state: JobState,
    },
    JobRemoved {
        id: u32,
        unit: String,
        result: String,
    },
    UnitFacts {
        unit: String,
        facts: UnitFacts,
    },
    UnitState {
        unit: String,
        active_state: String,
        started_at: Option<Usec>,
        finished_at: Option<Usec>,
    },
    /// Reason for a failure already recorded by `JobRemoved` or `UnitState`.
    FailureReason {
        unit: String,
        reason: String,
    },
    StartupFinished,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub done: u32,
    pub failed: u32,
    pub running: u32,
    pub waiting: u32,
}

#[derive(Debug)]
pub struct Boot {
    pub stage: Stage,
    pub default_target: Option<String>,
    pub jobs: BTreeMap<u32, Job>,
    pub units: HashMap<String, Unit>,
    /// Newest last.
    pub failures: Vec<Failure>,
    pub runs: Vec<Run>,
    /// Targets reached, in order, with when.
    pub milestones: Vec<(String, Usec)>,
    pub done: u32,
    /// Time of the switch from the initrd, if there was one.
    pub switched_root_at: Option<Usec>,
}

impl Default for Boot {
    fn default() -> Self {
        Self::new()
    }
}

impl Boot {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stage: Stage::Connecting,
            default_target: None,
            jobs: BTreeMap::new(),
            units: HashMap::new(),
            failures: Vec::new(),
            runs: Vec::new(),
            milestones: Vec::new(),
            done: 0,
            switched_root_at: None,
        }
    }

    pub fn apply(&mut self, event: Event, now: Usec) {
        match event {
            Event::Stage(stage) => self.set_stage(stage, now),
            Event::DefaultTarget(t) => self.default_target = Some(t),
            Event::JobNew { id, unit, kind, state } => {
                self.units.entry(unit.clone()).or_default();
                self.jobs.insert(id, Job { id, unit, kind, state, seen_at: now });
            }
            Event::JobState { id, state } => {
                if let Some(job) = self.jobs.get_mut(&id) {
                    job.state = state;
                }
            }
            Event::JobRemoved { id, unit, result } => self.job_removed(id, unit, &result, now),
            Event::UnitFacts { unit, facts } => self.units.entry(unit).or_default().facts = Some(facts),
            Event::UnitState { unit, active_state, started_at, finished_at } => {
                self.unit_state(unit, active_state, started_at, finished_at, now);
            }
            Event::FailureReason { unit, reason } => {
                if let Some(f) = self.failures.iter_mut().rev().find(|f| f.unit == unit) {
                    f.reason = reason;
                }
            }
            Event::StartupFinished => self.set_stage(Stage::Finished, now),
        }
    }

    fn set_stage(&mut self, stage: Stage, now: Usec) {
        if stage == Stage::SwitchingRoot && self.stage == Stage::Initrd {
            self.switched_root_at = Some(now);
        }
        if matches!(stage, Stage::SwitchingRoot | Stage::Main) && self.stage != stage {
            // The new manager knows nothing of the initrd's jobs; whatever
            // was still queued there is gone with it.
            self.jobs.clear();
            if stage == Stage::SwitchingRoot {
                self.default_target = None;
            }
        }
        self.stage = stage;
    }

    fn job_removed(&mut self, id: u32, unit: String, result: &str, now: Usec) {
        let job = self.jobs.remove(&id);
        let kind = match result {
            "failed" => Some(FailKind::Broke),
            "timeout" => Some(FailKind::Timeout),
            "dependency" => Some(FailKind::Dependency),
            _ => None,
        };
        match kind {
            Some(kind) => self.record_failure(&unit, kind, now),
            None if result == "done" => {
                self.done += 1;
                if unit.ends_with(".target") && job.is_some_and(|j| !j.kind.is_stop()) {
                    self.milestones.push((unit, now));
                }
            }
            None => {}
        }
    }

    fn unit_state(
        &mut self,
        unit: String,
        active_state: String,
        started_at: Option<Usec>,
        finished_at: Option<Usec>,
        now: Usec,
    ) {
        let failed = active_state == "failed";
        let entry = self.units.entry(unit.clone()).or_default();
        if let Some(s) = started_at.filter(|&s| s > 0) {
            entry.started_at = Some(s);
        }
        let was = std::mem::replace(&mut entry.active_state, active_state);
        let started = entry.started_at;
        if was == "activating"
            && entry.active_state == "active"
            && let (Some(started), Some(finished)) = (started, finished_at.filter(|&f| f > 0))
        {
            self.runs.push(Run { unit: unit.clone(), started, finished });
        }
        // A service can also crash after it started, with no failed job.
        if failed && was != "failed" {
            self.record_failure(&unit, FailKind::Broke, now);
        }
    }

    fn record_failure(&mut self, unit: &str, kind: FailKind, now: Usec) {
        // A failed job is often followed by `ActiveState=failed` on the same
        // unit; keep one entry, the more specific kind.
        if let Some(f) = self.failures.iter_mut().rev().find(|f| f.unit == unit && now.saturating_sub(f.at) < 2_000_000)
        {
            if f.kind == FailKind::Broke && kind != FailKind::Broke {
                f.kind = kind;
            }
            return;
        }
        self.failures.push(Failure { unit: unit.to_owned(), kind, reason: String::new(), at: now });
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        let running =
            u32::try_from(self.jobs.values().filter(|j| j.state == JobState::Running).count()).unwrap_or(u32::MAX);
        let total = u32::try_from(self.jobs.len()).unwrap_or(u32::MAX);
        Counts {
            done: self.done,
            failed: u32::try_from(self.failures.iter().filter(|f| f.kind != FailKind::Dependency).count())
                .unwrap_or(u32::MAX),
            running,
            waiting: total - running,
        }
    }

    /// The slowest finished runs, slowest first, none shorter than `min`.
    #[must_use]
    pub fn slowest(&self, n: usize, min: Usec) -> Vec<&Run> {
        let mut runs: Vec<&Run> = self.runs.iter().filter(|r| r.duration() >= min).collect();
        runs.sort_by(|a, b| b.duration().cmp(&a.duration()).then_with(|| a.unit.cmp(&b.unit)));
        // A unit that ran twice (restarted, or in the initrd and again
        // after the switch) shows once, with its longest run.
        let mut seen = std::collections::HashSet::new();
        runs.retain(|r| seen.insert(r.unit.as_str()));
        runs.truncate(n);
        runs
    }

    /// When the unit of a job started running, as well as we know it.
    #[must_use]
    pub fn running_since(&self, job: &Job) -> Usec {
        self.units.get(&job.unit).and_then(|u| u.started_at).unwrap_or(job.seen_at)
    }

    #[must_use]
    pub fn tree(&self) -> Tree {
        tree::build(self)
    }
}

#[cfg(test)]
mod tests;
