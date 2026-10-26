//! Development updates of either bank, the interrupted mode and give-ways,
//! run against the modeled helper the way the CLI runs them.

use super::*;
use crate::evidence::report::SummaryStatus;

#[test]
fn development_update_matches_the_independent_wire_and_changes_only_its_sectors() {
    for grow in [false, true] {
        let (_checkout, root) = private_root();
        let source = image(30000, 3);
        let target = variant(&source, grow);
        let old = development::payload(&source, &BANK1).unwrap().to_vec();
        let new = development::payload(&target, &BANK1).unwrap().to_vec();
        let unit = unit(&old);
        let mut port = Fake::new(unit.flash.clone());

        let report = develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "t",
        )
        .unwrap();

        assert_eq!(report.status, SummaryStatus::VerifiedViaSuppliedTransports);
        let helper = port.0.borrow();
        let length = old.len().max(new.len());
        let current = &unit.flash[START..START + length];
        assert!(helper.tx == expected_wire(&old, &new, current, START, 0x6000));

        let mut want = unit.flash.clone();
        want[START..START + new.len()].copy_from_slice(&new);
        assert!(helper.flash == want, "only the update's sectors changed");

        // The marker sector is staged first, then only changed sectors.
        assert_eq!(helper.erased[0], START);
        if !grow {
            assert_eq!(helper.erased.len(), 3);
        }

        let receipt = receipt(&root, "t");
        assert!(receipt.verified_terminal);
        assert_eq!(json!(receipt.erase_order), json!(helper.erased));
    }
}

#[test]
fn a_noop_update_acquires_once_and_writes_nothing() {
    let (_checkout, root) = private_root();
    let source = image(30000, 3);
    let unit = unit(development::payload(&source, &BANK1).unwrap());
    let mut port = Fake::new(unit.flash.clone());

    let report = develop(
        &root,
        &unit,
        &mut port,
        &update(Some(&source), &source, BANK1),
        "t",
    )
    .unwrap();

    assert_eq!(report.erase_sectors, Some(0));
    let helper = port.0.borrow();
    assert!(helper.erased.is_empty());
    assert!(helper.flash == unit.flash);
    assert!(!receipt(&root, "t").mutation_intent_recorded);
}

#[test]
fn protection_or_a_changed_guard_stops_before_any_mutation() {
    let source = image(30000, 3);
    let target = variant(&source, false);
    let unit = unit(development::payload(&source, &BANK1).unwrap());

    // Block-protect bits covering all of the array.
    let (_checkout, root) = private_root();
    let mut port = Fake::new(unit.flash.clone());
    port.0.borrow_mut().status = [7 << 2, 2, 0];

    let error = develop(
        &root,
        &unit,
        &mut port,
        &update(Some(&source), &target, BANK1),
        "t",
    )
    .unwrap_err();

    let crate::Error::Status(crate::link::status::StatusError::Blocked { boundary, reasons }) =
        &error
    else {
        panic!("not a status block: {error}");
    };
    assert_eq!(*boundary, crate::evidence::journal::Boundary::BeforeErase);
    assert_eq!(
        reasons,
        &[crate::link::status::Blocking::SoftwareProtectedRange]
    );
    assert_eq!(error.kind(), crate::error::ErrorKind::Refused);
    assert!(port.0.borrow().erased.is_empty());
    let stopped = receipt(&root, "t");
    assert_eq!(stopped.terminal_event, "transaction_stopped");
    assert!(!stopped.mutation_intent_recorded);

    // Bank 0 is neither the pinned baseline nor a valid image.
    let (_checkout, root) = private_root();
    let mut flash = unit.flash.clone();
    flash[0x6100] ^= 1;
    let mut port = Fake::new(flash);

    assert!(
        develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "t"
        )
        .is_err()
    );
    assert!(port.0.borrow().erased.is_empty());
}

