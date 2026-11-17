//! What waits on what, from the orderings.
//!
//! systemd.unit(5): if `a` is ordered `After=b` and both have start jobs,
//! `a` starts only once `b`'s job is done. So `a` waits on every unit in
//! its `After=` that still has a job, and for stop jobs it is the other way
//! round, `Before=`. The tree hangs each pending unit under the first unit
//! found waiting on it, breadth-first from the default target, so a unit
//! sits as close to the goal as any of its waiters.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{Boot, Job, JobState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    /// The goal itself, which may not have a job (yet).
    Goal,
    Waiting,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub unit: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub depth: u32,
    pub state: NodeState,
    pub job_id: Option<u32>,
}

/// Node 0 is the root. Children are in a stable order (by unit name), so
/// the layout does not shuffle between frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
}

impl Tree {
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// Path from the root to `idx`, root first.
    #[must_use]
    pub fn path_to(&self, mut idx: usize) -> Vec<usize> {
        let mut path = vec![idx];
        while let Some(p) = self.nodes.get(idx).and_then(|n| n.parent) {
            path.push(p);
            idx = p;
        }
        path.reverse();
        path
    }
}

pub(crate) fn build(boot: &Boot) -> Tree {
    let by_unit: HashMap<&str, &Job> = boot.jobs.values().map(|j| (j.unit.as_str(), j)).collect();

    let root_unit = boot
        .default_target
        .clone()
        .or_else(|| {
            // Before the default target is known, the goal of a shutdown is
            // the one stop-side target that has a job; otherwise any target.
            let mut targets: Vec<&str> = by_unit.keys().copied().filter(|u| u.ends_with(".target")).collect();
            targets.sort_unstable();
            targets.first().map(|s| (*s).to_owned())
        })
        .unwrap_or_default();

    let mut tree = Tree::default();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();

    let node = |unit: &str, parent: Option<usize>, depth: u32| {
        let job = by_unit.get(unit);
        TreeNode {
            unit: unit.to_owned(),
            parent,
            children: Vec::new(),
            depth,
            state: match job.map(|j| j.state) {
                Some(JobState::Running) => NodeState::Running,
                Some(JobState::Waiting) => NodeState::Waiting,
                None => NodeState::Goal,
            },
            job_id: job.map(|j| j.id),
        }
    };

    tree.nodes.push(node(&root_unit, None, 0));
    placed.insert(root_unit.as_str());
    queue.push_back(0);

    // Units with a job that nothing reachable waits on still belong on the
    // screen: they hang directly under the root once the walk is done.
    let mut seeds: Vec<&str> = by_unit.keys().copied().collect();
    seeds.sort_unstable();
    let mut seeds = seeds.into_iter();

    loop {
        while let Some(idx) = queue.pop_front() {
            let unit = tree.nodes[idx].unit.clone();
            let depth = tree.nodes[idx].depth;
            let mut deps = waits_on(boot, &by_unit, &unit);
            deps.sort_unstable();
            deps.dedup();
            for dep in deps {
                if !placed.insert(dep) {
                    continue;
                }
                let child = tree.nodes.len();
                tree.nodes.push(node(dep, Some(idx), depth + 1));
                tree.nodes[idx].children.push(child);
                queue.push_back(child);
            }
        }
        match seeds.find(|s| !placed.contains(s)) {
            Some(seed) => {
                placed.insert(seed);
                let child = tree.nodes.len();
                tree.nodes.push(node(seed, Some(0), 1));
                tree.nodes[0].children.push(child);
                queue.push_back(child);
            }
            None => break,
        }
    }
    tree
}

/// Units with a pending job that `unit` waits on.
fn waits_on<'a>(boot: &'a Boot, by_unit: &HashMap<&'a str, &'a Job>, unit: &str) -> Vec<&'a str> {
    let Some(facts) = boot.units.get(unit).and_then(|u| u.facts.as_ref()) else {
        return Vec::new();
    };
    // A goal without a job of its own is being started.
    let stop = by_unit.get(unit).is_some_and(|j| j.kind.is_stop());
    let list = if stop { &facts.before } else { &facts.after };
    list.iter()
        .filter_map(|dep| by_unit.get_key_value(dep.as_str()).map(|(k, _)| *k))
        .filter(|dep| *dep != unit)
        .collect()
}
