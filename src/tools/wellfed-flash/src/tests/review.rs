//! Frozen image reviews: what binds a review to its image, manifest and
//! required audits.

use super::*;

/// A frozen review of a firstboot build, and the checkout around it.
fn frozen_build(checkout: &Path, image: &[u8]) -> (PathBuf, Value, Value) {
    let directory = checkout.join("private/review");
    let (review, manifest) =
        crate::fixtures::freeze_build(checkout, &directory, image, &json!({})).unwrap();
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
