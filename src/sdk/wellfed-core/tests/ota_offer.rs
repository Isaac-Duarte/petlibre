//! Offers over the device API: the version 2 manifest, the key ring, key
//! statements and the anti-rollback floor.
//!
//! An offer must name a trusted key that is not retired, carry its
//! signature, target bank 1 within the reviewed size, differ from the
//! running build, and be above the confirmed floor unless a downgrade
//! statement for exactly this feeder, manifest and floor comes with it.
//! Stock always needs one.

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::offer::*;
use wellfed_core::ota::{BANK1_BYTES, MARKER, Manifest, SECTOR, SECTOR_BYTES, image};
use proptest::prelude::*;

const DEVICE: [u8; 32] = [0xd0; 32];
const RUNNING: [u8; 32] = [0xee; 32];

fn signer(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(key: &SigningKey) -> [u8; 32] {
    key.verifying_key().to_bytes()
}

fn id(key: &SigningKey) -> KeyId {
    key_id(&public(key))
}

fn signed(key: &SigningKey, version: u32, kind: ImageKind) -> Signed {
    Signed {
        manifest: Manifest {
            version,
            image_len: 3 * SECTOR_BYTES,
            target_bank: 1,
            image_sha256: [0x5a; 32],
            build_id: [0xb1; 32],
        },
        kind,
        key_id: id(key),
    }
}

fn sign(key: &SigningKey, body: &[u8]) -> Vec<u8> {
    let mut out = body.to_vec();
    out.extend_from_slice(&key.sign(body).to_bytes());
    out
}

fn manifest(key: &SigningKey, fields: &Signed) -> Vec<u8> {
    sign(key, &manifest_body(fields))
}

fn downgrade(key: &SigningKey, device: [u8; 32], manifest: &[u8], floor: u32) -> Vec<u8> {
    let body = downgrade_body(&id(key), &device, &manifest_digest(manifest), floor);
    sign(key, &body)
}

struct Keys {
    bench: SigningKey,
    owner: SigningKey,
    release: SigningKey,
    stranger: SigningKey,
}

fn keys() -> Keys {
    Keys {
        bench: signer(1),
        owner: signer(2),
        release: signer(3),
        stranger: signer(4),
    }
}

fn context<'a>(
    built_in: &'a [[u8; 32]],
    enrolled: &'a [[u8; 32]],
    revoked: &'a [KeyId],
    floor: u32,
) -> Context<'a> {
    Context {
        ring: KeyRing {
            built_in,
            added: None,
            enrolled,
            revoked,
        },
        floor,
        device: DEVICE,
        running: Some(RUNNING),
    }
}

#[test]
fn the_manifest_body_round_trips_through_the_parser() {
    let k = keys();
    let fields = signed(&k.bench, 7, ImageKind::Stock);
    let bytes = manifest(&k.bench, &fields);
    assert_eq!(bytes.len(), MANIFEST_LEN);
    assert_eq!(parse_manifest(&bytes), Ok(fields));
    assert_eq!(key_id(&public(&k.bench)).len(), KEY_ID_LEN);
}

#[test]
fn a_newer_manifest_from_any_trusted_key_is_admitted() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let enrolled = [public(&k.owner)];
    let ctx = context(&built_in, &enrolled, &[], 4);

    let bytes = manifest(&k.bench, &signed(&k.bench, 5, ImageKind::Replacement));
    let admitted = admit_offer(&bytes, None, &ctx).unwrap();
    assert_eq!(admitted.trust, Trust::BuiltIn);
    assert_eq!(admitted.digest, manifest_digest(&bytes));
    assert_eq!(admitted.downgrade, None);

    let bytes = manifest(&k.owner, &signed(&k.owner, 5, ImageKind::Replacement));
    assert_eq!(
        admit_offer(&bytes, None, &ctx).unwrap().trust,
        Trust::Enrolled
    );

    let mut added = ctx;
    added.ring.added = Some(public(&k.release));
    let bytes = manifest(&k.release, &signed(&k.release, 5, ImageKind::Replacement));
    assert_eq!(
        admit_offer(&bytes, None, &added).unwrap().trust,
        Trust::Added
    );
    assert_eq!(
        admit_offer(&bytes, None, &ctx),
        Err(OfferError::Key(KeyError::Unknown))
    );
}

