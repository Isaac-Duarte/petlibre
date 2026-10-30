use super::*;
use plans::{ENTRIES_LEN, StockPlan, StockPlanState};
use wellfed_model::Weekdays;

const BUILD: &str = "ab12000000000000000000000000000000000000000000000000000000000000";
const OTHER: &str = "cd34000000000000000000000000000000000000000000000000000000000000";
/// The signed manifest's version the runs below promote to.
const VERSION: u32 = 9;

fn frame() -> SetupFrame {
    SetupFrame {
        ssid: "Lab".into(),
        password: "secret12".into(),
        host: "10.42.103.1".into(),
        port: 1883,
        id: "1234".into(),
    }
}

fn enrolled() -> FeederTrust {
    FeederTrust::Enrolled {
        device: "AF_NEW".into(),
        address: "192.168.4.20".into(),
    }
}

fn ready() -> StockView {
    StockView {
        online: true,
        version: Some(STOCK_VERSION.into()),
        power: Some(StockPower::Adapter),
        trust: enrolled(),
    }
}

/// The feeder's floor when the runs below read it: below [`VERSION`].
const FLOOR: u32 = 3;

/// A bench review's run: the LAN recovery window, the bench key built in,
/// and a manifest signed before the run.
fn bench() -> Options {
    Options {
        route: PairRoute::Recovery,
        update_key: UpdateKey::BuiltIn,
        promotion: Promotion::Signed(VERSION),
    }
}

fn fresh() -> Install {
    Install::new(Provision::Done, BUILD, bench()).unwrap()
}

/// A run carried to `phase`, with each earlier step answered as it should be.
fn run_to(phase: Phase) -> Install {
    let mut install = fresh();
    let steps = [
        Phase::ConfirmFeeder,
        Phase::Request,
        Phase::AwaitInstall,
        Phase::Pair,
        Phase::ReadStatus,
        Phase::CloseTrial,
        Phase::Backup,
        Phase::StockPlans,
        Phase::Sign,
        Phase::Promote,
        Phase::AwaitConfirm,
    ];
    for at in steps {
        if install.phase() == phase {
            break;
        }
        match at {
            Phase::ConfirmFeeder => install.stock(&ready()).unwrap(),
            Phase::Request => {
                assert_eq!(install.step(), Step::ConfirmFeeder(enrolled()));
                install.feeder_confirmed();
            }
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
            Phase::StockPlans => {
                install.backup(VerifiedBackup::for_tests());
                assert_eq!(install.step(), Step::ReadStockPlans);
            }
            Phase::Sign => {
                let (count, entries) = stock_sectors(&[Some(breakfast())]);
                install.stock_plans(count, &entries, NOW);
                assert_eq!(install.step(), Step::ReadFloor);
            }
            Phase::Promote => {
                install.floor(FLOOR);
                assert_eq!(install.step(), Step::Promote);
            }
            _ => install.promoted(Ok(())),
        }
    }
    assert_eq!(install.phase(), phase);
    install
}

/// 2026-10-28T12:00:00Z.
const NOW: u64 = 1_792_843_200_000;

/// Stock's daily 07:30 plan of 2 portions.
fn breakfast() -> StockPlan {
    StockPlan {
        id: 101,
        hour: 7,
        minute: 30,
        once: false,
        days: [1, 2, 3, 4, 5, 6, 7],
        call: false,
        call_times: 0,
        portions: 2,
        state: StockPlanState::Idle,
        sync_ms: 0,
        skip_until_ms: 0,
    }
}

/// The count byte and entries the bank-0 role serves for these plans.
fn stock_sectors(plans: &[Option<StockPlan>]) -> (u8, Vec<u8>) {
    let mut stock = StockPlans::default();
    stock.slots[..plans.len()].copy_from_slice(plans);
    let sectors = stock.encode();
    (sectors[0], sectors[0x1000..0x1000 + ENTRIES_LEN].to_vec())
}

/// The replacement's plans as `GetPlans` answers them.
fn view(plans: [Option<Plan>; PLANS], writable: bool) -> Plans {
    Plans {
        generation: Generation(4),
        writable,
        plans,
    }
}

/// A run whose bank 1 just confirmed, with stock's breakfast imported.
fn confirmed() -> Install {
    let mut install = run_to(Phase::AwaitConfirm);
    install.confirmed(
        Replacement {
            build: BUILD,
            bank: 1,
        },
        VERSION,
        None,
    );
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

    // A run with a bad frame never starts.
    let mut bad = frame();
    bad.id = "0".into();
    assert_eq!(
        Install::new(Provision::Frame(bad), BUILD, bench()).err(),
        Some(FrameError::Id)
    );
}

#[test]
fn the_passphrase_never_reaches_a_debug_line() {
    let mut f = frame();
    f.password = "hunter2-secret".into();
    assert!(!format!("{f:?}").contains("hunter2"));

    let mut install = Install::new(Provision::Frame(f), BUILD, bench()).unwrap();
    assert!(!format!("{install:?}").contains("hunter2"));
    assert_eq!(install.step(), Step::ConfirmReset);
    assert!(!format!("{install:?}").contains("104, 117, 110"));
    install.reset_accepted();
    let step = install.step();
    assert!(!format!("{step:?}").contains("hunter2"));
    let Step::SendFrame(bytes) = step else {
        panic!("the frame goes first");
    };
    assert!(bytes.bytes().windows(7).any(|window| window == b"hunter2"));
    // Debug prints bytes as numbers, so look for those too.
    assert!(!format!("{install:?}").contains("104, 117, 110"));
}

