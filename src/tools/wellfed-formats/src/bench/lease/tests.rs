use std::process::{Child, Command};

use super::*;

/// A live process other than this one, to stand as another tool's holder.
struct Other(Child);

impl Other {
    fn start() -> Self {
        Self(Command::new("sleep").arg("60").spawn().unwrap())
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn holder(&self) -> Holder {
        Holder::of(self.pid(), vec!["wellfed-bench".into(), "dump".into()]).unwrap()
    }

    /// Kill and reap it, as a run killed mid-sequence ends.
    fn kill(mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for Other {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn place(root: &Path, holder: &Holder) {
    write_json(&root.join(LEASE), holder).unwrap();
}

fn on_disk(root: &Path) -> Option<Holder> {
    read(&root.join(LEASE)).unwrap()
}

#[test]
fn a_free_bench_is_taken_privately_and_freed_on_drop() {
    let root = tempfile::tempdir().unwrap();

    let lease = Lease::acquire(root.path()).unwrap();

    assert_eq!(lease.claim(), Claim::Owned);
    assert_eq!(lease.holder().pid, std::process::id());
    assert_eq!(on_disk(root.path()).as_ref(), Some(lease.holder()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.path().join(LEASE)).unwrap().permissions();
        assert_eq!(mode.mode() & 0o777, 0o600);
    }

    drop(lease);
    assert_eq!(on_disk(root.path()), None);
    assert!(inspect(root.path()).unwrap().is_none());
}

#[test]
fn a_live_holder_refuses_a_second_and_cannot_be_broken() {
    let root = tempfile::tempdir().unwrap();
    let other = Other::start();
    let holder = other.holder();
    place(root.path(), &holder);

    let refused = Lease::acquire(root.path()).unwrap_err();
    assert!(
        matches!(&refused, LeaseError::Held(h) if **h == holder),
        "{refused:?}"
    );
    assert_eq!(
        inspect(root.path()).unwrap(),
        Some((holder.clone(), Liveness::Alive))
    );

    let kept = break_stale(root.path()).unwrap_err();
    assert!(matches!(kept, LeaseError::Live(_)), "{kept:?}");
    assert_eq!(on_disk(root.path()), Some(holder));
}

#[test]
fn a_killed_holder_leaves_a_stale_lease_until_broken() {
    let root = tempfile::tempdir().unwrap();
    let other = Other::start();
    let holder = other.holder();
    place(root.path(), &holder);
    other.kill();

    let refused = Lease::acquire(root.path()).unwrap_err();
    assert!(
        matches!(&refused, LeaseError::Stale(h) if **h == holder),
        "{refused:?}"
    );
    assert_eq!(
        inspect(root.path()).unwrap(),
        Some((holder.clone(), Liveness::Gone))
    );

    assert_eq!(break_stale(root.path()).unwrap(), holder);
    assert_eq!(on_disk(root.path()), None);
    assert!(matches!(break_stale(root.path()), Err(LeaseError::Free)));
    assert_eq!(Lease::acquire(root.path()).unwrap().claim(), Claim::Owned);
}

#[test]
fn a_reused_pid_or_a_reboot_is_not_the_holder() {
    let other = Other::start();

    let mut reused = other.holder();
    reused.start_ticks += 1;
    assert_eq!(reused.liveness().unwrap(), Liveness::Gone);

    let mut rebooted = other.holder();
    rebooted.boot = "00000000-0000-0000-0000-000000000000".into();
    assert_eq!(rebooted.liveness().unwrap(), Liveness::Gone);
    assert!(!descends_from(std::process::id(), &rebooted).unwrap());
}

#[test]
fn a_holder_on_another_host_is_neither_taken_nor_broken() {
    let root = tempfile::tempdir().unwrap();
    let mut elsewhere = Holder::current().unwrap();
    elsewhere.host = "another-bench-host".into();
    place(root.path(), &elsewhere);

    assert_eq!(elsewhere.liveness().unwrap(), Liveness::Unknown);
    let refused = Lease::acquire(root.path()).unwrap_err();
    assert!(matches!(refused, LeaseError::Elsewhere(_)), "{refused:?}");
    let kept = break_stale(root.path()).unwrap_err();
    assert!(matches!(kept, LeaseError::Elsewhere(_)), "{kept:?}");
    assert_eq!(on_disk(root.path()), Some(elsewhere));
}

#[test]
fn a_descendant_runs_on_its_ancestors_lease_and_leaves_it() {
    let root = tempfile::tempdir().unwrap();
    let owned = Lease::acquire(root.path()).unwrap();

    // This process's children descend from it; it descends from none of them.
    let child = Other::start();
    assert!(descends_from(child.pid(), owned.holder()).unwrap());
    assert!(!descends_from(std::process::id(), &child.holder()).unwrap());

    // A nested acquire in the holder's own run joins it.
    let nested = Lease::acquire(root.path()).unwrap();
    assert_eq!(nested.claim(), Claim::Inherited);
    assert_eq!(nested.holder(), owned.holder());
    drop(nested);
    assert_eq!(on_disk(root.path()).as_ref(), Some(owned.holder()));

    drop(owned);
    assert_eq!(on_disk(root.path()), None);
}

#[test]
fn a_release_removes_only_its_own_lease() {
    let root = tempfile::tempdir().unwrap();
    let lease = Lease::acquire(root.path()).unwrap();

    // Broken by hand and taken by another tool meanwhile.
    let other = Other::start();
    let theirs = other.holder();
    place(root.path(), &theirs);
    drop(lease);

    assert_eq!(on_disk(root.path()), Some(theirs));
}

#[test]
fn an_unreadable_lease_is_refused_not_overwritten() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("private")).unwrap();
    fs::write(root.path().join(LEASE), b"{\"pid\": 1").unwrap();

    for error in [
        Lease::acquire(root.path()).unwrap_err(),
        break_stale(root.path()).unwrap_err(),
        inspect(root.path()).unwrap_err(),
    ] {
        assert!(matches!(error, LeaseError::Unreadable { .. }), "{error:?}");
    }
    assert_eq!(fs::read(root.path().join(LEASE)).unwrap(), b"{\"pid\": 1");
}

#[test]
fn stat_fields_count_from_the_last_parenthesis() {
    let line = "4242 (a (b) c) S 17 4242 4242 0 -1 4194560 120 0 0 0 1 2 0 0 20 0 1 0 987654 \
                1000 100 18446744073709551615";

    assert_eq!(
        Stat::parse(line),
        Some(Stat {
            state: 'S',
            ppid: 17,
            start_ticks: 987_654,
        })
    );
    assert_eq!(Stat::parse("4242 (truncated"), None);
    assert_eq!(Stat::parse("4242 (x) S 17"), None);
}

#[test]
fn the_holder_reads_as_who_and_what() {
    let mut holder = Holder::current().unwrap();
    holder.command = vec!["wellfed-bench".into(), "dump".into(), "fw-01".into()];
    holder.started = DateTime::from_timestamp(1_792_929_600, 0).unwrap();

    let text = LeaseError::Stale(Box::new(holder.clone())).to_string();

    assert!(
        text.contains(&format!(
            "pid {} (`wellfed-bench dump fw-01`, since 2026-10-25 12:00:00 UTC on {})",
            holder.pid, holder.host
        )),
        "{text}"
    );
    assert!(text.contains("--break-lease"), "{text}");
}