#[test]
fn a_lost_ack_is_terminal_and_the_interrupted_mode_finishes_from_a_fresh_read() {
    for fault in [Fault::LostEraseAck(2), Fault::LostProgramAck(6)] {
        let (_checkout, root) = private_root();
        let source = image(30000, 3);
        let target = variant(&source, false);
        let old = development::payload(&source, &BANK1).unwrap().to_vec();
        let new = development::payload(&target, &BANK1).unwrap().to_vec();
        let unit = unit(&old);
        let mut port = Fake::new(unit.flash.clone());
        port.0.borrow_mut().fault = fault;

        let stop = develop(
            &root,
            &unit,
            &mut port,
            &update(Some(&source), &target, BANK1),
            "first",
        );

        assert!(stop.is_err());
        let stopped = receipt(&root, "first");
        assert_eq!(stopped.terminal_event, "transaction_stopped");
        assert!(stopped.mutation_intent_recorded);
        let report: Value =
            serde_json::from_slice(&fs::read(root.join("private/first/report.json")).unwrap())
                .unwrap();
        assert_eq!(report["status"], "stopped");
        assert_eq!(report["mutation_possible"], true);

        port.0.borrow_mut().reconnect();

        // A plain retry is refused: bank 1 is no longer the source.
        assert!(
            develop(
                &root,
                &unit,
                &mut port,
                &update(Some(&source), &target, BANK1),
                "retry"
            )
            .is_err()
        );

        let mut finish = update(Some(&source), &target, BANK1);
        finish.interrupted = true;
        develop(&root, &unit, &mut port, &finish, "finish").unwrap();

        let mut want = unit.flash.clone();
        want[START..START + new.len()].copy_from_slice(&new);
        assert!(port.0.borrow().flash == want, "{fault:?}");
    }
}

#[test]
fn interrupted_mode_refuses_unknown_sector_content() {
    let source = image(30000, 3);
    let target = variant(&source, false);
    let old = development::payload(&source, &BANK1).unwrap().to_vec();
    let mut finish = update(Some(&source), &target, BANK1);
    finish.interrupted = true;

    // The marker sector is not staged.
    assert!(matches!(
        development::images(&finish, &old),
        Err(crate::Error::Development(
            development::DevelopmentError::MarkerNotStaged
        ))
    ));

    let (_, staged) = development::images(&update(Some(&source), &target, BANK1), &old).unwrap();
    let mut current = old.clone();
    current[..SECTOR].copy_from_slice(&staged[..SECTOR]);
    assert!(development::images(&finish, &current).is_ok());

    let changed: Vec<usize> = (SECTOR..old.len())
        .step_by(SECTOR)
        .filter(|at| old[*at..at + SECTOR] != staged[*at..at + SECTOR])
        .collect();

    let partial = |sector: &mut [u8], at: usize, frames: usize| {
        sector[at..at + SECTOR].fill(0xff);
        sector[at..at + frames * 1024].copy_from_slice(&staged[at..at + frames * 1024]);
    };

    // Two partially programmed sectors.
    let mut two = current.clone();
    partial(&mut two, changed[0], 1);
    partial(&mut two, changed[1], 1);
    assert!(matches!(
        development::images(&finish, &two),
        Err(crate::Error::Development(
            development::DevelopmentError::PartialSectors { count: 2 }
        ))
    ));

    // One is the state a stopped program leaves.
    let mut one = current.clone();
    partial(&mut one, changed[0], 2);
    assert!(development::images(&finish, &one).is_ok());

    // Anything else is refused.
    one[changed[0] + 4000] = 0;
    let unknown = development::images(&finish, &one).unwrap_err();
    assert!(matches!(
        unknown,
        crate::Error::Development(development::DevelopmentError::UnknownSector { offset })
            if offset == BANK1.start + changed[0]
    ));
}

/// A bank-0 update stopped between the marker sector's erase and its first
/// program frame: that sector reads wholly erased and every other sector is
/// still the source. The interrupted mode finishes it, staging the marker
/// sector first and committing the marker last.
#[test]
fn interrupted_mode_finishes_a_stop_between_the_marker_erase_and_its_program() {
    let installed = image(30000, 3);
    let source = image(20000, 9);
    let target = variant(&source, true);
    let old0 = development::payload(&source, &BANK0).unwrap().to_vec();
    let new0 = development::payload(&target, &BANK0).unwrap().to_vec();
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let mut stopped = unit.flash.clone();
    stopped[0x6000..0x6000 + old0.len()].copy_from_slice(&old0);
    stopped[0x6000..0x6000 + SECTOR].fill(0xff);

    let mut finish = update(Some(&source), &target, BANK0);
    finish.interrupted = true;
    let (_checkout, root) = private_root();
    let mut port = Fake::new(stopped.clone());
    develop(&root, &unit, &mut port, &finish, "finish").unwrap();

    let helper = port.0.borrow();
    assert_eq!(&helper.flash[0x6000..0x6000 + new0.len()], &new0[..]);
    assert_eq!(
        helper.erased[0], 0x6000,
        "the marker sector is staged first"
    );
    assert_eq!(&helper.flash[START..START + 8], MARKER, "bank 1 untouched");
    let mut outside = helper.flash.clone();
    outside[0x6000..0x6000 + new0.len()].copy_from_slice(&stopped[0x6000..0x6000 + new0.len()]);
    assert!(
        outside == stopped,
        "nothing outside bank 0's update changed"
    );

    // A plain update refuses it: bank 0 is no longer the source.
    let plain = update(Some(&source), &target, BANK0);
    let length = development::extent(Some(&source), &target, &BANK0).unwrap();
    let current = stopped[0x6000..0x6000 + length].to_vec();
    assert!(matches!(
        development::images(&plain, &current),
        Err(crate::Error::Development(
            development::DevelopmentError::NotSource
        ))
    ));
}