#[test]
fn a_whole_run_goes_from_the_frame_to_a_confirmed_replacement() {
    let mut install = Install::new(Provision::Frame(frame()), BUILD, bench()).unwrap();

    // The Reset hold erases stock's plans: the owner accepts that first.
    assert_eq!(install.step(), Step::ConfirmReset);
    install.frame_sent();
    assert_eq!(install.step(), Step::ConfirmReset, "no frame before it");
    install.reset_accepted();
    let Step::SendFrame(bytes) = install.step() else {
        panic!("the frame goes first");
    };
    assert_eq!(bytes.bytes(), frame().encode().unwrap());
    assert_eq!(install.step(), Step::AwaitStock, "the frame goes once");
    install.frame_sent();

    install.stock(&ready()).unwrap();
    assert_eq!(install.step(), Step::ConfirmFeeder(enrolled()));
    install.feeder_confirmed();
    assert_eq!(install.step(), Step::RequestInstall);
    install.requested(Requested::Sent);
    assert_eq!(install.step(), Step::AwaitInstall);
    install.stock_progress(40);
    assert_eq!(install.progress(), Some(40));
    install.stock_gone();

    assert_eq!(install.step(), Step::Pair(PairRoute::Recovery));
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
    assert_eq!(install.step(), Step::ReadStockPlans);
    let (count, entries) = stock_sectors(&[Some(breakfast())]);
    install.stock_plans(count, &entries, NOW);
    let import = install.import().expect("stock's plans were read");
    assert_eq!(import.plans.iter().flatten().count(), 1);
    assert_eq!(install.step(), Step::ReadFloor);
    install.floor(FLOOR);
    assert_eq!(install.step(), Step::Promote);
    install.promoted(Ok(()));
    assert_eq!(install.step(), Step::AwaitConfirm);

    // The trial boot answers before it has confirmed: wait.
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };
    install.confirmed(running, VERSION - 1, None);
    assert_eq!(install.step(), Step::AwaitConfirm);
    install.confirmed(running, VERSION, None);

    // Stock's plans go onto the confirmed replacement's empty table.
    assert_eq!(install.step(), Step::ReadPlans);
    install.plans(&view([None; PLANS], true));
    let imported = install.import().unwrap().plans;
    assert_eq!(
        install.step(),
        Step::PutPlans {
            base: Generation(4),
            plans: imported
        }
    );
    install.plans_put(PlansPut::Done);
    assert_eq!(install.step(), Step::Done);
}

fn dns() -> DnsOverride {
    DnsOverride::new("broker.example.net", "10.42.103.1:1883".parse().unwrap()).unwrap()
}

/// DNS first: the owner points stock's broker name at the installer and
/// restarts the feeder; no frame, no Reset hold, and the run then waits
/// for stock at the broker like any other.
#[test]
fn the_dns_route_sends_no_frame_and_waits_for_stock() {
    let mut install = Install::new(Provision::Dns(dns()), BUILD, bench()).unwrap();
    assert_eq!(install.phase(), Phase::Provision);
    assert_eq!(install.step(), Step::PointDns(dns()));

    // Nothing moves it on but the owner's word.
    install.stock(&ready()).unwrap();
    install.frame_sent();
    install.reset_accepted();
    assert_eq!(install.step(), Step::PointDns(dns()));

    install.dns_pointed();
    assert_eq!(install.step(), Step::AwaitStock);
    assert_eq!(
        install.stock(&StockView {
            online: false,
            ..ready()
        }),
        Err(StockNotReady::Offline)
    );
    install.stock(&ready()).unwrap();
    assert_eq!(install.step(), Step::ConfirmFeeder(enrolled()));
}

/// Stock never reaching the broker (the override not in place, or a
/// feeder that did not restart) ends the run at provisioning, with nothing
/// sent to stock.
#[test]
fn stock_that_never_arrives_ends_the_run_with_nothing_sent() {
    let mut install = Install::new(Provision::Dns(dns()), BUILD, bench()).unwrap();
    install.dns_pointed();
    install.gave_up();
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::TimedOut(Phase::AwaitStock))
    );
}

#[test]
fn declining_the_reset_hold_ends_the_run_before_anything_is_sent() {
    let mut install = Install::new(Provision::Frame(frame()), BUILD, bench()).unwrap();
    assert_eq!(install.step(), Step::ConfirmReset);
    install.reset_declined();
    assert_eq!(install.step(), Step::Failed(InstallError::ResetDeclined));
    install.reset_accepted();
    assert_eq!(install.step(), Step::Failed(InstallError::ResetDeclined));
}

