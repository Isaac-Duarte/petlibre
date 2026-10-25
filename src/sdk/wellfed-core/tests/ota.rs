//! Tests for the update logic: signed manifest parsing (with an
//! RFC 8032 vector), the bank-1 sector writer, the boot-trial decision,
//! confirmation records and the delivery messages.
//!
//! The writer must withhold the marker until commit and refuse to commit
//! after any order, marker, readback or hash failure, so an incomplete image
//! can never be activated (design: docs/firmware/ota-design.md).

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use wellfed_core::ota::*;
use sha2::{Digest, Sha256};

const SEED: [u8; 32] = [7; 32];

fn key() -> (SigningKey, [u8; 32]) {
    let signing = SigningKey::from_bytes(&SEED);
    let public = signing.verifying_key().to_bytes();
    (signing, public)
}

fn image(len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    out[..8].copy_from_slice(&MARKER);
    out
}

fn manifest_bytes(signing: &SigningKey, version: u32, payload: &[u8], bank: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&MAGIC);
    body.extend_from_slice(&version.to_le_bytes());
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&bank.to_le_bytes());
    body.extend_from_slice(&Sha256::digest(payload));
    body.extend_from_slice(&[0xab; 32]);

    assert_eq!(body.len(), MANIFEST_BODY);
    let signature = signing.sign(&body).to_bytes();
    body.extend_from_slice(&signature);
    body
}

#[test]
fn manifest_body_matches_the_parser() {
    let (signing, public) = key();
    let payload = image(SECTOR);
    let bytes = manifest_bytes(&signing, 3, &payload, 1);
    let parsed = parse_manifest(&bytes, &public, 0).unwrap();
    assert_eq!(manifest_body(&parsed).as_slice(), &bytes[..MANIFEST_BODY]);
}

#[test]
fn rfc8032_vector_one_verifies() {
    // RFC 8032 section 7.1, test 1 (empty message).
    let public: [u8; 32] = hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
    let signature: [u8; 64] = hex(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    );

    let key = VerifyingKey::from_bytes(&public).unwrap();
    assert!(
        key.verify_strict(b"", &ed25519_dalek::Signature::from_bytes(&signature))
            .is_ok()
    );
}

fn hex<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

#[test]
fn manifest_needs_signature_bank_one_size_and_a_newer_version() {
    let (signing, public) = key();
    let payload = image(3 * SECTOR + 100);
    let good = manifest_bytes(&signing, 5, &payload, 1);
    let m = parse_manifest(&good, &public, 4).unwrap();
    assert_eq!(
        (m.version, m.image_len, m.target_bank),
        (5, payload.len() as u32, 1)
    );
    assert_eq!(
        m.image_sha256.as_slice(),
        Sha256::digest(&payload).as_slice()
    );

    assert_eq!(
        parse_manifest(&good, &public, 5),
        Err(ManifestError::Downgrade)
    );
    assert_eq!(
        parse_manifest(&good[1..], &public, 0),
        Err(ManifestError::Length)
    );

    let mut tampered = good.clone();
    tampered[12] ^= 1;
    assert_eq!(
        parse_manifest(&tampered, &public, 0),
        Err(ManifestError::Signature)
    );

    let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    assert_eq!(
        parse_manifest(&good, &other, 0),
        Err(ManifestError::Signature)
    );

    let mut magic = good.clone();
    magic[0] = b'X';
    assert_eq!(
        parse_manifest(&magic, &public, 0),
        Err(ManifestError::Magic)
    );

    assert_eq!(
        parse_manifest(&manifest_bytes(&signing, 5, &payload, 0), &public, 0),
        Err(ManifestError::Bank)
    );
    assert_eq!(
        parse_manifest(&manifest_bytes(&signing, 5, &payload[..100], 1), &public, 0),
        Err(ManifestError::Size)
    );

    let big = image(BANK1_BYTES as usize + 1);
    assert_eq!(
        parse_manifest(&manifest_bytes(&signing, 5, &big, 1), &public, 0),
        Err(ManifestError::Size)
    );
}

/// Drive a writer to completion; `flash` models bank 1 (offset 0 = BANK1_OFFSET).
fn write_all(
    w: &mut Writer,
    payload: &[u8],
    flash: &mut Vec<u8>,
    bad_readback: Option<u32>,
) -> Result<(), WriteError> {
    loop {
        let wanted: Vec<_> = w.missing().collect();
        if wanted.is_empty() {
            break;
        }

        // Last chunk first: a sector's chunks may arrive in any order.
        for (offset, len) in wanted.into_iter().rev() {
            let chunk = &payload[offset as usize..(offset + len) as usize];
            if let Some(program) = w.accept(offset, chunk)? {
                let at = (program.offset - BANK1_OFFSET) as usize;
                if flash.len() < at + program.data.len() {
                    flash.resize(at + program.data.len(), 0xff);
                }
                flash[at..at + program.data.len()].copy_from_slice(program.data);
                let ok = bad_readback != Some(program.offset);
                w.programmed(ok)?;
            }
        }
    }

    Ok(())
}

