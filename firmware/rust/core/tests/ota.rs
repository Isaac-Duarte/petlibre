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
    while let Some((offset, len)) = w.next_request() {
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
}

#[test]
fn writer_refuses_disorder_bad_markers_readback_and_hash_and_never_commits() {
    let (signing, public) = key();
    let payload = image(2 * SECTOR + 10);
    let m = parse_manifest(&manifest_bytes(&signing, 2, &payload, 1), &public, 1).unwrap();

    let mut w = Writer::new(m, 1024).unwrap();
    assert_eq!(w.accept(1024, &payload[1024..2048]), Err(WriteError::Order));
    assert_eq!(w.commit(), Err(WriteError::State));

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