#[test]
fn stock_is_asked_once_and_only_when_ready_and_confirmed() {
    let mut install = fresh();
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
    // Until the owner recognizes the feeder, nothing is asked of it.
    for _ in 0..3 {
        assert_eq!(install.step(), Step::ConfirmFeeder(enrolled()));
        install.requested(Requested::Sent);
    }
    install.feeder_confirmed();
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
fn a_contested_or_unrecognized_enrollment_sends_nothing() {
    let contested = StockView {
        trust: FeederTrust::Contested,
        ..ready()
    };

    let mut early = fresh();
    early.stock(&contested).unwrap();
    assert_eq!(early.step(), Step::Failed(InstallError::Contested));

    // A second identity seen while the owner is still checking also ends it.
    let mut checking = run_to(Phase::ConfirmFeeder);
    checking.stock(&contested).unwrap();
    checking.feeder_confirmed();
    assert_eq!(checking.step(), Step::Failed(InstallError::Contested));

    let mut rejected = run_to(Phase::ConfirmFeeder);
    rejected.feeder_rejected();
    assert_eq!(rejected.step(), Step::Failed(InstallError::FeederRejected));
}

#[test]
fn another_stock_version_ends_the_run() {
    let mut install = fresh();
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
fn stock_s_refusal_ends_the_run_even_before_the_request_s_outcome() {
    let mut install = run_to(Phase::AwaitInstall);
    install.stock_refused(Some(0x2003));
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::StockRefused { code: Some(0x2003) })
    );
    install.stock_gone();
    assert!(matches!(install.step(), Step::Failed(_)));

    let mut asking = run_to(Phase::Request);
    assert_eq!(asking.step(), Step::RequestInstall);
    asking.stock_refused(None);
    asking.requested(Requested::Sent);
    assert_eq!(
        asking.step(),
        Step::Failed(InstallError::StockRefused { code: None })
    );
}

#[test]
fn stock_s_unset_clock_refusal_is_its_own_error() {
    let mut install = run_to(Phase::AwaitInstall);
    install.stock_refused(Some(STOCK_CLOCK_UNSET));
    assert_eq!(install.step(), Step::Failed(InstallError::StockClockUnset));
}

#[test]
fn only_the_served_build_moves_on() {
    let mut install = run_to(Phase::ReadStatus);
    install.replacement(Replacement {
        build: OTHER,
        bank: 0,
    });
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::WrongReplacement {
            reported: OTHER.into(),
            bank: 0
        })
    );
}

#[test]
fn a_run_resumed_on_bank_0_asks_stock_nothing_and_still_needs_the_backup() {
    let mut install = Install::from_bank0(&BUILD.to_ascii_uppercase(), bench());
    assert_eq!(install.step(), Step::Pair(PairRoute::Recovery));
    install.paired();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    // No request this run, so no record to close: straight to the gate.
    assert_eq!(install.step(), Step::Backup);
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::ReadStockPlans);
}

#[test]
fn a_run_resumed_after_activation_only_waits_for_the_confirmation() {
    let mut install = Install::from_bank0(BUILD, bench());
    install.paired();
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };
    install.replacement(running);
    assert_eq!(install.step(), Step::AwaitConfirm);
    install.confirmed(running, VERSION, None);
    assert_eq!(install.step(), Step::Done);
}

/// A reinstall of a build the feeder confirmed before skips the trial, so the
/// floor stays below the version: only readiness can say it is over, and a
/// trial still deciding reports `Starting`.
#[test]
fn a_reinstall_that_leaves_the_floor_behind_is_confirmed_by_its_readiness() {
    let mut install = Install::from_bank0(BUILD, bench());
    install.paired();
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };
    install.replacement(running);
    assert_eq!(install.step(), Step::AwaitConfirm);

    let starting = Some(Readiness::Blocked(wellfed_model::Blocked::Starting));
    install.confirmed(running, VERSION - 1, starting);
    assert_eq!(install.step(), Step::AwaitConfirm);
    install.confirmed(running, VERSION - 1, None);
    assert_eq!(install.step(), Step::AwaitConfirm);

    install.confirmed(running, VERSION - 1, Some(Readiness::Ready));
    assert_eq!(install.step(), Step::Done);
}

#[test]
fn promotion_is_refused_without_a_verified_backup() {
    let mut install = run_to(Phase::Backup);
    // However often the caller asks, without a backup the step stays.
    for _ in 0..3 {
        assert_eq!(install.step(), Step::Backup);
        install.promoted(Ok(()));
        assert_eq!(install.phase(), Phase::Backup);
    }

    // A backup given early (before the install began) counts as well.
    let mut early = fresh();
    early.backup(VerifiedBackup::for_tests());
    early.stock(&ready()).unwrap();
    early.feeder_confirmed();
    assert_eq!(early.step(), Step::RequestInstall);
    early.requested(Requested::Sent);
    early.stock_gone();
    early.paired();
    early.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    early.trial_closed();
    assert_eq!(early.step(), Step::ReadStockPlans);
}