#[test]
fn writer_stages_every_sector_with_the_marker_withheld_until_commit() {
    let (signing, public) = key();
    for chunk in [512, 1024, SECTOR] {
        let payload = image(3 * SECTOR + 100);
        let m = parse_manifest(&manifest_bytes(&signing, 2, &payload, 1), &public, 1).unwrap();
        let mut w = Writer::new(m, chunk).unwrap();
        let mut flash = Vec::new();
        write_all(&mut w, &payload, &mut flash, None).unwrap();

        // Everything but the marker is in place; the marker bytes are erased.
        assert_eq!(&flash[..8], &[0xff; 8]);
        assert_eq!(&flash[8..], &payload[8..]);
        assert_eq!(w.commit(), Ok((BANK1_OFFSET, MARKER)));
    }

    assert!(
        Writer::new(
            parse_manifest(&manifest_bytes(&signing, 2, &image(SECTOR), 1), &public, 1).unwrap(),
            3000
        )
        .is_none()
    );
    // More chunks per sector than the received bitmap tracks.
    assert!(
        Writer::new(
            parse_manifest(&manifest_bytes(&signing, 2, &image(SECTOR), 1), &public, 1).unwrap(),
            64
        )
        .is_none()
    );
}

#[test]
fn writer_refuses_disorder_bad_markers_readback_and_hash_and_never_commits() {
    let (signing, public) = key();
    let payload = image(2 * SECTOR + 10);
    let m = parse_manifest(&manifest_bytes(&signing, 2, &payload, 1), &public, 1).unwrap();

    // Only the staged sector's chunks, each once.
    let mut w = Writer::new(m, 1024).unwrap();
    let next = &payload[SECTOR..SECTOR + 1024];
    assert_eq!(w.accept(SECTOR as u32, next), Err(WriteError::Order));
    assert_eq!(w.commit(), Err(WriteError::State));

    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(w.accept(1024, &payload[1024..2048]), Ok(None));
    assert_eq!(w.accept(1024, &payload[1024..2048]), Err(WriteError::Order));

    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(w.accept(512, &payload[512..1536]), Err(WriteError::Order));

    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(w.accept(0, &payload[..1000]), Err(WriteError::Order));

    let mut no_marker = payload.clone();
    no_marker[0] = b'0';
    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(w.accept(0, &no_marker[..1024]), Err(WriteError::Marker));

    let mut w = Writer::new(m, 1024).unwrap();
    let result = write_all(
        &mut w,
        &payload,
        &mut Vec::new(),
        Some(BANK1_OFFSET + SECTOR as u32),
    );
    assert_eq!(result, Err(WriteError::Readback));
    assert_eq!(w.failed(), Some(WriteError::Readback));
    assert_eq!(w.commit(), Err(WriteError::State));

    // A corrupted byte after the marker passes every chunk check, then fails the hash.
    let mut corrupt = payload.clone();
    corrupt[SECTOR + 5] ^= 0x10;
    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(
        write_all(&mut w, &corrupt, &mut Vec::new(), None),
        Err(WriteError::Hash)
    );
    assert_eq!(w.commit(), Err(WriteError::State));

    // Truncated: the writer keeps asking for the rest and cannot commit.
    let mut w = Writer::new(m, 1024).unwrap();
    for offset in (0..SECTOR as u32).step_by(1024) {
        let _ = w
            .accept(offset, &payload[offset as usize..offset as usize + 1024])
            .unwrap();
    }
    w.programmed(true).unwrap();
    assert_eq!(w.next_request(), Some((SECTOR as u32, 1024)));
    assert_eq!(w.commit(), Err(WriteError::State));
}

#[test]
fn trial_counts_unconfirmed_boots_then_gives_way() {
    assert_eq!(trial(true, 9, 3), Trial::Run);
    assert_eq!(trial(false, 0, 3), Trial::Count(1));
    assert_eq!(trial(false, 2, 3), Trial::Count(3));
    assert_eq!(trial(false, 3, 3), Trial::GiveWay);
    assert_eq!(trial(false, 0, 0), Trial::Count(1));
    assert_eq!(trial(false, 1, 0), Trial::GiveWay);
}

#[test]
fn confirmation_names_exactly_one_image() {
    let sha = [3u8; 32];
    let record = confirm_record(&sha, 7);
    assert_eq!(confirmed(&record, &sha), Some(7));
    assert_eq!(confirmed(&record, &[4u8; 32]), None);
    assert_eq!(confirmed(&record[1..], &sha), None);

    let mut bad = record;
    bad[0] = b'X';
    assert_eq!(confirmed(&bad, &sha), None);
}

