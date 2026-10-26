//! Frozen image reviews: what binds a review to its image, manifest and
//! required audits.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::bank::BANK1;
use crate::fixtures::{synthetic_payload, wrap};

/// A checkout with an empty `private/`.
fn private_root() -> (tempfile::TempDir, Root) {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir(checkout.path().join("private")).unwrap();
    let root = Root::new(checkout.path()).unwrap();
    (checkout, root)
}

fn image(km4: usize, seed: u8) -> Vec<u8> {
    wrap(&synthetic_payload(9000, km4, seed))
}

fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

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

    let accepted = frozen(&root, &directory, &BANK1).unwrap();
    assert_eq!(accepted.image, image);

    let refusal = || frozen(&root, &directory, &BANK1).err();

    // A changed image byte.
    let mut changed = image.clone();
    *changed.last_mut().unwrap() ^= 1;
    write(&directory.join("image.bin"), &changed);
    assert!(matches!(
        refusal(),
        Some(crate::Error::Ota(crate::ota::Error::Checksum))
    ));
    write(&directory.join("image.bin"), &image);

    // A replaced manifest.
    let manifest = fs::read(directory.join("build-manifest.json")).unwrap();
    write(&directory.join("build-manifest.json"), b"{}");
    assert!(matches!(
        refusal(),
        Some(crate::Error::Review(Error::ManifestMismatch))
    ));
    write(&directory.join("build-manifest.json"), &manifest);

    // A review that names another image, sizes with a bool, or lacks the mapping.
    type Expected = fn(&crate::Error) -> bool;
    let cases: [(&str, Value, Expected); 5] = [
        ("/ota_sha256", json!("0".repeat(64)), |error| {
            matches!(error, crate::Error::Review(Error::ImageMismatch))
        }),
        ("/payload_bytes", json!(true), |error| {
            matches!(error, crate::Error::Json(_))
        }),
        ("/review/factory_mapping", json!([]), |error| {
            matches!(error, crate::Error::Review(Error::IncompleteMapping))
        }),
        ("/kind", json!("other"), |error| {
            matches!(error, crate::Error::Review(Error::Unsupported))
        }),
        ("/image_kind", json!("unknown"), |error| {
            matches!(error, crate::Error::Json(_))
        }),
    ];
    for (path, value, expected) in cases {
        let mut bad = review.clone();
        *bad.pointer_mut(path).unwrap() = value;
        write(&directory.join("review.json"), bad.to_string().as_bytes());
        let error = refusal().unwrap();
        assert!(expected(&error), "{path}: {error:?}");
    }

    // The restored review is accepted again, so each refusal above came from
    // its own change.
    write(
        &directory.join("review.json"),
        review.to_string().as_bytes(),
    );
    assert!(refusal().is_none());
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
        let mut bound = review.clone();
        bound["manifest_sha256"] = json!(sha256(raw.as_bytes()));
        write(&directory.join("build-manifest.json"), raw.as_bytes());
        write(&directory.join("review.json"), bound.to_string().as_bytes());
        frozen(&root, &directory, &BANK1).is_ok()
    };

    assert!(freeze(None, "plaf103-firstboot"));
    assert!(freeze(Some("plaf103"), "firstboot"));
    assert!(!freeze(Some("plaf103"), "plaf103-firstboot"));
    assert!(!freeze(None, "firstboot"));
}