#[test]
fn only_the_pinned_stock_payload_is_a_backup() {
    let error = VerifiedBackup::verify(b"not stock").unwrap_err();
    assert!(matches!(error, BackupError::NotPinned { sha256 } if sha256 != STOCK_OTA_SHA256));
    assert!(VerifiedBackup::verify(&[]).is_err());
    assert_eq!(VerifiedBackup::for_tests().version(), STOCK_VERSION);

    // The same digests the device layout, the image profile and the stock
    // review pin.
    let hex = |digest: [u8; 32]| -> String { digest.iter().map(|b| format!("{b:02x}")).collect() };
    assert_eq!(
        hex(STOCK_OTA_SHA256),
        "05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498"
    );
    assert_eq!(
        hex(STOCK_PAYLOAD_SHA256),
        "802ef6204c2dce8af8b74d78c5b58b1dc5f912c204bd0ac68e0a03a72120705a"
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
        None,
    );
    assert_eq!(install.step(), Step::Failed(InstallError::FellBack));

    let mut install = run_to(Phase::AwaitConfirm);
    install.confirmed(
        Replacement {
            build: OTHER,
            bank: 1,
        },
        VERSION,
        None,
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

    let mut done = confirmed();
    done.plans(&view([None; PLANS], true));
    assert!(matches!(done.step(), Step::PutPlans { .. }));
    done.plans_put(PlansPut::Done);
    done.gave_up();
    assert_eq!(done.step(), Step::Done, "a finished run stays finished");
}

#[test]
fn answers_out_of_turn_change_nothing() {
    let mut install = run_to(Phase::AwaitStock);
    install.paired();
    install.stock_gone();
    install.trial_closed();
    install.feeder_confirmed();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    install.requested(Requested::Sent);
    install.stock_refused(None);
    assert_eq!(install.step(), Step::AwaitStock);
}

/// A table stock's boot could not read stops the run before promotion,
/// with stock intact in bank 1, unless the owner chose to go on without
/// it; so does a feeder that does not serve the sectors.
#[test]
fn unreadable_stock_plans_stop_before_promotion() {
    let (_, entries) = stock_sectors(&[Some(breakfast())]);

    let mut install = run_to(Phase::StockPlans);
    install.stock_plans(11, &entries, NOW);
    let error = InstallError::StockPlans(DecodeError::Count { count: 11 });
    assert_eq!(install.step(), Step::Failed(error.clone()));
    install.promoted(Ok(()));
    assert_eq!(install.step(), Step::Failed(error));

    let mut torn = run_to(Phase::StockPlans);
    torn.stock_plans(0xff, &entries, NOW);
    assert_eq!(
        torn.step(),
        Step::Failed(InstallError::StockPlans(DecodeError::Torn))
    );

    let mut refused = run_to(Phase::StockPlans);
    refused.stock_plans_unread(Refusal::Unavailable);
    assert_eq!(
        refused.step(),
        Step::Failed(InstallError::StockPlansUnread(Refusal::Unavailable))
    );

    // The owner's choice: promote, and say what was left behind.
    let mut without = run_to(Phase::Backup);
    without.without_stock_plans();
    without.backup(VerifiedBackup::for_tests());
    assert_eq!(without.step(), Step::ReadStockPlans);
    without.stock_plans(0xff, &entries, NOW);
    assert_eq!(without.step(), Step::ReadFloor);
    without.floor(FLOOR);
    assert_eq!(without.step(), Step::Promote);
    assert_eq!(
        without.plans_skipped(),
        Some(&InstallError::StockPlans(DecodeError::Torn))
    );
    assert!(without.import().is_none());
    without.promoted(Ok(()));
    without.confirmed(
        Replacement {
            build: BUILD,
            bank: 1,
        },
        VERSION,
        None,
    );
    assert_eq!(without.step(), Step::Done, "nothing to apply");
}

/// Plans the owner already set on the replacement are never replaced, and
/// a table that cannot be written is not tried.
#[test]
fn plans_already_set_are_never_replaced() {
    let mut set = [None; PLANS];
    set[3] = Some(Plan {
        minute: 600,
        portions: 1,
        days: Weekdays::ONCE,
        enabled: true,
        call: 0,
    });
    let mut install = confirmed();
    assert_eq!(install.step(), Step::ReadPlans);
    install.plans(&view(set, true));
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::PlansAlreadySet { slots: vec![3] })
    );

    let mut locked = confirmed();
    locked.plans(&view([None; PLANS], false));
    assert_eq!(locked.step(), Step::Failed(InstallError::PlansNotWritable));
}

/// A run whose bank 1 just confirmed, whose earlier run sent its put against
/// the generation [`view`] reads.
fn confirmed_after_a_put() -> Install {
    let mut install = run_to(Phase::AwaitConfirm);
    install.earlier_put(Generation(4));
    install.confirmed(
        Replacement {
            build: BUILD,
            bank: 1,
        },
        VERSION,
        None,
    );
    install
}

/// The same generation is proof nothing changed the table since that put went
/// out, so it goes once more, against that generation.
#[test]
fn an_earlier_put_goes_again_once_from_an_unchanged_generation() {
    let mut install = confirmed_after_a_put();
    install.plans(&view([None; PLANS], true));
    assert!(matches!(
        install.step(),
        Step::PutPlans {
            base: Generation(4),
            ..
        }
    ));
    for _ in 0..3 {
        assert_eq!(install.step(), Step::AwaitPut);
    }
    install.plans_put(PlansPut::Done);
    assert_eq!(install.step(), Step::Done);
}