#[test]
fn unknown_retired_and_misnamed_keys_are_refused() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let bytes = manifest(&k.stranger, &signed(&k.stranger, 5, ImageKind::Replacement));
    assert_eq!(
        admit_offer(&bytes, None, &context(&built_in, &[], &[], 0)),
        Err(OfferError::Key(KeyError::Unknown))
    );

    let bytes = manifest(&k.bench, &signed(&k.bench, 5, ImageKind::Replacement));
    let revoked = [id(&k.bench)];
    assert_eq!(
        admit_offer(&bytes, None, &context(&built_in, &[], &revoked, 0)),
        Err(OfferError::Key(KeyError::Revoked))
    );

    // Signed by a stranger but naming the bench key: the bench key's check fails.
    let mut fields = signed(&k.stranger, 5, ImageKind::Replacement);
    fields.key_id = id(&k.bench);
    let bytes = manifest(&k.stranger, &fields);
    assert_eq!(
        admit_offer(&bytes, None, &context(&built_in, &[], &[], 0)),
        Err(OfferError::Signature)
    );
}

#[test]
fn a_tampered_or_swapped_manifest_fails_its_signature() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let ctx = context(&built_in, &[], &[], 0);
    let good = manifest(&k.bench, &signed(&k.bench, 5, ImageKind::Replacement));

    for at in [8, 12, 20, 33, 70, MANIFEST_BODY + 3] {
        let mut bad = good.clone();
        bad[at] ^= 1;
        let refused = admit_offer(&bad, None, &ctx);
        assert!(
            matches!(
                refused,
                Err(OfferError::Signature | OfferError::Kind | OfferError::Key(_))
            ),
            "byte {at}: {refused:?}"
        );
    }

    // Another image's hash under the first manifest's signature.
    let mut other = signed(&k.bench, 5, ImageKind::Replacement);
    other.manifest.image_sha256 = [0x77; 32];
    let mut swapped = manifest_body(&other).to_vec();
    swapped.extend_from_slice(&good[MANIFEST_BODY..]);
    assert_eq!(
        admit_offer(&swapped, None, &ctx),
        Err(OfferError::Signature)
    );
}

#[test]
fn version_one_bank_size_kind_and_the_running_build_are_refused() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let ctx = context(&built_in, &[], &[], 0);

    // The retired version 1 manifest: 148 bytes behind its own magic.
    let mut v1 = vec![0; 148];
    v1[..8].copy_from_slice(b"PLOTA1\0\0");
    assert_eq!(admit_offer(&v1, None, &ctx), Err(OfferError::Version));
    assert_eq!(
        admit_offer(&[0; MANIFEST_LEN - 1], None, &ctx),
        Err(OfferError::Length)
    );
    assert_eq!(
        admit_offer(&[0; MANIFEST_LEN], None, &ctx),
        Err(OfferError::Magic)
    );

    let mut fields = signed(&k.bench, 5, ImageKind::Replacement);
    fields.manifest.target_bank = 0;
    assert_eq!(
        admit_offer(&manifest(&k.bench, &fields), None, &ctx),
        Err(OfferError::Bank)
    );

    for len in [SECTOR_BYTES - 1, BANK1_BYTES + 1] {
        let mut fields = signed(&k.bench, 5, ImageKind::Replacement);
        fields.manifest.image_len = len;
        assert_eq!(
            admit_offer(&manifest(&k.bench, &fields), None, &ctx),
            Err(OfferError::Size)
        );
    }

    let mut body = manifest_body(&signed(&k.bench, 5, ImageKind::Replacement));
    body[20] = 9;
    assert_eq!(
        admit_offer(&sign(&k.bench, &body), None, &ctx),
        Err(OfferError::Kind)
    );
    body[20] = 1;
    body[22] = 1;
    assert_eq!(
        admit_offer(&sign(&k.bench, &body), None, &ctx),
        Err(OfferError::Kind)
    );

    let mut fields = signed(&k.bench, 5, ImageKind::Replacement);
    fields.manifest.build_id = RUNNING;
    assert_eq!(
        admit_offer(&manifest(&k.bench, &fields), None, &ctx),
        Err(OfferError::Same)
    );

    // Bank 0 installs any build into bank 1, its own included.
    let bank0 = Context {
        running: None,
        ..ctx
    };
    assert!(admit_offer(&manifest(&k.bench, &fields), None, &bank0).is_ok());
}

#[test]
fn a_replayed_or_older_manifest_needs_a_downgrade_statement() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let enrolled = [public(&k.owner)];
    let ctx = context(&built_in, &enrolled, &[], 9);

    for version in [1, 9] {
        let old = manifest(&k.bench, &signed(&k.bench, version, ImageKind::Replacement));
        assert_eq!(admit_offer(&old, None, &ctx), Err(OfferError::Downgrade));

        let allowed = downgrade(&k.owner, DEVICE, &old, 9);
        let admitted = admit_offer(&old, Some(&allowed), &ctx).unwrap();
        assert!(admitted.downgrade.is_some());
    }

    // Above the floor, a statement is not needed (and not looked at).
    let new = manifest(&k.bench, &signed(&k.bench, 10, ImageKind::Replacement));
    assert_eq!(
        admit_offer(&new, Some(&[0; 3]), &ctx).unwrap().downgrade,
        None
    );
}

