use super::*;

#[test]
fn ota_takes_its_flags_and_the_global_local_bin() {
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
        "--local-bin",
        "bin/wellfed-local",
    ])
    .unwrap();

    let Some(Command::Ota(args)) = &cli.command else {
        panic!("not ota")
    };
    assert_eq!((args.version, args.minutes), (Some(14), 5.0));
    assert!(!args.attended_motion && !args.build);
    assert_eq!(args.config, PathBuf::from("private/server.json"));
    assert_eq!(cli.local_bin, Some(PathBuf::from("bin/wellfed-local")));
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
    assert_eq!(cli.command.as_ref().map(Command::label), Some("stock-ota"));

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
        &[
            "wellfed-bench",
            "itest",
            "run1",
            "--build",
            "--review",
            "r",
        ],
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

fn command(argv: &[&str]) -> Command {
    let argv = std::iter::once("wellfed-bench").chain(argv.iter().copied());
    Cli::try_parse_from(argv).unwrap().command.unwrap()
}

#[test]
fn the_bench_hands_the_suite_on_and_runs_the_rest() {
    let itest_bin = Path::new("/bench/wellfed-itest");
    let built = Binary::Bench {
        itest_bin,
        itest_built: true,
    };
    let itest = command(&["itest", "run1"]);
    let stock = command(&["stock-ota", "poc-01"]);

    assert_eq!(route(built, Some(&itest), "a"), Route::HandOff);
    assert_eq!(route(built, Some(&stock), "a"), Route::HandOff);
    assert_eq!(route(built, Some(&command(&["status"])), "a"), Route::Run);
    // The page alone.
    assert_eq!(route(built, None, "a"), Route::Run);
    assert_eq!(
        route(built, Some(&command(&["itest", "--list"])), "a"),
        Route::List
    );
}

#[test]
fn a_missing_suite_binary_is_refused_naming_it_and_the_rebuild() {
    let missing = Binary::Bench {
        itest_bin: Path::new("/bench/wellfed-itest"),
        itest_built: false,
    };

    let Route::Refuse(refusal) = route(missing, Some(&command(&["itest", "run1"])), "a") else {
        panic!("not refused");
    };
    assert_eq!(
        refusal,
        Refusal::ItestMissing(PathBuf::from("/bench/wellfed-itest"))
    );
    let message = refusal.to_string();
    assert!(
        message.starts_with("/bench/wellfed-itest not found"),
        "{message}"
    );
    assert!(
        message.contains("-p wellfed-bench -p wellfed-itest"),
        "{message}"
    );

    // `--list` and the bench's own commands need no suite binary.
    assert_eq!(
        route(missing, Some(&command(&["itest", "--list"])), "a"),
        Route::List
    );
    assert_eq!(route(missing, Some(&command(&["off"])), "a"), Route::Run);
}

#[test]
fn the_suite_refuses_a_stale_hand_off_and_other_commands() {
    let itest = command(&["itest", "run1"]);
    let from = |handed| Binary::Itest { handed };

    assert_eq!(route(from(Some("a")), Some(&itest), "a"), Route::Run);
    // Run directly, not handed: its own build is the one that runs.
    assert_eq!(route(from(None), Some(&itest), "a"), Route::Run);

    let stale = route(from(Some("b")), Some(&itest), "a");
    assert_eq!(stale, Route::Refuse(Refusal::Stale));
    let Route::Refuse(refusal) = stale else {
        unreachable!()
    };
    assert!(refusal.to_string().contains("rebuild both"), "{refusal}");

    for other in [Some(command(&["status"])), None] {
        assert_eq!(
            route(from(Some("a")), other.as_ref(), "a"),
            Route::Refuse(Refusal::NotSuite)
        );
    }
}

#[test]
fn the_fingerprint_is_a_sha256() {
    assert_eq!(crate::BUILD_FINGERPRINT.len(), 64);
    assert!(
        crate::BUILD_FINGERPRINT
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    );
}
