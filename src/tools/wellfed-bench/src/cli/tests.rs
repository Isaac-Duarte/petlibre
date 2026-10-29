use super::*;
use wellfed_formats::bench::progress::BenchCommand;
use wellfed_rig::commands;

fn job(cli: &Cli) -> &JobCommand {
    match &cli.command {
        Command::Job(command) => command,
        _ => panic!("not a job"),
    }
}

#[test]
fn ota_takes_its_flags_and_serve_its_tools() {
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
    ])
    .unwrap();

    let JobCommand::Ota(args) = job(&cli) else {
        panic!("not ota")
    };
    assert_eq!((args.version, args.minutes), (Some(14), 5.0));
    assert!(!args.attended_motion && !args.build);
    assert_eq!(args.config, PathBuf::from("private/server.json"));

    let cli = Cli::try_parse_from(["wellfed-bench", "serve", "--stock-bin", "bin/wellfed-stock"])
        .unwrap();
    let Command::Serve(args) = &cli.command else {
        panic!("not serve")
    };
    assert_eq!(args.stock_bin, Some(PathBuf::from("bin/wellfed-stock")));
    // The tools are the server's, not a job's.
    assert!(Cli::try_parse_from(["wellfed-bench", "on", "--flash-bin", "f"]).is_err());
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
    let JobCommand::StockOta(args) = job(&cli) else {
        panic!("not stock-ota")
    };
    assert_eq!(args.name, "poc-01");
    assert!(args.no_promote && !args.power);
    assert_eq!(job(&cli).label(), BenchCommand::StockOta);

    assert!(Cli::try_parse_from(["wellfed-bench", "stock-ota"]).is_err());
}

