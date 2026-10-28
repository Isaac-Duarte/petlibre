//! Release files checked against the project's and the owner's keys, with
//! manifests signed the way `wellfed-build sign` signs them.

use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use wellfed_client::release::{
    Keys, MANIFEST_LEN, MAX_LEN, OfferError, ReleaseError, Signer, parts, verify,
};
use wellfed_core::ota::offer::{self, ImageKind, Signed, key_id};
use wellfed_core::ota::{MARKER, Manifest, SECTOR};

fn project() -> SigningKey {
    SigningKey::from_bytes(&[0x05; 32])
}

fn owner() -> SigningKey {
    SigningKey::from_bytes(&[0x06; 32])
}

fn stranger() -> SigningKey {
    SigningKey::from_bytes(&[0x07; 32])
}

fn image() -> Vec<u8> {
    let mut image: Vec<u8> = (0..2 * SECTOR).map(|i| (i % 251) as u8).collect();
    image[..8].copy_from_slice(&MARKER);
    image
}

/// A release of `image` at `version`, signed by `key`.
fn release(key: &SigningKey, version: u32, kind: ImageKind, image: &[u8]) -> Vec<u8> {
    for_bank(key, version, kind, 1, image)
}

/// [`release`], aimed at `bank`.
fn for_bank(key: &SigningKey, version: u32, kind: ImageKind, bank: u32, image: &[u8]) -> Vec<u8> {
    let signed = Signed {
        manifest: Manifest {
            version,
            image_len: u32::try_from(image.len()).unwrap(),
            target_bank: bank,
            image_sha256: Sha256::digest(image).into(),
            build_id: [0x3c; 32],
        },
        kind,
        key_id: key_id(&key.verifying_key().to_bytes()),
    };
    let body = offer::manifest_body(&signed);
    let mut file = body.to_vec();
    file.extend_from_slice(&key.sign(&body).to_bytes());
    file.extend_from_slice(image);
    file
}

fn trusted() -> ([[u8; 32]; 1], [[u8; 32]; 1]) {
    (
        [project().verifying_key().to_bytes()],
        [owner().verifying_key().to_bytes()],
    )
}

fn check(file: &[u8]) -> Result<wellfed_client::release::Verified, ReleaseError> {
    let (project, owner) = trusted();
    verify(
        file,
        &Keys {
            project: &project,
            owner: &owner,
        },
    )
}

#[test]
fn a_good_release_verifies_and_names_its_signer() {
    let image = image();
    let file = release(&project(), 9, ImageKind::Replacement, &image);

    let verified = check(&file).unwrap();

    assert_eq!(verified.version, 9);
    assert_eq!(verified.signer, Signer::Project);
    assert_eq!(
        verified.image_sha256,
        <[u8; 32]>::from(Sha256::digest(&image))
    );
    assert_eq!(
        verified.digest,
        offer::manifest_digest(&file[..MANIFEST_LEN])
    );
    assert_eq!(parts(&file).unwrap().1, image.as_slice());

    let own = release(&owner(), 10, ImageKind::Replacement, &image);
    assert_eq!(check(&own).unwrap().signer, Signer::Owner);
}

#[test]
fn a_bad_signature_is_refused() {
    let mut file = release(&project(), 9, ImageKind::Replacement, &image());
    file[MANIFEST_LEN - 1] ^= 0x01;

    assert_eq!(check(&file), Err(ReleaseError::Signature));
}

#[test]
fn a_key_neither_list_holds_is_refused() {
    let file = release(&stranger(), 9, ImageKind::Replacement, &image());

    assert_eq!(check(&file), Err(ReleaseError::UnknownKey));
}

#[test]
fn a_corrupted_image_is_refused() {
    let image = image();
    let file = release(&project(), 9, ImageKind::Replacement, &image);

    let mut flipped = file.clone();
    *flipped.last_mut().unwrap() ^= 0x80;
    assert_eq!(check(&flipped), Err(ReleaseError::ImageDigest));

    let cut = &file[..file.len() - 1];
    assert_eq!(
        check(cut),
        Err(ReleaseError::ImageLength {
            expected: u32::try_from(image.len()).unwrap(),
            actual: image.len() - 1,
        })
    );

    assert_eq!(check(&file[..100]), Err(ReleaseError::Short { len: 100 }));
    let mut garbage = file.clone();
    garbage[0] = b'X';
    assert_eq!(
        check(&garbage),
        Err(ReleaseError::Manifest(OfferError::Magic))
    );
}

#[test]
fn what_needs_a_feeders_statement_is_not_a_release() {
    let image = image();

    let stock = release(&project(), 9, ImageKind::Stock, &image);
    assert_eq!(check(&stock), Err(ReleaseError::Stock));

    let zero = release(&project(), 0, ImageKind::Replacement, &image);
    assert_eq!(
        check(&zero),
        Err(ReleaseError::Refused(OfferError::Downgrade))
    );
}

#[test]
fn what_a_feeder_would_refuse_is_not_a_release() {
    let image = image();

    let bank0 = for_bank(&project(), 9, ImageKind::Replacement, 0, &image);
    assert_eq!(check(&bank0), Err(ReleaseError::Refused(OfferError::Bank)));

    let tiny = release(&project(), 9, ImageKind::Replacement, &image[..SECTOR - 1]);
    assert_eq!(check(&tiny), Err(ReleaseError::Refused(OfferError::Size)));

    let huge = vec![0; MAX_LEN + 1];
    assert_eq!(
        check(&huge),
        Err(ReleaseError::TooLarge { len: MAX_LEN + 1 })
    );
}

#[test]
fn an_image_without_the_banks_marker_is_refused() {
    let mut image = image();
    image[0] = b'0';

    let file = release(&project(), 9, ImageKind::Replacement, &image);

    assert_eq!(check(&file), Err(ReleaseError::Marker));
}
