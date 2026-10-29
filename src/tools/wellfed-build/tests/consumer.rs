//! A consumer image carries no build Wi-Fi and no bench key: its packaged
//! OTA holds neither the hotspot's SSID or passphrase nor the bench update
//! key's public half, and a bench image of the same checkout holds them, so
//! the scan is known to see them.
//!
//! Optional evidence (`make check-fixtures`): these need the ignored
//! `private/firmware-build/plaf103/firstboot-consumer` and `…/firstboot`
//! builds of the current sources, `private/hotspot.json` and the bench key.
//! No value is ever printed: a failure names only what was found.

use std::fs;
use std::path::{Path, PathBuf};

use wellfed_build::identity::current_inputs;
use wellfed_build::{Device, Firmware, Profile, Provisioning, Repo};
use wellfed_formats::credentials::{OtaKeyFile, WifiCredential};
use wellfed_formats::manifest::BuildManifest;

/// The packaged OTA of `provisioning`'s first-boot build, checked to be of
/// the current sources.
fn image(repo: &Repo, provisioning: Provisioning) -> (BuildManifest, Vec<u8>) {
    let device = Device::new(repo, "plaf103").unwrap();
    let dir = device.output(repo, Profile::Firstboot, provisioning);
    let manifest: BuildManifest = wellfed_formats::load(&dir.join("manifest.json"))
        .expect("completed ignored first-boot build");
    assert_eq!(manifest.provisioning, provisioning);

    let firmware = Firmware::new(repo, manifest.firmware.as_deref().unwrap()).unwrap();
    let inputs =
        current_inputs(repo, &device, Profile::Firstboot, provisioning, &firmware).unwrap();
    assert!(
        manifest.inputs == inputs,
        "stale {provisioning:?} first-boot build; rebuild it before using it as evidence"
    );

    let ota: PathBuf = dir.join(&manifest.artifact("OTA_All.bin").unwrap().path);
    let bytes = fs::read(ota).unwrap();
    (manifest, bytes)
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Each secret or key to look for, by a name that is safe to print.
fn needles(root: &Path) -> Vec<(&'static str, Vec<u8>)> {
    let wifi: WifiCredential = wellfed_formats::load(&root.join("private/hotspot.json")).unwrap();
    let bench: OtaKeyFile = wellfed_formats::load(&root.join(wellfed_signing::KEY_PATH)).unwrap();
    let raw = wellfed_formats::hex::decode::<32>(&bench.public_key).unwrap();
    vec![
        ("the hotspot's SSID", wifi.ssid.into_bytes()),
        ("the hotspot's passphrase", wifi.password.into_bytes()),
        (
            "the bench key's public half (hex)",
            bench.public_key.into_bytes(),
        ),
        ("the bench key's public half (bytes)", raw.to_vec()),
    ]
}

#[test]
#[ignore = "needs bench and consumer first-boot builds of the current sources"]
fn a_consumer_image_carries_no_build_wifi_and_no_bench_key() {
    let repo = Repo::here().unwrap();
    let (consumer, consumer_image) = image(&repo, Provisioning::Consumer);
    let (_, bench_image) = image(&repo, Provisioning::Bench);
    let needles = needles(&repo.root);

    // The bench image embeds the SSID, the passphrase and the key's hex, so
    // a scan that misses them in the consumer's is not blind.
    for (name, needle) in &needles[..3] {
        assert!(holds(&bench_image, needle), "the bench image lacks {name}");
    }
    for (name, needle) in &needles {
        assert!(
            !holds(&consumer_image, needle),
            "the consumer image holds {name}"
        );
    }

    let release = consumer
        .ota_public_key
        .expect("a consumer build embeds a key");
    assert!(
        holds(&consumer_image, release.as_bytes()),
        "the consumer image lacks the release key its manifest names"
    );
}