#[test]
fn an_erased_marker_sector_is_accepted_only_with_the_rest_exactly_the_source() {
    let source = image(20000, 9);
    let target = variant(&source, false);
    let old0 = development::payload(&source, &BANK0).unwrap().to_vec();
    let mut finish = update(Some(&source), &target, BANK0);
    finish.interrupted = true;
    let mut erased = old0.clone();
    erased[..SECTOR].fill(0xff);
    assert!(development::images(&finish, &erased).is_ok());

    // One byte elsewhere differs from the source.
    let mut drifted = erased.clone();
    drifted[3 * SECTOR + 17] ^= 1;
    assert!(matches!(
        development::images(&finish, &drifted),
        Err(crate::Error::Development(
            development::DevelopmentError::UnknownSector { offset }
        )) if offset == BANK0.start + 3 * SECTOR
    ));

    // Another sector erased too: not the state one stopped step leaves.
    let mut two = erased.clone();
    two[2 * SECTOR..3 * SECTOR].fill(0xff);
    assert!(development::images(&finish, &two).is_err());

    // A stray byte in the marker sector, or frames that are not the
    // target's staged sector, are refused as before.
    let (_, staged) = development::images(&update(Some(&source), &target, BANK0), &old0).unwrap();
    let mut stray = erased.clone();
    stray[100] = 0;
    let mut foreign = erased.clone();
    foreign[..1024].copy_from_slice(&noise(1024, 5));
    let mut source_frames = erased.clone();
    source_frames[..2048].copy_from_slice(&old0[..2048]);
    for current in [stray, foreign, source_frames] {
        assert!(matches!(
            development::images(&finish, &current),
            Err(crate::Error::Development(
                development::DevelopmentError::MarkerNotStaged
            ))
        ));
    }
    // The staged sector's own first frames are still the staged sector's
    // partial program, which the marker rule never accepted either.
    let mut own = erased;
    own[..1024].copy_from_slice(&staged[..1024]);
    assert!(development::images(&finish, &own).is_err());
}

/// The erased-marker finish keeps the other bank's guards: bank 1 must be
/// valid, and must not change between the plan and its execution.
#[test]
fn an_erased_marker_finish_still_requires_bank1_unchanged() {
    let installed = image(30000, 3);
    let source = image(20000, 9);
    let target = variant(&source, false);
    let old0 = development::payload(&source, &BANK0).unwrap().to_vec();
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let mut stopped = unit.flash.clone();
    stopped[0x6000..0x6000 + old0.len()].copy_from_slice(&old0);
    stopped[0x6000..0x6000 + SECTOR].fill(0xff);
    let mut finish = update(Some(&source), &target, BANK0);
    finish.interrupted = true;

    // Bank 1 invalid.
    let (_checkout, root) = private_root();
    let mut invalid = stopped.clone();
    invalid[START] = 0;
    let mut port = Fake::new(invalid);
    assert!(develop(&root, &unit, &mut port, &finish, "t").is_err());
    assert!(port.0.borrow().erased.is_empty());

    // Bank 1 changed after the plan pinned its first sector.
    let (_checkout, root) = private_root();
    let mut port = Fake::new(stopped);
    let length = development::extent(Some(&source), &target, &BANK0).unwrap();
    let current = protocol::read_region(&mut port, 0x6000, length).unwrap();
    let other = protocol::read_region(&mut port, BANK0.other.0, BANK0.other.1).unwrap();
    let plan = development::plan(&finish, &current, Some(&other)).unwrap();
    port.0.borrow_mut().flash[START + 100] ^= 1;
    let mut acquire = acquire_for(BANK0, length);
    let transaction = development::Transaction {
        baseline: &unit.baseline,
        update: &finish,
        current: &current,
        other: Some(&other),
        reviewed: &plan,
    };
    let result = development::run(
        &mut port,
        &root,
        &transaction,
        &mut acquire,
        &root.join("private/t"),
        &mut |_, _| {},
    );
    assert!(result.is_err());
    assert!(port.0.borrow().erased.is_empty());
}