#[test]
fn delivery_messages_round_trip_and_reject_malformed() {
    let get = encode_get(Part::Chunk, 4096, 1024);
    assert_eq!(decode_get(&get), Some((Part::Chunk, 4096, 1024)));
    assert_eq!(
        decode_get(&encode_get(Part::Manifest, 0, 0)),
        Some((Part::Manifest, 0, 0))
    );

    assert_eq!(decode_get(&get[..8]), None);
    let mut bad = get;
    bad[0] = 9;
    assert_eq!(decode_get(&bad), None);

    let mut data = encode_data_header(Part::Chunk, Status::Ok, 4096, 3).to_vec();
    data.extend_from_slice(&[1, 2, 3]);
    assert_eq!(
        decode_data(&data),
        Some(Data {
            part: Part::Chunk,
            status: Status::Ok,
            offset: 4096,
            bytes: &[1, 2, 3]
        })
    );

    // A declared length that disagrees with the body is refused.
    assert_eq!(decode_data(&data[..data.len() - 1]), None);

    let refused = encode_data_header(Part::Chunk, Status::Range, 0, 0);
    assert_eq!(decode_data(&refused).map(|d| d.status), Some(Status::Range));
    let mut refused_with_body = encode_data_header(Part::Chunk, Status::NoOffer, 0, 1).to_vec();
    refused_with_body.push(0);
    assert_eq!(decode_data(&refused_with_body), None);
    assert_eq!(decode_data(&[0; 3]), None);
}

/// Bounded properties of what an update server, or anyone posing as one,
/// can send: manifests, delivery messages and chunks.
mod properties {
    use super::*;
    use proptest::prelude::*;

    fn signed(signing: &SigningKey, manifest: &Manifest) -> Vec<u8> {
        let mut bytes = manifest_body(manifest).to_vec();
        bytes.extend_from_slice(&signing.sign(&bytes).to_bytes());
        bytes
    }

    fn manifest() -> impl Strategy<Value = Manifest> {
        (
            any::<u32>(),
            prop_oneof![any::<u32>(), SECTOR_BYTES - 1..=BANK1_BYTES + 1],
            prop_oneof![Just(1u32), any::<u32>()],
            any::<[u8; 32]>(),
            any::<[u8; 32]>(),
        )
            .prop_map(
                |(version, image_len, target_bank, image_sha256, build_id)| Manifest {
                    version,
                    image_len,
                    target_bank,
                    image_sha256,
                    build_id,
                },
            )
    }

    fn part() -> impl Strategy<Value = Part> {
        prop_oneof![Just(Part::Manifest), Just(Part::Chunk)]
    }

    fn status() -> impl Strategy<Value = Status> {
        prop_oneof![Just(Status::Ok), Just(Status::NoOffer), Just(Status::Range)]
    }

