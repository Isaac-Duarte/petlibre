use super::*;

const BUILD: &str = "ab12000000000000000000000000000000000000000000000000000000000000";
const OTHER: &str = "cd34000000000000000000000000000000000000000000000000000000000000";

fn frame() -> SetupFrame {
    SetupFrame {
        ssid: "Lab".into(),
        password: "secret12".into(),
        host: "10.42.103.1".into(),
        port: 1883,
        id: "1234".into(),
    }
}

fn ready() -> StockView {
    StockView {
        online: true,
        version: Some(STOCK_VERSION.into()),
        power: Some(StockPower::Adapter),
    }
}

/// A run carried to `step`, with each earlier step answered as it should be.
fn run_to(phase: Phase) -> Install {
    let mut install = Install::new(Provision::Done, BUILD).unwrap();
    let steps = [
        Phase::Request,
        Phase::AwaitInstall,
        Phase::Pair,
        Phase::ReadStatus,
        Phase::CloseTrial,
        Phase::Backup,
        Phase::AwaitConfirm,
    ];
    for at in steps {
        if install.phase() == phase {
            break;
        }
        match at {
            Phase::Request => install.stock(&ready()).unwrap(),
            Phase::AwaitInstall => {
                assert_eq!(install.step(), Step::RequestInstall);
                install.requested(Requested::Sent);
            }
            Phase::Pair => install.stock_gone(),
            Phase::ReadStatus => install.paired(),
            Phase::CloseTrial => install.replacement(Replacement {
                build: BUILD,
                bank: 0,
            }),
            Phase::Backup => install.trial_closed(),
            _ => {
                install.backup(VerifiedBackup::for_tests());
                assert_eq!(install.step(), Step::Promote);
                if phase != Phase::Promote {
                    install.promoted(Ok(7));
                }
            }
        }
    }
    assert_eq!(install.phase(), phase);
    install
}

#[test]
fn the_frame_is_what_stock_s_type_3_parser_reads() {
    let encoded = frame().encode().unwrap();
    let mut want = vec![b'D', b'L', 3, 0, 0, 4 + 3 + 8 + 16 + 4];
    for field in ["Lab", "secret12", "10.42.103.1:1883", "1234"] {
        want.push(u8::try_from(field.len()).unwrap());
        want.extend_from_slice(field.as_bytes());
    }
    want.push(0xff);
    assert_eq!(encoded, want);

    // The parser's own length check: byte 5 is every prefix plus string.
    let lengths: usize = [3, 8, 16, 4].iter().sum::<usize>() + 4;
    assert_eq!(usize::from(encoded[5]), lengths);
    assert_eq!(SETUP_ADDRESS.to_string(), "192.168.1.1:17484");
}

#[test]
fn values_stock_would_misread_are_refused() {
    let refused = |change: fn(&mut SetupFrame)| {
        let mut f = frame();
        change(&mut f);
        f.encode().err()
    };
    assert_eq!(refused(|f| f.ssid = String::new()), Some(FrameError::Ssid));
    assert_eq!(refused(|f| f.ssid = "x".repeat(33)), Some(FrameError::Ssid));
    assert_eq!(
        refused(|f| f.password = "x".repeat(65)),
        Some(FrameError::Password)
    );
    assert_eq!(refused(|f| f.port = 18883), Some(FrameError::Port));
    assert_eq!(refused(|f| f.port = 0), Some(FrameError::Port));
    assert_eq!(
        refused(|f| f.host = "10.0.0.1:1883".into()),
        Some(FrameError::Host)
    );
    assert_eq!(refused(|f| f.host = "h".repeat(96)), Some(FrameError::Host));
    assert_eq!(refused(|f| f.id = "12a4".into()), Some(FrameError::Id));
    assert_eq!(refused(|f| f.id = "000".into()), Some(FrameError::Id));
    assert_eq!(refused(|f| f.id = "1".repeat(31)), Some(FrameError::Id));

    // At the limits it still fits one length byte.
    let mut f = frame();
    f.ssid = "s".repeat(32);
    f.password = "p".repeat(64);
    f.host = "h".repeat(95);
    f.port = 9999;
    f.id = "9".repeat(30);
    assert_eq!(f.encode().unwrap().len(), 6 + 4 + 32 + 64 + 100 + 30 + 1);

    // The passphrase never reaches a log line.
    assert!(!format!("{f:?}").contains("pppp"));

    // A run with a bad frame never starts.
    let mut bad = frame();
    bad.id = "0".into();
    assert_eq!(
        Install::new(Provision::Frame(bad), BUILD).err(),
        Some(FrameError::Id)
    );
}

