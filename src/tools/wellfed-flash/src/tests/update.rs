//! The update command's refusals before it opens any serial port.

use super::*;

#[test]
fn the_cli_refuses_an_unpinned_helper_before_any_serial_access() {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir(checkout.path().join("private")).unwrap();

    let installed = image(30000, 3);
    let unit = unit(development::payload(&installed, &BANK1).unwrap());
    write(
        &checkout.path().join("private/baseline.bin"),
        unit.baseline.bytes(),
    );

    let opened = Rc::new(RefCell::new(0));
    let count = opened.clone();
    let context = Context {
        root: Root::new(checkout.path()).unwrap(),
        tool_sha256: sha256(b"test binary"),
        open: Box::new(move |_| {
            *count.borrow_mut() += 1;
            Ok(Box::new(Fake::new(vec![0xff; CAPACITY])))
        }),
        pin: Pin::Synthetic,
    };

    let private = |name: &str| checkout.path().join("private").join(name);
    let mut helper = vec![0u8; protocol::HELPER_BYTES];
    helper[..4].copy_from_slice(&(protocol::HELPER_ADDRESS + 0x21).to_le_bytes());
    write(&private("helper.bin"), &helper);

    let evidence = |output: &str| crate::writers::update::Evidence {
        port: "/dev/fake".into(),
        baseline: private("baseline.bin"),
        rom_report: private("rom/report.json"),
        small_report: private("small/report.json"),
        helper: Some(private("helper.bin")),
        output: private(output),
        inspection: None,
        plan: None,
        expected_plan_sha256: None,
    };

    let review = |name: &str, image: &[u8]| -> PathBuf {
        let payload = development::payload(image, &BANK1).unwrap();
        write(&private(name).join("image.bin"), image);
        write(
            &private(name).join("review.json"),
            json!({"schema_version": 1, "kind": "plaf103_development_image_review",
                   "image_kind": "stock", "ota_sha256": sha256(image),
                   "payload_sha256": sha256(payload), "payload_bytes": payload.len(),
                   "manifest_sha256": null, "review": {"stock_sha256": sha256(image)},
                   "reviewer_sha256": "0".repeat(64), "application_boot_verified": false})
            .to_string()
            .as_bytes(),
        );
        private(name)
    };

    let args = crate::writers::update::Args {
        mode: crate::writers::update::Mode::Inspect,
        evidence: evidence("inspect"),
        target: review("target", &variant(&installed, false)),
        source: Some(review("source", &installed)),
        original_read: None,
        bank: 1,
        restore_marker: false,
        interrupted: false,
    };

    // A synthetic image is not the pinned stock image.
    let error = crate::writers::update::run(&context, &args).err().unwrap();
    assert!(matches!(error, crate::Error::Image(_)), "{error}");
    assert_eq!(error.kind(), crate::error::ErrorKind::Refused);
    assert_eq!(*opened.borrow(), 0);
    assert!(!private("inspect").exists());
    let _ = evidence("unused");
}
