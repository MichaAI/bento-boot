use super::*;

fn facts(after: &[&str], before: &[&str]) -> UnitFacts {
    UnitFacts {
        description: String::new(),
        after: after.iter().map(|s| (*s).to_owned()).collect(),
        before: before.iter().map(|s| (*s).to_owned()).collect(),
        timeout: 0,
    }
}

fn start(boot: &mut Boot, id: u32, unit: &str, after: &[&str]) {
    boot.apply(Event::JobNew { id, unit: unit.into(), kind: JobKind::Start, state: JobState::Waiting }, 0);
    boot.apply(Event::UnitFacts { unit: unit.into(), facts: facts(after, &[]) }, 0);
}

fn sample() -> Boot {
    let mut b = Boot::new();
    b.apply(Event::Stage(Stage::Main), 0);
    b.apply(Event::DefaultTarget("graphical.target".into()), 0);
    start(&mut b, 1, "graphical.target", &["multi-user.target", "display-manager.service"]);
    start(&mut b, 2, "multi-user.target", &["network-online.target", "sshd.service", "basic.target"]);
    start(&mut b, 3, "display-manager.service", &["systemd-user-sessions.service", "basic.target"]);
    start(&mut b, 4, "network-online.target", &["NetworkManager-wait-online.service"]);
    start(&mut b, 5, "NetworkManager-wait-online.service", &["NetworkManager.service"]);
    start(&mut b, 6, "sshd.service", &["network.target", "basic.target"]);
    // A job nothing above waits on.
    start(&mut b, 7, "fstrim.service", &[]);
    b.apply(Event::JobState { id: 5, state: JobState::Running }, 1);
    b
}

#[test]
fn tree_follows_after_orderings_with_jobs_only() {
    let t = sample().tree();
    assert_eq!(t.nodes[0].unit, "graphical.target");
    let names: Vec<&str> = t.nodes.iter().map(|n| n.unit.as_str()).collect();
    // basic.target, network.target and friends have no job: not on screen.
    assert!(!names.contains(&"basic.target"));
    assert!(!names.contains(&"network.target"));
    let wait = t.nodes.iter().position(|n| n.unit == "NetworkManager-wait-online.service").unwrap();
    let path: Vec<&str> = t.path_to(wait).iter().map(|&i| t.nodes[i].unit.as_str()).collect();
    assert_eq!(
        path,
        ["graphical.target", "multi-user.target", "network-online.target", "NetworkManager-wait-online.service"]
    );
    assert_eq!(t.nodes[wait].state, NodeState::Running);
    // Orphan jobs hang under the root.
    let trim = t.nodes.iter().position(|n| n.unit == "fstrim.service").unwrap();
    assert_eq!(t.nodes[trim].parent, Some(0));
}

#[test]
fn each_unit_is_placed_once_even_with_cycles() {
    let mut b = Boot::new();
    b.apply(Event::DefaultTarget("a.target".into()), 0);
    start(&mut b, 1, "a.target", &["b.service", "c.service"]);
    start(&mut b, 2, "b.service", &["c.service"]);
    start(&mut b, 3, "c.service", &["b.service", "c.service"]);
    let t = b.tree();
    assert_eq!(t.nodes.len(), 3);
    for (i, n) in t.nodes.iter().enumerate() {
        for &c in &n.children {
            assert_eq!(t.nodes[c].parent, Some(i));
            assert!(c > i, "children come after parents");
        }
    }
}

#[test]
fn stop_jobs_wait_on_before() {
    let mut b = Boot::new();
    b.apply(Event::DefaultTarget("poweroff.target".into()), 0);
    b.apply(Event::JobNew { id: 1, unit: "poweroff.target".into(), kind: JobKind::Start, state: JobState::Waiting }, 0);
    b.apply(Event::UnitFacts { unit: "poweroff.target".into(), facts: facts(&["umount.target"], &[]) }, 0);
    b.apply(Event::JobNew { id: 2, unit: "umount.target".into(), kind: JobKind::Start, state: JobState::Waiting }, 0);
    b.apply(Event::UnitFacts { unit: "umount.target".into(), facts: facts(&["home.mount"], &[]) }, 0);
    b.apply(Event::JobNew { id: 3, unit: "home.mount".into(), kind: JobKind::Stop, state: JobState::Running }, 0);
    b.apply(Event::UnitFacts { unit: "home.mount".into(), facts: facts(&[], &["umount.target"]) }, 0);
    b.apply(Event::JobNew { id: 4, unit: "app.service".into(), kind: JobKind::Stop, state: JobState::Running }, 0);
    // On stop the order turns around: home.mount waits on app.service
    // only if the mount is ordered Before= it, which it is not here.
    b.apply(Event::UnitFacts { unit: "app.service".into(), facts: facts(&["home.mount"], &[]) }, 0);
    let t = b.tree();
    let home = t.nodes.iter().position(|n| n.unit == "home.mount").unwrap();
    assert_eq!(t.nodes[t.nodes[home].parent.unwrap()].unit, "umount.target");
}