#[test]
fn itest_lists_without_a_name_and_takes_its_scenarios() {
    let cli = Cli::try_parse_from(["wellfed-bench", "itest", "--list"]).unwrap();
    let JobCommand::Itest(args) = job(&cli) else {
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
    let JobCommand::Itest(args) = job(&cli) else {
        panic!("not itest")
    };
    assert_eq!(args.only, ["requests", "network-bounce"]);
    assert!(args.build && args.power && !args.keep_going);
    assert_eq!(job(&cli).name(), Some("run1"));

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

    let JobCommand::Ota(args) = job(&cli) else {
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
    let command = vec!["wellfed-bench".into(), "serve".into()];
    let holder = lease::Holder::of(child.0.id(), command).unwrap();
    wellfed_formats::private::write_json(&root.join(lease::LEASE), &holder).unwrap();
    child
}

fn run_in(root: &Path, argv: &[&str]) -> anyhow::Result<u8> {
    let root = root.display().to_string();
    let head = ["wellfed-bench", "--root", &root];
    let words: Vec<String> = head[1..]
        .iter()
        .chain(argv)
        .map(|word| (*word).to_string())
        .collect();
    run(
        Cli::try_parse_from(head.iter().chain(argv)).unwrap(),
        &words,
    )
}

/// The lease's refusal, from `serve` or from `status`.
fn lease_error(result: anyhow::Result<u8>) -> lease::LeaseError {
    match result.unwrap_err().downcast() {
        Ok(wellfed_bench_server::ServeError::Lease(error)) => error,
        Ok(other) => panic!("not a lease refusal: {other:?}"),
        Err(error) => error.downcast().unwrap(),
    }
}

#[test]
fn with_no_server_every_hardware_command_refuses_and_names_serve() {
    let root = tempfile::tempdir().unwrap();

    for argv in [
        &["off"][..],
        &["on"],
        &["capture", "c1"],
        &["dump", "d1"],
        &["ota", "o1", "--build"],
        &["itest", "i1"],
        &["stock-ota", "s1"],
        &["attach", "1"],
    ] {
        let error = run_in(root.path(), argv).unwrap_err();
        let refused: ClientError = error.downcast().unwrap();
        assert!(
            matches!(refused, ClientError::NoServer { .. }),
            "{argv:?}: {refused:?}"
        );
        assert!(refused.to_string().contains("wellfed-bench serve"));
    }
    // Nothing took the bench.
    assert!(lease::inspect(root.path()).unwrap().is_none());
}

#[test]
fn a_bad_name_is_refused_before_the_server_is_asked() {
    let root = tempfile::tempdir().unwrap();

    let error = run_in(root.path(), &["capture", "../x"]).unwrap_err();

    let refused: wellfed_rig::Error = error.downcast().unwrap();
    assert!(
        matches!(
            refused,
            wellfed_rig::Error::Command(commands::CommandsError::BadName { .. })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_held_bench_refuses_a_server_before_it_opens_anything() {
    let root = tempfile::tempdir().unwrap();
    let holder = hold(root.path());

    // There is no bench.json either: the lease refuses first.
    let refused = lease_error(run_in(root.path(), &["serve"]));
    assert!(matches!(refused, lease::LeaseError::Held(_)), "{refused:?}");

    let kept = lease_error(run_in(root.path(), &["status", "--break-lease"]));
    assert!(matches!(kept, lease::LeaseError::Live(_)), "{kept:?}");
    assert!(lease::inspect(root.path()).unwrap().is_some());
    drop(holder);
}

#[test]
fn a_flash_client_a_killed_server_left_keeps_the_lease_from_breaking() {
    let root = tempfile::tempdir().unwrap();
    let mut holder = hold(root.path());
    holder.0.kill().unwrap();
    holder.0.wait().unwrap();
    // Stands in for the killed server's wellfed-flash: a process of that
    // name running from this checkout.
    let tools = tempfile::tempdir().unwrap();
    let flash = tools.path().join("wellfed-flash");
    std::os::unix::fs::symlink(which("sleep"), &flash).unwrap();
    let left = Other(
        std::process::Command::new(&flash)
            .arg("60")
            .current_dir(root.path())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while orphans::running(root.path()).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the stand-in never showed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let found = orphans::running(root.path());
    assert_eq!(
        (found[0].pid, found[0].name.as_str()),
        (left.0.id(), "wellfed-flash")
    );
    let error = run_in(root.path(), &["status", "--break-lease"]).unwrap_err();
    let refused: orphans::OrphanError = error.downcast().unwrap();
    assert!(
        matches!(&refused, orphans::OrphanError::FlashAlive { pid, .. } if *pid == left.0.id()),
        "{refused:?}"
    );
    assert!(lease::inspect(root.path()).unwrap().is_some());
    // A flash client of another checkout is not this bench's.
    assert!(orphans::running(tools.path()).is_empty());

    drop(left);
    assert!(run_in(root.path(), &["status", "--break-lease"]).is_err());
    assert!(lease::inspect(root.path()).unwrap().is_none());
}

/// `name` on the PATH.
fn which(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap()
}

#[test]
fn a_job_posts_its_words_without_the_root() {
    let words = |argv: &[&str]| {
        let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
        job_words(&argv)
    };

    assert_eq!(
        words(&[
            "--root",
            "/r",
            "dump",
            "d1",
            "--root=/r",
            "--off-seconds",
            "1"
        ]),
        ["dump", "d1", "--off-seconds", "1"]
    );
}

#[test]
fn a_killed_holder_is_reported_until_status_breaks_it() {
    let root = tempfile::tempdir().unwrap();
    let mut holder = hold(root.path());
    holder.0.kill().unwrap();
    holder.0.wait().unwrap();

    let refused = lease_error(run_in(root.path(), &["serve"]));
    assert!(
        matches!(refused, lease::LeaseError::Stale(_)),
        "{refused:?}"
    );

    // Breaking frees the bench; the status itself then finds no server and
    // stops on the missing bench.json, before any relay.
    assert!(run_in(root.path(), &["status", "--break-lease"]).is_err());
    assert!(lease::inspect(root.path()).unwrap().is_none());
}

#[test]
fn a_server_frees_the_lease_it_took_when_it_stops() {
    let root = tempfile::tempdir().unwrap();

    // Taken, then the missing bench.json stops the server.
    assert!(run_in(root.path(), &["serve"]).is_err());

    assert!(root.path().join("private/bench.lease.lock").is_file());
    assert!(lease::inspect(root.path()).unwrap().is_none());
}