    proptest! {
        /// Unsigned bytes, magic or not, never parse.
        #[test]
        fn unsigned_manifests_never_parse(
            magic in any::<bool>(),
            mut bytes in prop_oneof![
                proptest::collection::vec(any::<u8>(), 0..2 * MANIFEST_LEN),
                proptest::collection::vec(any::<u8>(), MANIFEST_LEN),
            ],
            installed in any::<u32>(),
        ) {
            if magic && bytes.len() >= MAGIC.len() {
                bytes[..MAGIC.len()].copy_from_slice(&MAGIC);
            }
            let (_, public) = key();
            prop_assert!(parse_manifest(&bytes, &public, installed).is_err());
        }

        /// A signed manifest parses back to itself exactly when it targets
        /// bank 1, fits it and is newer; otherwise it names why not.
        #[test]
        fn signed_manifests_round_trip_or_name_the_refusal(
            manifest in manifest(),
            installed in any::<u32>(),
        ) {
            let (signing, public) = key();
            let expected = if manifest.target_bank != 1 {
                Err(ManifestError::Bank)
            } else if !(SECTOR_BYTES..=BANK1_BYTES).contains(&manifest.image_len) {
                Err(ManifestError::Size)
            } else if manifest.version <= installed {
                Err(ManifestError::Downgrade)
            } else {
                Ok(manifest)
            };
            prop_assert_eq!(parse_manifest(&signed(&signing, &manifest), &public, installed), expected);
        }

        /// One flipped bit anywhere in a signed manifest is refused.
        #[test]
        fn a_flipped_manifest_bit_is_refused(
            manifest in manifest(),
            bit in 0..MANIFEST_LEN * 8,
        ) {
            let (signing, public) = key();
            let mut bytes = signed(&signing, &manifest);
            bytes[bit / 8] ^= 1 << (bit % 8);
            let refusal = parse_manifest(&bytes, &public, 0);
            prop_assert!(
                matches!(refusal, Err(ManifestError::Magic | ManifestError::Signature)),
                "{refusal:?}"
            );
        }

        #[test]
        fn delivery_messages_round_trip(
            part in part(),
            status in status(),
            offset in any::<u32>(),
            len in any::<u32>(),
            body in proptest::collection::vec(any::<u8>(), 0..=MAX_CHUNK as usize),
        ) {
            prop_assert_eq!(decode_get(&encode_get(part, offset, len)), Some((part, offset, len)));

            let body = if status == Status::Ok { body } else { Vec::new() };
            let mut bytes = encode_data_header(part, status, offset, u32::try_from(body.len()).unwrap()).to_vec();
            bytes.extend_from_slice(&body);
            prop_assert_eq!(
                decode_data(&bytes),
                Some(Data { part, status, offset, bytes: &body })
            );
        }

        /// Arbitrary delivery bytes decode only when their declared length
        /// is exactly what follows the header.
        #[test]
        fn delivery_decoders_survive_any_bytes(
            bytes in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            if let Some((part, offset, len)) = decode_get(&bytes) {
                let encoded = encode_get(part, offset, len);
                prop_assert_eq!(encoded.as_slice(), bytes.as_slice());
            }
            if let Some(data) = decode_data(&bytes) {
                prop_assert_eq!(data.bytes.len(), bytes.len() - DATA_HEADER);
                prop_assert!(data.status == Status::Ok || data.bytes.is_empty());
            }
        }

        /// Any image, chunked any way the bitmap tracks, its chunks arriving
        /// in any order within a sector, lands intact with the marker
        /// withheld until commit.
        #[test]
        fn any_image_in_any_chunk_order_lands_intact(
            len in SECTOR..3 * SECTOR,
            fill in any::<u8>(),
            chunk in proptest::sample::select(vec![128usize, 256, 512, 1024, 2048, SECTOR]),
            seed in any::<u64>(),
        ) {
            let mut payload: Vec<u8> =
                (0..len).map(|i| fill.wrapping_add((i * 131 % 251) as u8)).collect();
            payload[..MARKER.len()].copy_from_slice(&MARKER);
            let manifest = Manifest {
                version: 2,
                image_len: u32::try_from(len).unwrap(),
                target_bank: 1,
                image_sha256: Sha256::digest(&payload).into(),
                build_id: [0; 32],
            };
            let mut w = Writer::new(manifest, chunk).unwrap();
            let mut flash = vec![0xff; len];
            let mut state = seed | 1;

            loop {
                let mut wanted: Vec<_> = w.missing().collect();
                if wanted.is_empty() {
                    break;
                }
                // A seeded shuffle, so a failure replays.
                for i in (1..wanted.len()).rev() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    wanted.swap(i, usize::try_from(state % (i as u64 + 1)).unwrap());
                }
                for (offset, len) in wanted {
                    let data = &payload[offset as usize..(offset + len) as usize];
                    if let Some(program) = w.accept(offset, data).unwrap() {
                        let at = (program.offset - BANK1_OFFSET) as usize;
                        flash[at..at + program.data.len()].copy_from_slice(program.data);
                        w.programmed(true).unwrap();
                    }
                }
            }

            prop_assert_eq!(&flash[..MARKER.len()], &[0xff; 8]);
            prop_assert_eq!(&flash[MARKER.len()..], &payload[MARKER.len()..]);
            prop_assert_eq!(w.commit(), Ok((BANK1_OFFSET, MARKER)));
        }

        /// Whatever chunks arrive, in whatever order, the writer never
        /// panics and never commits an image whose hash it did not see.
        #[test]
        fn the_writer_survives_any_chunks(
            image_len in SECTOR_BYTES..=BANK1_BYTES,
            calls in proptest::collection::vec(
                (any::<u32>(), 0usize..SECTOR + 2, any::<u8>(), any::<bool>()),
                0..24,
            ),
        ) {
            let manifest = Manifest {
                version: 2,
                image_len,
                target_bank: 1,
                image_sha256: [0; 32],
                build_id: [0; 32],
            };
            let mut w = Writer::new(manifest, 512).unwrap();
            for (offset, len, byte, readback) in calls {
                // Bias half the offsets onto the chunk grid, where the
                // writer's arithmetic happens.
                let offset = if byte & 1 == 0 { offset % (2 * SECTOR_BYTES) / 512 * 512 } else { offset };
                let data = vec![byte; len];
                if let Ok(Some(_)) = w.accept(offset, &data) {
                    let _ = w.programmed(readback);
                }
                let _ = w.next_request();
            }
            prop_assert!(w.commit().is_err());
        }
    }
}