#[test]
fn a_whole_run_goes_from_the_frame_to_a_confirmed_replacement() {
    let mut install = Install::new(Provision::Frame(frame()), BUILD).unwrap();

    let Step::SendFrame(bytes) = install.step() else {
        panic!("the frame goes first");
    };
    assert_eq!(bytes, frame().encode().unwrap());
    assert_eq!(install.step(), Step::AwaitStock, "the frame goes once");
    install.frame_sent();

    install.stock(&ready()).unwrap();
    assert_eq!(install.step(), Step::RequestInstall);
    install.requested(Requested::Sent);
    assert_eq!(install.step(), Step::AwaitInstall);
    install.stock_progress(40);
    assert_eq!(install.progress(), Some(40));
    install.stock_gone();

    assert_eq!(install.step(), Step::Pair);
    install.paired();
    assert_eq!(install.step(), Step::ReadStatus);
    install.replacement(Replacement {
        build: &BUILD.to_ascii_uppercase(),
        bank: 0,
    });
    assert_eq!(install.step(), Step::CloseTrial);
    install.trial_closed();

    assert_eq!(install.step(), Step::Backup);
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::Promote);
    install.promoted(Ok(9));
    assert_eq!(install.step(), Step::AwaitConfirm);

    // The trial boot answers before it has confirmed: wait.
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };
    install.confirmed(running, 8);
    assert_eq!(install.step(), Step::AwaitConfirm);
    install.confirmed(running, 9);
    assert_eq!(install.step(), Step::Done);
}

#[test]
fn stock_is_asked_once_and_only_when_ready() {
    let mut install = Install::new(Provision::Done, BUILD).unwrap();
    let offline = StockView {
        online: false,
        ..ready()
    };
    assert_eq!(install.stock(&offline), Err(StockNotReady::Offline));
    let silent = StockView {
        version: None,
        ..ready()
    };
    assert_eq!(install.stock(&silent), Err(StockNotReady::NoVersion));
    let battery = StockView {
        power: Some(StockPower::from_type(2)),
        ..ready()
    };
    assert_eq!(install.stock(&battery), Err(StockNotReady::NotOnAdapter));
    assert_eq!(install.step(), Step::AwaitStock);

    install.stock(&ready()).unwrap();
    assert_eq!(install.step(), Step::RequestInstall);
    // Until the outcome comes, and after it, the request never comes again.
    for _ in 0..3 {
        assert_eq!(install.step(), Step::AwaitInstall);
        install.stock(&ready()).unwrap();
    }
    install.requested(Requested::Sent);
    assert_eq!(install.step(), Step::AwaitInstall);
}

#[test]
fn another_stock_version_ends_the_run() {
    let mut install = Install::new(Provision::Done, BUILD).unwrap();
    let newer = StockView {
        version: Some("1.1.16".into()),
        ..ready()
    };
    install.stock(&newer).unwrap();
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::UnsupportedVersion("1.1.16".into()))
    );
}

#[test]
fn a_refused_or_uncertain_request_ends_the_run_without_asking_again() {
    for (outcome, error) in [
        (Requested::Refused, InstallError::RequestRefused),
        (Requested::Uncertain, InstallError::RequestUncertain),
    ] {
        let mut install = run_to(Phase::Request);
        assert_eq!(install.step(), Step::RequestInstall);
        install.requested(outcome);
        assert_eq!(install.step(), Step::Failed(error.clone()));
        // Stock reporting ready again does not reopen the request.
        install.stock(&ready()).unwrap();
        assert_eq!(install.step(), Step::Failed(error));
    }
}

