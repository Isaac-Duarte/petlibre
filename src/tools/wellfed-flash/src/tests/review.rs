//! Frozen image reviews: what binds a review to its image, manifest and
//! required audits.

use super::*;

/// A frozen review of a firstboot build, and the checkout around it.
fn frozen_build(checkout: &Path, image: &[u8]) -> (PathBuf, Value, Value) {
    let lock = json!({"sdk": {"url": "sdk", "revision": "r1"},
                      "toolchain": {"url": "toolchain", "sha256": "t".repeat(64), "directory": "asdk"}});
    write(
        &checkout.join("src/firmware/vendor/sdk.lock.json"),
        lock.to_string().as_bytes(),
    );

    let mains = json!({"hp": {"address": "0x1"}, "lp": {"address": "0x2"}});
    let pinmap = json!({"matches_stock": true, "runtime_address": "0x3"});
    let manifest = json!({
        "device": "plaf103", "profile": "firstboot", "sdk_revision": "r1", "toolchain_sha256": "t".repeat(64),
        "artifacts": [{"path": "image/OTA_All.bin", "sha256": sha256(image), "bytes": image.len()}],
        "firstboot_startup_audit": {"mains": mains}, "km0_pinmap_audit": pinmap,
        "checks": {"ota_payload_matches": true, "ota_checksum_matches": true,
                   "source_snapshot_unchanged": true, "staged_sources_without_diagnostics": true},
    });

    let payload = development::payload(image, &BANK1).unwrap();
    // The loader runs themselves are not checked here, only that both banks ran.
    let run = json!({"stop": "0x8f000", "result": 1, "configs": [], "commands": [],
                     "final_registers": [[0, 0, 0, 0], [0, 0, 0, 0]]});
    let case = |bank: u8| json!({"bank": bank, "bank_check": run, "selection": run});
    let review = json!({
        "schema_version": 1, "kind": "plaf103_development_image_review", "image_kind": "build",
        "ota_sha256": sha256(image), "payload_sha256": sha256(payload), "payload_bytes": payload.len(),
        "manifest_sha256": sha256(manifest.to_string().as_bytes()),
        "review": {"startup_mains": mains, "pinmap": pinmap, "layout": {"sha256": sha256(image)},
                   "factory_mapping": [case(0), case(1)], "scope": "unit"},
        "reviewer_sha256": "0".repeat(64), "application_boot_verified": false,
    });

    let directory = checkout.join("private/review");
    write(&directory.join("image.bin"), image);
    write(
        &directory.join("build-manifest.json"),
        manifest.to_string().as_bytes(),
    );
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    );

    (directory, review, manifest)
}

#[test]
fn frozen_reviews_bind_image_manifest_and_required_audits() {
    let (checkout, root) = private_root();
    let image = image(30000, 3);
    let (directory, review, _) = frozen_build(checkout.path(), &image);

    let frozen = crate::evidence::review::frozen(&root, &directory, &BANK1).unwrap();
    assert_eq!(frozen.image, image);

    let refused = || crate::evidence::review::frozen(&root, &directory, &BANK1).is_err();

    // A changed image byte.
    let mut changed = image.clone();
    *changed.last_mut().unwrap() ^= 1;
    write(&directory.join("image.bin"), &changed);
    assert!(refused());
    write(&directory.join("image.bin"), &image);

    // A replaced manifest.
    let manifest = fs::read(directory.join("build-manifest.json")).unwrap();
    write(&directory.join("build-manifest.json"), b"{}");
    assert!(refused());
    write(&directory.join("build-manifest.json"), &manifest);

    // A review that names another image, sizes with a bool, or lacks the mapping.
    for (path, value) in [
        ("/ota_sha256", json!("0".repeat(64))),
        ("/payload_bytes", json!(true)),
        ("/review/factory_mapping", json!([])),
        ("/kind", json!("other")),
        ("/image_kind", json!("unknown")),
    ] {
        let mut bad = review.clone();
        *bad.pointer_mut(path).unwrap() = value;
        write(&directory.join("review.json"), bad.to_string().as_bytes());
        assert!(refused(), "{path}");
    }

    // The restored review is accepted again, so each refusal above came from
    // its own change.
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    );
    assert!(!refused());
}

#[test]
fn reviews_of_builds_before_device_names_still_read() {
    let (checkout, root) = private_root();
    let image = image(30000, 3);
    let (directory, review, manifest) = frozen_build(checkout.path(), &image);

    // A manifest as older builds wrote it: no device, the old profile
    // name. Any other pairing stays unknown.
    let freeze = |device: Option<&str>, profile: &str| {
        let mut changed = manifest.clone();
        let object = changed.as_object_mut().unwrap();
        object.remove("device");
        if let Some(device) = device {
            object.insert("device".into(), json!(device));
        }
        object.insert("profile".into(), json!(profile));
        let raw = changed.to_string();
        let mut frozen = review.clone();
        frozen["manifest_sha256"] = json!(sha256(raw.as_bytes()));
        write(&directory.join("build-manifest.json"), raw.as_bytes());
        write(
            &directory.join("review.json"),
            frozen.to_string().as_bytes(),
        );
        crate::evidence::review::frozen(&root, &directory, &BANK1).is_ok()
    };

    assert!(freeze(None, "plaf103-firstboot"));
    assert!(freeze(Some("plaf103"), "firstboot"));
    assert!(!freeze(Some("plaf103"), "plaf103-firstboot"));
    assert!(!freeze(None, "firstboot"));
}