#[test]
fn a_downgrade_statement_binds_the_feeder_the_manifest_and_the_floor() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let enrolled = [public(&k.owner)];
    let ctx = context(&built_in, &enrolled, &[], 9);
    let old = manifest(&k.bench, &signed(&k.bench, 3, ImageKind::Replacement));
    let refused = |statement: &[u8]| match admit_offer(&old, Some(statement), &ctx) {
        Err(OfferError::Statement(error)) => error,
        other => panic!("{other:?}"),
    };

    assert_eq!(
        refused(&downgrade(&k.owner, [0x01; 32], &old, 9)),
        StatementError::Device
    );
    assert_eq!(
        refused(&downgrade(&k.owner, DEVICE, &old, 8)),
        StatementError::Floor
    );
    let other = manifest(&k.bench, &signed(&k.bench, 4, ImageKind::Replacement));
    assert_eq!(
        refused(&downgrade(&k.owner, DEVICE, &other, 9)),
        StatementError::Digest
    );
    assert_eq!(
        refused(&downgrade(&k.stranger, DEVICE, &old, 9)),
        StatementError::Key(KeyError::Unknown)
    );

    let mut forged = downgrade(&k.owner, DEVICE, &old, 9);
    forged[30] ^= 1;
    assert_eq!(refused(&forged), StatementError::Signature);
    let good = downgrade(&k.owner, DEVICE, &old, 9);
    assert_eq!(refused(&good[1..]), StatementError::Length);

    // A key statement is not a downgrade statement, whatever it signs.
    let mut wrong = good.clone();
    wrong[..8].copy_from_slice(&REVOKE_MAGIC);
    assert_eq!(refused(&wrong), StatementError::Magic);

    let revoked = [id(&k.owner)];
    let ctx = context(&built_in, &enrolled, &revoked, 9);
    assert_eq!(
        admit_offer(&old, Some(&good), &ctx),
        Err(OfferError::Statement(StatementError::Key(
            KeyError::Revoked
        )))
    );
}

#[test]
fn stock_always_needs_a_downgrade_statement() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let ctx = context(&built_in, &[], &[], 0);
    let stock = manifest(&k.bench, &signed(&k.bench, 50, ImageKind::Stock));
    assert_eq!(admit_offer(&stock, None, &ctx), Err(OfferError::Downgrade));

    let allowed = downgrade(&k.bench, DEVICE, &stock, 0);
    let admitted = admit_offer(&stock, Some(&allowed), &ctx).unwrap();
    assert_eq!(admitted.signed.kind, ImageKind::Stock);
    assert!(admitted.downgrade.is_some());
}

#[test]
fn built_in_and_release_keys_sign_key_statements_and_owner_keys_never() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let enrolled = [public(&k.owner)];
    let ring = KeyRing {
        built_in: &built_in,
        added: Some(public(&k.release)),
        enrolled: &enrolled,
        revoked: &[],
    };

    let add = sign(&k.bench, &key_add_body(&id(&k.bench), &public(&k.release)));
    assert_eq!(
        admit_statement(&add, &ring),
        Ok(KeyStatement::Add(public(&k.release)))
    );
    let revoke = sign(&k.bench, &revoke_body(&id(&k.bench), &id(&k.owner)));
    assert_eq!(
        admit_statement(&revoke, &ring),
        Ok(KeyStatement::Revoke(id(&k.owner)))
    );

    // The owner's enrolled key never signs one.
    let statement = sign(&k.owner, &revoke_body(&id(&k.owner), &id(&k.bench)));
    assert_eq!(
        admit_statement(&statement, &ring),
        Err(StatementError::Key(KeyError::Unknown))
    );

    // The release key does: retiring the bench key leaves rotation to it.
    let retire_bench = sign(&k.release, &revoke_body(&id(&k.release), &id(&k.bench)));
    assert_eq!(
        admit_statement(&retire_bench, &ring),
        Ok(KeyStatement::Revoke(id(&k.bench)))
    );
    let rotate = sign(
        &k.release,
        &key_add_body(&id(&k.release), &public(&k.stranger)),
    );
    let bench_retired = [id(&k.bench)];
    let after = KeyRing {
        revoked: &bench_retired,
        ..ring
    };
    assert_eq!(
        admit_statement(&rotate, &after),
        Ok(KeyStatement::Add(public(&k.stranger)))
    );

    // Once retired itself, it signs nothing more.
    let release_retired = [id(&k.release)];
    let gone = KeyRing {
        revoked: &release_retired,
        ..ring
    };
    assert_eq!(
        admit_statement(&rotate, &gone),
        Err(StatementError::Key(KeyError::Revoked))
    );

    let retired = [id(&k.bench)];
    let ring = KeyRing {
        revoked: &retired,
        ..ring
    };
    assert_eq!(
        admit_statement(&add, &ring),
        Err(StatementError::Key(KeyError::Revoked))
    );
}