#[test]
fn bank0_update_keeps_bank1_valid_and_never_frames_outside_bank0() {
    let (_checkout, root) = private_root();
    let installed = image(30000, 3);
    let target = image(20000, 9);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    let new = development::payload(&target, &BANK0).unwrap().to_vec();
    let original = unit.flash[0x6000..0x6000 + new.len()].to_vec();
    let mut bank0 = update(None, &target, BANK0);
    bank0.original = Some(&original);
    let mut port = Fake::new(unit.flash.clone());

    develop(&root, &unit, &mut port, &bank0, "t").unwrap();

    {
        let helper = port.0.borrow();
        assert!(
            helper
                .erased
                .iter()
                .all(|at| (0x6000..0x156000).contains(at))
        );
        assert_eq!(&helper.flash[0x6000..0x6000 + new.len()], &new[..]);
        assert_eq!(&helper.flash[START..START + 8], MARKER);
    }

    for offset in [0x156000, 0x5c00] {
        let error = development::frame(&BANK0, 1, offset, &[0; 1024]).unwrap_err();
        let crate::Error::Sector(crate::link::sector::SectorError::PacketOutside {
            bank,
            offset: at,
        }) = &error
        else {
            panic!("not a bounded frame: {error}");
        };
        assert_eq!(
            (*bank, *at),
            (crate::link::sector::Bank::Development(0), offset)
        );
        assert_eq!(error.to_string(), "Packet outside aligned bank 0");
    }

    // Bank 0 requires a valid bank 1.
    let (_checkout, root) = private_root();
    let mut flash = unit.flash.clone();
    flash[START] = 0;
    let mut port = Fake::new(flash);
    assert!(develop(&root, &unit, &mut port, &bank0, "t").is_err());
    assert!(port.0.borrow().erased.is_empty());
}

#[test]
fn give_way_then_restore_marker_round_trip() {
    let installed = image(30000, 3);
    let fallback = image(20000, 9);
    let old = development::payload(&installed, &BANK1).unwrap().to_vec();
    let bank0 = development::payload(&fallback, &BANK0).unwrap().to_vec();
    let unit = unit(&old);
    let mut flash = unit.flash.clone();
    flash[0x6000..0x6000 + bank0.len()].copy_from_slice(&bank0);
    let mut port = Fake::new(flash.clone());
    let (_checkout, root) = private_root();
    let first = protocol::read_region(&mut port, START, SECTOR).unwrap();
    let plan = development::give_way_plan(&fallback, &bank0, &first).unwrap();
    let length = bank0.len();

    let mut acquire = |port: &mut dyn Port| -> crate::Result<[Vec<u8>; 3]> {
        Ok([
            protocol::read_region(port, 0, 0x6000)?,
            protocol::read_region(port, 0x6000, length)?,
            protocol::read_region(port, START, SECTOR)?,
        ])
    };

    let give_way = development::GiveWay {
        baseline: &unit.baseline,
        fallback: &fallback,
        bank0: &bank0,
        bank1_sector: &first,
        reviewed: &plan,
    };
    development::run_give_way(
        &mut port,
        &root,
        &give_way,
        &mut acquire,
        &root.join("private/give-way"),
    )
    .unwrap();

    assert_eq!(port.0.borrow().flash[START], 0);
    assert!(port.0.borrow().erased.is_empty(), "a give-way never erases");

    // Restoring needs the explicit mode; a plain update refuses.
    let plain = update(Some(&installed), &installed, BANK1);
    assert!(develop(&root, &unit, &mut port, &plain, "plain").is_err());

    let mut restore = plain;
    restore.restore = true;
    develop(&root, &unit, &mut port, &restore, "restore").unwrap();
    assert!(port.0.borrow().flash == flash);
}