#[test]
fn stock_s_refusal_ends_the_run() {
    let mut install = run_to(Phase::AwaitInstall);
    install.stock_refused(Some(0x2003));
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::StockRefused { code: Some(0x2003) })
    );
    install.stock_gone();
    assert!(matches!(install.step(), Step::Failed(_)));
}

#[test]
fn only_the_served_build_on_bank_0_moves_on() {
    for (build, bank) in [(OTHER, 0), (BUILD, 1)] {
        let mut install = run_to(Phase::ReadStatus);
        install.replacement(Replacement { build, bank });
        assert_eq!(
            install.step(),
            Step::Failed(InstallError::WrongReplacement {
                reported: build.into(),
                bank
            })
        );
    }
}

#[test]
fn promotion_is_refused_without_a_verified_backup() {
    let mut install = run_to(Phase::Backup);
    // However often the caller asks, without a backup the step stays.
    for _ in 0..3 {
        assert_eq!(install.step(), Step::Backup);
        install.promoted(Ok(9));
        assert_eq!(install.phase(), Phase::Backup);
    }

    // A backup given early (before the install began) counts as well.
    let mut early = Install::new(Provision::Done, BUILD).unwrap();
    early.backup(VerifiedBackup::for_tests());
    early.stock(&ready()).unwrap();
    assert_eq!(early.step(), Step::RequestInstall);
    early.requested(Requested::Sent);
    early.stock_gone();
    early.paired();
    early.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    early.trial_closed();
    assert_eq!(early.step(), Step::Promote);
}

#[test]
fn a_run_resumed_on_bank_0_asks_stock_nothing_and_still_needs_the_backup() {
    let mut install = Install::from_bank0(&BUILD.to_ascii_uppercase());
    assert_eq!(install.step(), Step::Pair);
    install.paired();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    // No request this run, so no record to close: straight to the gate.
    assert_eq!(install.step(), Step::Backup);
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::Promote);
}

#[test]
fn only_the_pinned_stock_image_is_a_backup() {
    let error = VerifiedBackup::verify(b"not stock").unwrap_err();
    assert!(matches!(error, BackupError::NotPinned { sha256 } if sha256 != STOCK_OTA_SHA256));
    assert!(VerifiedBackup::verify(&[]).is_err());
    assert_eq!(VerifiedBackup::for_tests().version(), STOCK_VERSION);

    // The same digest the device layout and the image profile pin.
    let hex: String = STOCK_OTA_SHA256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        hex,
        "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
    );
}

#[test]
fn a_failed_promotion_or_a_fallback_ends_the_run() {
    let mut install = run_to(Phase::Promote);
    let error = UpdateError::Lost;
    install.promoted(Err(error));
    assert_eq!(install.step(), Step::Failed(InstallError::Promotion(error)));

    let mut install = run_to(Phase::AwaitConfirm);
    install.confirmed(
        Replacement {
            build: BUILD,
            bank: 0,
        },
        6,
    );
    assert_eq!(install.step(), Step::Failed(InstallError::FellBack));

    let mut install = run_to(Phase::AwaitConfirm);
    install.confirmed(
        Replacement {
            build: OTHER,
            bank: 1,
        },
        7,
    );
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::WrongPromotion {
            reported: OTHER.into()
        })
    );
}

#[test]
fn giving_up_names_the_wait() {
    let mut install = run_to(Phase::Pair);
    install.gave_up();
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::TimedOut(Phase::Pair))
    );

    let mut done = run_to(Phase::AwaitConfirm);
    done.confirmed(
        Replacement {
            build: BUILD,
            bank: 1,
        },
        7,
    );
    done.gave_up();
    assert_eq!(done.step(), Step::Done, "a finished run stays finished");
}

#[test]
fn answers_out_of_turn_change_nothing() {
    let mut install = run_to(Phase::AwaitStock);
    install.paired();
    install.stock_gone();
    install.trial_closed();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    install.requested(Requested::Sent);
    assert_eq!(install.step(), Step::AwaitStock);
}