/// Anything else stops with nothing sent. An empty table at a later generation
/// is the owner having emptied it after the put landed: putting it back would
/// bring back meals the owner deleted.
#[test]
fn an_earlier_put_never_goes_again_once_the_generation_moved() {
    let mut breakfast = [None; PLANS];
    breakfast[3] = Some(Plan {
        minute: 600,
        portions: 1,
        days: Weekdays::ONCE,
        enabled: true,
        call: 0,
    });
    let emptied = Plans {
        generation: Generation(6),
        writable: true,
        plans: [None; PLANS],
    };
    let edited = Plans {
        generation: Generation(5),
        writable: true,
        plans: breakfast,
    };
    for table in [emptied, edited] {
        let mut install = confirmed_after_a_put();
        install.plans(&table);
        assert_eq!(install.step(), Step::Failed(InstallError::PlansUncertain));
    }

    let mut locked = confirmed_after_a_put();
    locked.plans(&view([None; PLANS], false));
    assert_eq!(locked.step(), Step::Failed(InstallError::PlansUncertain));

    // A record that cannot be read is no proof of anything.
    let mut unread = run_to(Phase::AwaitConfirm);
    unread.earlier_put_unread();
    unread.confirmed(
        Replacement {
            build: BUILD,
            bank: 1,
        },
        VERSION,
        None,
    );
    unread.plans(&view([None; PLANS], true));
    assert_eq!(unread.step(), Step::Failed(InstallError::PlansUncertain));

    // Not reading the plans leaves it just as unknown.
    let mut silent = confirmed_after_a_put();
    silent.gave_up();
    assert_eq!(silent.step(), Step::Failed(InstallError::PlansUncertain));
}

/// A put already settled is not sent again, at whatever generation it landed.
#[test]
fn an_earlier_put_that_landed_is_done_and_not_sent_again() {
    let mut install = confirmed_after_a_put();
    let imported = install.import().unwrap().plans;
    install.plans(&Plans {
        generation: Generation(5),
        writable: true,
        plans: imported,
    });
    assert_eq!(install.step(), Step::Done);
}

/// The first put arriving late moves the table, and the feeder refuses the
/// second by its base. That refusal says the first may have landed, not that
/// someone else changed the plans.
#[test]
fn a_put_refused_after_an_earlier_one_leaves_it_uncertain() {
    for refusal in [
        Refusal::Stale {
            current: Generation(5),
        },
        Refusal::Busy,
    ] {
        let mut install = confirmed_after_a_put();
        install.plans(&view([None; PLANS], true));
        assert!(matches!(install.step(), Step::PutPlans { .. }));
        install.plans_put(PlansPut::Refused(refusal));
        assert_eq!(install.step(), Step::Failed(InstallError::PlansUncertain));
    }
}

/// With no record, no put ever left: the first put's path, and its refusals in
/// their own words.
#[test]
fn no_earlier_put_is_the_first_puts_path() {
    let mut install = confirmed();
    install.plans(&view([None; PLANS], true));
    assert!(matches!(install.step(), Step::PutPlans { .. }));
    install.plans_put(PlansPut::Refused(Refusal::Stale {
        current: Generation(5),
    }));
    assert_eq!(
        install.step(),
        Step::Failed(InstallError::PlansStale {
            current: Generation(5)
        })
    );

    // Reported any later, it changes nothing: this run's own unanswered put is
    // judged by its read back.
    let mut late = confirmed();
    late.earlier_put(Generation(4));
    late.plans(&view([None; PLANS], true));
    assert!(matches!(late.step(), Step::PutPlans { .. }));
    late.plans_put(PlansPut::Refused(Refusal::Invalid));
    assert_eq!(
        late.step(),
        Step::Failed(InstallError::PlansRefused(Refusal::Invalid))
    );
}

/// A stale or refused put changed nothing and ends the run; an
/// unanswered one is judged by reading the plans back, never resent.
#[test]
fn the_put_goes_once_and_an_unanswered_one_is_read_back() {
    let put = || {
        let mut install = confirmed();
        install.plans(&view([None; PLANS], true));
        assert!(matches!(install.step(), Step::PutPlans { .. }));
        // Until its answer comes, the put is never asked for again.
        for _ in 0..3 {
            assert_eq!(install.step(), Step::AwaitPut);
            install.plans(&view([None; PLANS], true));
        }
        install
    };

    let mut stale = put();
    stale.plans_put(PlansPut::Refused(Refusal::Stale {
        current: Generation(5),
    }));
    assert_eq!(
        stale.step(),
        Step::Failed(InstallError::PlansStale {
            current: Generation(5)
        })
    );

    let mut refused = put();
    refused.plans_put(PlansPut::Refused(Refusal::Invalid));
    assert_eq!(
        refused.step(),
        Step::Failed(InstallError::PlansRefused(Refusal::Invalid))
    );

    // Unanswered, and the plans read back are the import: it landed.
    let mut landed = put();
    let imported = landed.import().unwrap().plans;
    landed.plans_put(PlansPut::Unanswered);
    assert_eq!(landed.step(), Step::ReadPlans);
    landed.plans(&Plans {
        generation: Generation(5),
        writable: true,
        plans: imported,
    });
    assert_eq!(landed.step(), Step::Done);

    // Unanswered, and the table is still empty (or anything else): it is
    // uncertain, and nothing is sent again.
    let mut lost = put();
    lost.plans_put(PlansPut::Unanswered);
    lost.plans(&view([None; PLANS], true));
    assert_eq!(lost.step(), Step::Failed(InstallError::PlansUncertain));
    lost.plans_put(PlansPut::Done);
    assert_eq!(lost.step(), Step::Failed(InstallError::PlansUncertain));

    // Giving up on the read back leaves it uncertain, never resent.
    let mut silent = put();
    silent.plans_put(PlansPut::Unanswered);
    silent.gave_up();
    assert_eq!(silent.step(), Step::Failed(InstallError::PlansUncertain));

    // Giving up before the put: installed, with the plans unread.
    let mut unread = confirmed();
    assert_eq!(unread.step(), Step::ReadPlans);
    unread.gave_up();
    assert_eq!(unread.step(), Step::Failed(InstallError::PlansUnread));
}

