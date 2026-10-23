use super::*;

#[test]
fn ota_takes_its_flags_and_the_global_local_bin() {
    let cli = Cli::try_parse_from([
        "petlibre-bench",
        "ota",
        "fw12-x",
        "--review",
        "private/verification/r",
        "--version",
        "14",
        "--minutes",
        "5",
        "--local-bin",
        "bin/petlibre-local",
    ])
    .unwrap();

    let Some(Command::Ota(args)) = &cli.command else {
        panic!("not ota")
    };
    assert_eq!((args.version, args.minutes), (Some(14), 5.0));
    assert!(!args.attended_motion && !args.build);
    assert_eq!(args.config, PathBuf::from("private/server.json"));
    assert_eq!(cli.local_bin, Some(PathBuf::from("bin/petlibre-local")));
}

#[test]
fn stock_ota_takes_a_name_its_firmware_and_no_promote() {
    let cli = Cli::try_parse_from([
        "petlibre-bench",
        "stock-ota",
        "poc-01",
        "--firmware",
        "ble",
        "--no-promote",
    ])
    .unwrap();
    let Some(Command::StockOta(args)) = &cli.command else {
        panic!("not stock-ota")
    };
    assert_eq!(args.name, "poc-01");
    assert!(args.no_promote && !args.power);
    assert_eq!(cli.command.as_ref().map(Command::label), Some("stock-ota"));

    assert!(Cli::try_parse_from(["petlibre-bench", "stock-ota"]).is_err());
}

#[test]
fn itest_lists_without_a_name_and_takes_its_scenarios() {
    let cli = Cli::try_parse_from(["petlibre-bench", "itest", "--list"]).unwrap();
    let Some(Command::Itest(args)) = &cli.command else {
        panic!("not itest")
    };
    assert!(args.list && args.name.is_none());

    let cli = Cli::try_parse_from([
        "petlibre-bench",
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
        &["petlibre-bench", "itest"][..],
        &[
            "petlibre-bench",
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
    Cli::try_parse_from(["petlibre-bench", "ota", "fw12-x"].iter().chain(extra))
}

#[test]
fn ota_build_takes_a_no_motion_image_and_config() {
    let cli = ota(&[
        "--build",
        "--firmware",
        "trial",
        "--variant",
        "fw11-fail",
        "--config",
        "private/other.json",
    ])
    .unwrap();

    let Some(Command::Ota(args)) = &cli.command else {
        panic!("not ota")
    };
    assert!(args.build && args.review.is_none());
    assert_eq!(args.firmware, Some(commands::Firmware::Trial));
    assert_eq!(args.variant, Some(commands::Variant::Fw11Fail));
    assert_eq!(args.config, PathBuf::from("private/other.json"));
}

#[test]
fn ota_refuses_motion_and_mixed_sources() {
    for motion in [&["--variant", "fw16-pulse"], &["--firmware", "motor"]] {
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
