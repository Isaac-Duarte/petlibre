//! Original-stock recovery of bank 1 against the modeled helper.

use super::*;

#[test]
fn stock_recovery_restores_bank1_and_preserves_everything_else() {
    let (_checkout, root) = private_root();
    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());

    // A bootable bank is never rewritten in place.
    let mut current = unit.flash.clone();
    assert!(crate::writers::recovery::plan(&unit.baseline, &current).is_err());
    current[START] = 0;
    let plan = crate::writers::recovery::plan(&unit.baseline, &current).unwrap();

    let offsets: Vec<usize> = plan
        .ordered_sector_payloads
        .iter()
        .map(|s| s.offset)
        .collect();
    assert!(offsets.len() > 1);
    assert_eq!(*offsets.last().unwrap(), START, "marker sector last");

    // The reviewed plan as its file holds it.
    let plan = serde_json::to_value(&plan).unwrap();
    let mut port = Fake::new(current.clone());
    let mut snapshot = |port: &mut dyn Port| protocol::read_region(port, 0, CAPACITY);
    let report = crate::writers::recovery::run_transaction(
        &mut port,
        &root,
        &unit.baseline,
        &current,
        &plan,
        &mut snapshot,
        &root.join("private/recovery"),
    )
    .unwrap();

    let mut want = current.clone();
    want[START..sector::END].copy_from_slice(&unit.baseline.bytes()[START..sector::END]);
    assert!(port.0.borrow().flash == want);
    assert_eq!(report.snapshot_sha256, sha256(&want));

    let receipt = receipt(&root, "recovery");
    assert!(receipt.verified_terminal);
    assert_eq!(receipt.transaction_id, Some(json!(report.transaction_id)));

    // A changed plan is refused before any journal exists.
    let mut changed = plan.clone();
    changed["changed_sector_count"] = json!(0);
    assert!(
        crate::writers::recovery::run_transaction(
            &mut port,
            &root,
            &unit.baseline,
            &current,
            &changed,
            &mut snapshot,
            &root.join("private/second"),
        )
        .is_err()
    );
    assert!(!root.join("private/second").exists());
}