/// Stock with no plans, or only refused ones, leaves nothing to apply: the
/// run ends at the confirmation, the refusals named in the import.
#[test]
fn nothing_converted_leaves_nothing_to_apply() {
    let mut never = breakfast();
    never.days = [0; 7];
    for plans in [vec![], vec![Some(never)]] {
        let mut install = run_to(Phase::StockPlans);
        let (count, entries) = stock_sectors(&plans);
        install.stock_plans(count, &entries, NOW);
        assert_eq!(
            install.import().unwrap().refused().count(),
            plans.len(),
            "a refused plan is named, not dropped"
        );
        assert_eq!(install.step(), Step::ReadFloor);
        install.floor(FLOOR);
        assert_eq!(install.step(), Step::Promote);
        install.promoted(Ok(()));
        install.confirmed(
            Replacement {
                build: BUILD,
                bank: 1,
            },
            VERSION,
            None,
        );
        assert_eq!(install.step(), Step::Done);
    }
}

/// A run resumed after the promotion reads no sectors from bank 1: it takes
/// the copy an earlier run kept, under the same empty-table check, and a
/// table already holding the import (a put that landed unanswered) is done.
#[test]
fn a_resumed_run_applies_the_kept_copy() {
    let resumed = || {
        let mut install = Install::from_bank0(BUILD, bench());
        install.paired();
        install.replacement(Replacement {
            build: BUILD,
            bank: 1,
        });
        assert_eq!(install.step(), Step::AwaitConfirm);
        install
    };
    let (count, entries) = stock_sectors(&[Some(breakfast())]);
    let kept = plans::kept(count, &entries);
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };

    // The kept copy goes on through the usual check and one put.
    let mut install = resumed();
    install.kept_stock_plans(&kept, NOW);
    let imported = install.import().expect("the kept copy converts").plans;
    install.confirmed(running, VERSION, None);
    assert_eq!(install.step(), Step::ReadPlans);
    install.plans(&view([None; PLANS], true));
    assert_eq!(
        install.step(),
        Step::PutPlans {
            base: Generation(4),
            plans: imported
        }
    );

    // The import already there counts as applied; other plans still stop it.
    let mut landed = resumed();
    landed.kept_stock_plans(&kept, NOW);
    landed.confirmed(running, VERSION, None);
    landed.plans(&view(imported, true));
    assert_eq!(landed.step(), Step::Done);

    // Too late once the plans are being checked, and never over an import.
    let mut late = confirmed();
    let before = late.import().cloned();
    late.kept_stock_plans(&plans::kept(0, &entries), NOW);
    assert_eq!(late.import().cloned(), before);

    // A kept copy that does not decode is the reason none were applied.
    let mut torn = resumed();
    torn.kept_stock_plans(&plans::kept(0xff, &entries), NOW);
    assert!(torn.import().is_none());
    assert_eq!(
        torn.plans_skipped(),
        Some(&InstallError::StockPlans(DecodeError::Torn))
    );
    torn.confirmed(running, VERSION, None);
    assert_eq!(torn.step(), Step::Done);
}

/// The installer's update key, which a consumer image does not build in.
const UPDATE_KEY: [u8; 32] = [0x5e; 32];

/// A consumer review's run: Bluetooth setup, the installer's key enrolled,
/// and the build signed by the run above `above`.
fn consumer(above: u32) -> Options {
    Options {
        route: PairRoute::Setup,
        update_key: UpdateKey::Enroll(UPDATE_KEY),
        promotion: Promotion::Sign { above },
    }
}

/// A consumer run from stock to the replacement answering on bank 0 with
/// its stock record closed: the enrollment comes next.
fn consumer_on_bank0(above: u32) -> Install {
    let mut install = Install::new(Provision::Done, BUILD, consumer(above)).unwrap();
    install.stock(&ready()).unwrap();
    install.feeder_confirmed();
    assert_eq!(install.step(), Step::RequestInstall);
    install.requested(Requested::Sent);
    install.stock_gone();

    assert_eq!(install.step(), Step::Pair(PairRoute::Setup));
    install.set_up();
    assert_eq!(install.step(), Step::AwaitJoin);
    install.joined();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    assert_eq!(install.step(), Step::CloseTrial);
    install.trial_closed();
    install
}