#[test]
fn malformed_key_statements_are_refused() {
    let k = keys();
    let built_in = [public(&k.bench)];
    let ring = KeyRing {
        built_in: &built_in,
        added: None,
        enrolled: &[],
        revoked: &[],
    };
    let revoke = sign(&k.bench, &revoke_body(&id(&k.bench), &[7; 8]));

    assert_eq!(admit_statement(&[], &ring), Err(StatementError::Length));
    assert_eq!(
        admit_statement(&revoke[..revoke.len() - 1], &ring),
        Err(StatementError::Length)
    );
    let mut forged = revoke.clone();
    forged[17] ^= 1;
    assert_eq!(
        admit_statement(&forged, &ring),
        Err(StatementError::Signature)
    );
    let mut magic = revoke.clone();
    magic[..8].copy_from_slice(&DOWNGRADE_MAGIC);
    assert_eq!(admit_statement(&magic, &ring), Err(StatementError::Magic));
}

/// A bank laid out as the reviewed builds are.
fn bank(km0: u32, km0_ram: u32, km4: u32, km4_ram: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let part = |out: &mut Vec<u8>, address: u32, body: u32| {
        out.extend_from_slice(&MARKER);
        out.extend_from_slice(&body.to_le_bytes());
        out.extend_from_slice(&address.to_le_bytes());
        out.extend_from_slice(&[0xff; 16]);
        out.extend(std::iter::repeat_n(0x42, body as usize));
    };
    part(&mut out, 0x0C00_0020, km0);
    part(&mut out, 0x0008_3000, km0_ram);
    out.resize(out.len().next_multiple_of(SECTOR), 0xff);
    part(&mut out, 0x0E00_0020, km4);
    part(&mut out, 0x1000_5000, km4_ram);
    part(&mut out, 0x0200_0020, 0);
    out.resize(out.len().next_multiple_of(SECTOR), 0xff);
    out
}

fn extent_of(bank: &[u8], limit: u32) -> Option<u32> {
    image::extent(
        |at, header| {
            let at = at as usize;
            let Some(bytes) = bank.get(at..at + 32) else {
                return false;
            };
            header.copy_from_slice(bytes);
            true
        },
        limit,
    )
}

#[test]
fn a_bank_s_extent_comes_from_its_five_headers() {
    let image = bank(5000, 300, 20_000, 700);
    let len = u32::try_from(image.len()).unwrap();
    assert_eq!(extent_of(&image, BANK1_BYTES), Some(len));
    assert_eq!(extent_of(&image, len - 1), None);

    let mut no_marker = image.clone();
    no_marker[SECTOR * 2] = b'0';
    assert_eq!(extent_of(&no_marker, BANK1_BYTES), None);

    let mut moved = image.clone();
    moved[12] ^= 1;
    assert_eq!(extent_of(&moved, BANK1_BYTES), None);

    // Erased flash has no extent.
    assert_eq!(extent_of(&[0xff; 4 * SECTOR], BANK1_BYTES), None);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_offer_or_statement_panics(
        manifest in proptest::collection::vec(any::<u8>(), 0..=MANIFEST_LEN + 8),
        statement in proptest::collection::vec(any::<u8>(), 0..=STATEMENT_MAX + 8),
        magic in 0usize..4,
    ) {
        let k = keys();
        let built_in = [public(&k.bench)];
        let ctx = context(&built_in, &[], &[], 3);
        let mut manifest = manifest;
        let mut statement = statement;
        if manifest.len() >= 8 {
            manifest[..8].copy_from_slice(&MAGIC);
        }
        if statement.len() >= 8 {
            let magics = [MAGIC, KEY_ADD_MAGIC, REVOKE_MAGIC, DOWNGRADE_MAGIC];
            statement[..8].copy_from_slice(&magics[magic]);
        }
        let _ = parse_manifest(&manifest);
        let _ = admit_offer(&manifest, Some(&statement), &ctx);
        let _ = admit_statement(&statement, &ctx.ring);
    }

    #[test]
    fn no_bank_contents_panic_the_extent(
        bytes in proptest::collection::vec(any::<u8>(), 0..3 * SECTOR),
        limit in any::<u32>(),
    ) {
        let _ = extent_of(&bytes, limit);
    }
}
