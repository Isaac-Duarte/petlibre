use super::*;

#[test]
fn ota_takes_its_flags_and_the_global_stock_bin() {
    let cli = Cli::try_parse_from([
        "wellfed-bench",
        "ota",
        "fw12-x",
        "--review",
        "private/verification/r",
        "--version",
        "14",
        "--minutes",
        "5",
        "--stock-bin",
        "bin/wellfed-stock",
    ])
    .unwrap();

    let Some(Command::Ota(args)) = &cli.command else {
        panic!("not ota")
    };
    assert_eq!((args.version, args.minutes), (Some(14), 5.0));
    assert!(!args.attended_motion && !args.build);
    assert_eq!(args.config, PathBuf::from("private/server.json"));
    assert_eq!(cli.stock_bin, Some(PathBuf::from("bin/wellfed-stock")));
}

#[test]
fn stock_ota_takes_a_name_its_firmware_and_no_promote() {
    let cli = Cli::try_parse_from([
        "wellfed-bench",
        "stock-ota",
        "poc-01",
        "--firmware",
        "feeder",
        "--no-promote",
    ])
    .unwrap();
    let Some(Command::StockOta(args)) = &cli.command else {
        panic!("not stock-ota")
    };
    assert_eq!(args.name, "poc-01");
    assert!(args.no_promote && !args.power);
    assert_eq!(
        cli.command.as_ref().map(Command::label),
        Some(BenchCommand::StockOta)
    );

    assert!(Cli::try_parse_from(["wellfed-bench", "stock-ota"]).is_err());
}

#[test]
fn itest_lists_without_a_name_and_takes_its_scenarios() {
    let cli = Cli::try_parse_from(["wellfed-bench", "itest", "--list"]).unwrap();
    let Some(Command::Itest(args)) = &cli.command else {
        panic!("not itest")
    };
    assert!(args.list && args.name.is_none());

    let cli = Cli::try_parse_from([
        "wellfed-bench",
        "itest",
        "run1",
        "--build",
        "--power",
        "--only",
        "requests,network-bounce",
    ])
    .unwrap();
    let Some(Command::Itest(args)) = &cli.command else {
        panic!("not itest")
    };
    assert_eq!(args.only, ["requests", "network-bounce"]);
    assert!(args.build && args.power && !args.keep_going);
    assert_eq!(cli.command.as_ref().unwrap().name(), Some("run1"));

    for argv in [
        &["wellfed-bench", "itest"][..],
        &["wellfed-bench", "itest", "run1", "--build", "--review", "r"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err(), "{argv:?}");
    }
}

fn ota(extra: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(["wellfed-bench", "ota", "fw12-x"].iter().chain(extra))
}

#[test]
fn ota_build_takes_a_no_motion_image_and_config() {
    let cli = ota(&[
        "--build",
        "--firmware",
        "trial",
        "--variant",
        "fail",
        "--config",
        "private/other.json",
    ])
    .unwrap();

    let Some(Command::Ota(args)) = &cli.command else {
        panic!("not ota")
    };
    assert!(args.build && args.review.is_none());
    assert_eq!(args.firmware, Some(commands::Firmware::Trial));
    assert_eq!(args.variant, Some(commands::Variant::Fail));
    assert_eq!(args.config, PathBuf::from("private/other.json"));
}

#[test]
fn ota_refuses_motion_and_mixed_sources() {
    for motion in [&["--variant", "pulse"], &["--firmware", "motor"]] {
        let args = [&["--build"][..], motion].concat();
        let error = ota(&args).err().unwrap().to_string();
        assert!(error.contains("motion builds stay manual"), "{error}");
    }

    // --firmware and --variant without --build are refused by the
    // command.
    for extra in [&["--build", "--review", "private/verification/r"][..], &[]] {
        assert!(ota(extra).is_err(), "{extra:?}");
    }
}

#[test]
fn every_command_but_status_takes_the_lease() {
    let parse = |argv: &[&str]| {
        let cli = Cli::try_parse_from(["wellfed-bench"].iter().chain(argv)).unwrap();
        cli.command.unwrap().lease()
    };

    assert_eq!(parse(&["status"]), LeaseUse::Report);
    assert_eq!(parse(&["status", "--break-lease"]), LeaseUse::Report);
    for argv in [
        &["off"][..],
        &["on"],
        &["capture", "c1"],
        &["dump", "d1"],
        &["ota", "o1", "--build"],
        &["itest", "i1"],
        &["stock-ota", "s1"],
    ] {
        assert_eq!(parse(argv), LeaseUse::Take, "{argv:?}");
    }
}

/// Another tool's process, killed when the test ends, passed or not.
struct Other(std::process::Child);

impl Drop for Other {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A process other than this test, standing as another tool holding the
/// bench under `root`.
fn hold(root: &Path) -> Other {
    let child = Other(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap(),
    );
    let command = vec!["wellfed-bench".into(), "on".into()];
    let holder = lease::Holder::of(child.0.id(), command).unwrap();
    wellfed_formats::private::write_json(&root.join(lease::LEASE), &holder).unwrap();
    child
}

fn run_in(root: &Path, argv: &[&str]) -> anyhow::Result<u8> {
    let root = root.display().to_string();
    let head = ["wellfed-bench", "--root", &root];
    run(
        Cli::try_parse_from(head.iter().chain(argv)).unwrap(),
        &Cancel::new(),
    )
}

fn lease_error(result: anyhow::Result<u8>) -> lease::LeaseError {
    result.unwrap_err().downcast().unwrap()
}

#[test]
fn a_held_bench_refuses_a_command_before_it_opens_anything() {
    let root = tempfile::tempdir().unwrap();
    let holder = hold(root.path());

    // There is no bench.json either: the lease refuses first.
    let refused = lease_error(run_in(root.path(), &["off"]));
    assert!(matches!(refused, lease::LeaseError::Held(_)), "{refused:?}");

    let kept = lease_error(run_in(root.path(), &["status", "--break-lease"]));
    assert!(matches!(kept, lease::LeaseError::Live(_)), "{kept:?}");
    assert!(lease::inspect(root.path()).unwrap().is_some());
    drop(holder);
}

#[test]
fn a_killed_holder_is_reported_until_status_breaks_it() {
    let root = tempfile::tempdir().unwrap();
    let mut holder = hold(root.path());
    holder.0.kill().unwrap();
    holder.0.wait().unwrap();

    let refused = lease_error(run_in(root.path(), &["on"]));
    assert!(
        matches!(refused, lease::LeaseError::Stale(_)),
        "{refused:?}"
    );

    // Breaking frees the bench; the status itself then stops on the
    // missing bench.json, before any relay.
    assert!(run_in(root.path(), &["status", "--break-lease"]).is_err());
    assert!(lease::inspect(root.path()).unwrap().is_none());
}

#[test]
fn a_command_frees_the_lease_it_took_when_it_stops() {
    let root = tempfile::tempdir().unwrap();

    // Taken, then the missing bench.json stops the command.
    assert!(run_in(root.path(), &["off"]).is_err());

    assert!(root.path().join("private/bench.lease.lock").is_file());
    assert!(lease::inspect(root.path()).unwrap().is_none());
}