/// A run resumed on bank 0 carried to its floor read, its update key
/// built in and stock's plans read.
fn resumed_to_floor(options: Options) -> Install {
    let mut install = Install::from_bank0(BUILD, options);
    install.paired();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::ReadStockPlans);
    let (count, entries) = stock_sectors(&[Some(breakfast())]);
    install.stock_plans(count, &entries, NOW);
    assert_eq!(install.step(), Step::ReadFloor);
    install
}

fn client(key: [u8; 32], role: ClientRole) -> Client {
    Client {
        key: wellfed_model::Hex(key),
        label: "installer".into(),
        role,
        you: false,
    }
}

/// The review's provisioning picks the route: a consumer image is paired
/// over Bluetooth setup and then found on the LAN once it joined; a bench
/// image through the recovery window, with no join to wait for.
#[test]
fn the_pair_route_follows_the_provisioning() {
    let mut install = Install::from_bank0(BUILD, consumer(0));
    assert_eq!(install.step(), Step::Pair(PairRoute::Setup));
    install.set_up();
    assert_eq!(install.step(), Step::AwaitJoin);
    assert_eq!(install.phase(), Phase::Join);
    // Pairing is done; only the join moves the run on.
    install.paired();
    assert_eq!(install.step(), Step::AwaitJoin);
    install.joined();
    assert_eq!(install.step(), Step::ReadStatus);

    // A key the build already knows opens a session at once on either.
    let mut known = Install::from_bank0(BUILD, consumer(0));
    known.paired();
    assert_eq!(known.step(), Step::ReadStatus);

    let mut bench = Install::from_bank0(BUILD, bench());
    assert_eq!(bench.step(), Step::Pair(PairRoute::Recovery));
    bench.set_up();
    assert_eq!(bench.step(), Step::Pair(PairRoute::Recovery));
    bench.paired();
    assert_eq!(bench.step(), Step::ReadStatus);

    // A feeder that never joins ends the run at the join.
    let mut lost = Install::from_bank0(BUILD, consumer(0));
    lost.set_up();
    lost.gave_up();
    assert_eq!(
        lost.step(),
        Step::Failed(InstallError::TimedOut(Phase::Join))
    );
}

#[test]
fn the_update_key_is_enrolled_once_before_the_backup() {
    let mut install = consumer_on_bank0(0);
    assert_eq!(install.step(), Step::EnrollKey(UPDATE_KEY));
    assert_eq!(install.phase(), Phase::UpdateKey);
    // Sent once: until its end comes, there is only waiting.
    assert_eq!(install.step(), Step::AwaitEnroll);
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::AwaitEnroll);
    install.key_enrolled(Ok(Enrolled::Now));
    assert_eq!(install.step(), Step::ReadStockPlans);

    // A bench image builds its key in: nothing is enrolled.
    let mut bench = run_to(Phase::Backup);
    assert_eq!(bench.step(), Step::Backup);
}

/// A refused or unconfirmed enrollment ends the run before the backup, the
/// floor or the promotion.
#[test]
fn a_refused_enrollment_ends_the_run_before_any_promotion() {
    for error in [
        EnrollError::Unconfirmed,
        EnrollError::Forbidden,
        EnrollError::Busy,
        EnrollError::Full,
        EnrollError::SessionKey {
            role: ClientRole::Owner,
        },
        EnrollError::ListUnanswered,
    ] {
        let mut install = consumer_on_bank0(0);
        install.step();
        install.key_enrolled(Err(error));
        let failed = Step::Failed(InstallError::UpdateKey(error));
        assert_eq!(install.step(), failed);

        install.backup(VerifiedBackup::for_tests());
        install.floor(0);
        install.signed();
        install.promoted(Ok(()));
        assert_eq!(install.step(), failed, "{error:?}");
    }
}

/// A key the feeder already lists as a signer sends nothing: the shared
/// enrollment reads the list first and ends there.
#[test]
fn an_already_enrolled_key_is_skipped() {
    use crate::update_key::{Enroll, Next};
    use wellfed_model::{Answer, Capabilities, Op};

    let mut install = consumer_on_bank0(0);
    let Step::EnrollKey(key) = install.step() else {
        panic!("the enrollment comes after the bank-0 status");
    };
    let owner = Capabilities {
        update: true,
        ..Capabilities::default()
    };
    let mut enroll = Enroll::new(key, "installer", ClientRole::Owner, owner).unwrap();
    assert_eq!(enroll.poll(), Next::Send(Op::ListClients));
    enroll.answer(&Answer::Clients(vec![client(
        key,
        ClientRole::UpdateSigner,
    )]));
    let Next::Done(enrolled) = enroll.poll() else {
        panic!("a listed key ends the enrollment");
    };
    assert_eq!(enrolled, Enrolled::Already);

    install.key_enrolled(Ok(enrolled));
    assert_eq!(install.step(), Step::Backup);
}