#[test]
fn failures_are_merged_and_dependency_failures_not_counted() {
    let mut b = sample();
    b.apply(Event::JobRemoved { id: 6, unit: "sshd.service".into(), result: "failed".into() }, 10);
    b.apply(
        Event::UnitState {
            unit: "sshd.service".into(),
            active_state: "failed".into(),
            started_at: None,
            finished_at: None,
        },
        11,
    );
    b.apply(Event::JobRemoved { id: 2, unit: "multi-user.target".into(), result: "dependency".into() }, 12);
    b.apply(Event::FailureReason { unit: "sshd.service".into(), reason: "exited with status 1".into() }, 13);
    assert_eq!(b.failures.len(), 2);
    assert_eq!(b.failures[0].reason, "exited with status 1");
    assert_eq!(b.counts().failed, 1);
}

#[test]
fn runs_and_milestones() {
    let mut b = sample();
    b.apply(
        Event::UnitState {
            unit: "NetworkManager.service".into(),
            active_state: "activating".into(),
            started_at: Some(100),
            finished_at: None,
        },
        100,
    );
    b.apply(
        Event::UnitState {
            unit: "NetworkManager.service".into(),
            active_state: "active".into(),
            started_at: Some(100),
            finished_at: Some(2_100),
        },
        2_100,
    );
    b.apply(Event::JobRemoved { id: 4, unit: "network-online.target".into(), result: "done".into() }, 3_000);
    assert_eq!(b.slowest(5, 0)[0].duration(), 2_000);
    // A second, shorter run of the same unit does not get its own row.
    b.apply(
        Event::UnitState {
            unit: "NetworkManager.service".into(),
            active_state: "activating".into(),
            started_at: Some(5_000),
            finished_at: None,
        },
        5_000,
    );
    b.apply(
        Event::UnitState {
            unit: "NetworkManager.service".into(),
            active_state: "active".into(),
            started_at: Some(5_000),
            finished_at: Some(5_500),
        },
        5_500,
    );
    assert_eq!(b.slowest(5, 0).len(), 1);
    assert_eq!(b.milestones, [("network-online.target".to_owned(), 3_000)]);
    assert_eq!(b.done, 1);
}

#[test]
fn switch_root_forgets_initrd_jobs_but_keeps_failures() {
    let mut b = Boot::new();
    b.apply(Event::Stage(Stage::Initrd), 0);
    b.apply(Event::DefaultTarget("initrd.target".into()), 0);
    start(&mut b, 1, "initrd.target", &["sysroot.mount"]);
    start(&mut b, 2, "sysroot.mount", &[]);
    b.apply(Event::JobRemoved { id: 9, unit: "fw.service".into(), result: "failed".into() }, 5);
    b.apply(Event::Stage(Stage::SwitchingRoot), 50);
    assert!(b.jobs.is_empty());
    assert_eq!(b.default_target, None);
    assert_eq!(b.switched_root_at, Some(50));
    assert_eq!(b.failures.len(), 1);
}

#[test]
fn orbit_layout_partitions_the_circle() {
    let t = sample().tree();
    let p = orbit::layout(&t, 0.0);
    assert_eq!(p.len(), t.nodes.len());
    for (i, n) in t.nodes.iter().enumerate() {
        if n.children.is_empty() {
            continue;
        }
        let (from, to) = p[i].span;
        let first = p[n.children[0]].span.0;
        let last = p[*n.children.last().unwrap()].span.1;
        assert!((first - from).abs() < 1e-4 && (last - to).abs() < 1e-4);
        for &c in &n.children {
            assert!(p[c].angle > from - 1e-4 && p[c].angle < to + 1e-4);
            assert_eq!(p[c].ring, n.depth + 1);
        }
    }
    assert!((orbit::ring_radius(3, 3, 100.0, 300.0) - 300.0).abs() < 1e-4);
    assert!((orbit::ring_radius(1, 3, 100.0, 300.0) - 100.0).abs() < 1e-4);
}

#[test]
fn labels_use_descriptions_for_mounts() {
    let u = Unit { facts: Some(UnitFacts { description: "/boot".into(), ..UnitFacts::default() }), ..Unit::default() };
    assert_eq!(u.label("boot.mount"), "/boot");
    assert_eq!(u.label("x.service"), "x.service");
}