/// An unanswered enrollment may have enrolled the key: the signer list
/// decides, and the enrollment is never sent again.
#[test]
fn an_unanswered_enrollment_is_uncertain_and_never_resent() {
    let unanswered = || {
        let mut install = consumer_on_bank0(0);
        install.step();
        install.key_enrolled(Err(EnrollError::Uncertain));
        assert_eq!(install.step(), Step::CheckSigners);
        assert_eq!(install.step(), Step::CheckSigners);
        install
    };

    let mut landed = unanswered();
    landed.signers(&[
        client([1; 32], ClientRole::Owner),
        client(UPDATE_KEY, ClientRole::UpdateSigner),
    ]);
    assert_eq!(landed.step(), Step::Backup);

    let uncertain = Step::Failed(InstallError::UpdateKeyUncertain);
    for listed in [vec![], vec![client(UPDATE_KEY, ClientRole::Member)]] {
        let mut missing = unanswered();
        missing.signers(&listed);
        assert_eq!(missing.step(), uncertain);
    }
    let mut unread = unanswered();
    unread.signers_unread();
    assert_eq!(unread.step(), uncertain);
    let mut given_up = unanswered();
    given_up.gave_up();
    assert_eq!(given_up.step(), uncertain);

    // Nothing after it offers the key again or moves the run on.
    for mut ended in [unread, given_up] {
        ended.key_enrolled(Ok(Enrolled::Now));
        ended.signers(&[client(UPDATE_KEY, ClientRole::UpdateSigner)]);
        assert_eq!(ended.step(), uncertain);
    }
}

/// A consumer run resumed on bank 0: paired by the known key, nothing asked
/// of stock and no record to close; the key enrolled, the backup, then the
/// version signed above both the floor and what the installer signed
/// before.
#[test]
fn a_resumed_consumer_run_enrolls_signs_and_promotes() {
    let mut install = Install::from_bank0(BUILD, consumer(11));
    assert_eq!(install.version(), None);
    install.paired();
    install.replacement(Replacement {
        build: BUILD,
        bank: 0,
    });
    assert_eq!(install.step(), Step::EnrollKey(UPDATE_KEY));
    install.key_enrolled(Ok(Enrolled::Now));
    install.backup(VerifiedBackup::for_tests());
    assert_eq!(install.step(), Step::ReadStockPlans);
    let (count, entries) = stock_sectors(&[Some(breakfast())]);
    install.stock_plans(count, &entries, NOW);

    assert_eq!(install.step(), Step::ReadFloor);
    install.floor(FLOOR);
    assert_eq!(install.step(), Step::Sign { version: 12 });
    assert_eq!(install.version(), Some(12));
    assert_eq!(install.phase(), Phase::Sign);
    install.promoted(Ok(()));
    assert_eq!(install.step(), Step::Sign { version: 12 });
    install.signed();
    assert_eq!(install.step(), Step::Promote);
    install.promoted(Ok(()));

    let running = Replacement {
        build: BUILD,
        bank: 1,
    };
    install.confirmed(running, 11, None);
    assert_eq!(install.step(), Step::AwaitConfirm);
    install.confirmed(running, 12, None);
    assert_eq!(install.step(), Step::ReadPlans);
}

/// Resumed after the activation, a run that signs for itself waits for the
/// version an earlier run kept; without one it cannot tell a trial from a
/// confirmation, and stops.
#[test]
fn a_resumed_run_on_bank_1_needs_the_promoted_version() {
    let running = Replacement {
        build: BUILD,
        bank: 1,
    };

    let mut unknown = Install::from_bank0(BUILD, consumer(0));
    unknown.paired();
    unknown.replacement(running);
    assert_eq!(unknown.step(), Step::Failed(InstallError::PromotionUnknown));

    let mut kept = Install::from_bank0(BUILD, consumer(0));
    kept.paired();
    kept.promoted_before(12);
    kept.replacement(running);
    assert_eq!(kept.step(), Step::AwaitConfirm);
    kept.confirmed(running, 11, None);
    assert_eq!(kept.step(), Step::AwaitConfirm);
    kept.confirmed(running, 12, None);
    assert_eq!(kept.step(), Step::Done);

    // A manifest given to the run names its own version.
    let mut given = Install::from_bank0(BUILD, bench());
    given.promoted_before(99);
    assert_eq!(given.version(), Some(VERSION));
}

/// A manifest at or below the feeder's floor would need a downgrade
/// statement, which the installer never makes: refused before promotion.
/// A run that signs for itself goes above the floor.
#[test]
fn a_version_at_or_below_the_floor_is_refused() {
    for floor in [VERSION, VERSION + 1] {
        let mut install = run_to(Phase::Sign);
        install.floor(floor);
        let refused = Step::Failed(InstallError::NotAboveFloor {
            version: VERSION,
            floor,
        });
        assert_eq!(install.step(), refused);
        install.promoted(Ok(()));
        assert_eq!(install.step(), refused);
    }

    let signing = |above| Options {
        update_key: UpdateKey::BuiltIn,
        ..consumer(above)
    };
    let mut above_floor = resumed_to_floor(signing(2));
    above_floor.floor(7);
    assert_eq!(above_floor.step(), Step::Sign { version: 8 });
    let mut above_signed = resumed_to_floor(signing(9));
    above_signed.floor(7);
    assert_eq!(above_signed.step(), Step::Sign { version: 10 });

    let mut full = resumed_to_floor(signing(u32::MAX));
    full.floor(7);
    assert_eq!(
        full.step(),
        Step::Failed(InstallError::NoVersionAbove { floor: 7 })
    );
}
