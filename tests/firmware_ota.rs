//! FW10: the server's signing tool produces exactly what the firmware verifies.
use wellfed_core::ota::{self, ManifestError, SECTOR};
use wellfed_local::firmware_ota;

fn container(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; firmware_ota::CONTAINER_HEADER];
    let mut payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    payload[..8].copy_from_slice(&ota::MARKER);
    out.extend(payload);
    out
}

#[test]
fn keygen_sign_and_firmware_verify_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ota-key.json");
    let public = firmware_ota::keygen(&path).unwrap();
    assert!(
        firmware_ota::keygen(&path).is_err(),
        "keygen must not replace a key"
    );
    let key = firmware_ota::load_key(&path).unwrap();
    let image = container(2 * SECTOR + 17);
    let manifest = firmware_ota::sign(&key, &image, 9, [0x5a; 32]).unwrap();
    let mut public_bytes = [0u8; 32];
    for (i, b) in public_bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&public[2 * i..2 * i + 2], 16).unwrap();
    }
    let parsed = ota::parse_manifest(&manifest, &public_bytes, 8).unwrap();
    assert_eq!(
        (parsed.version, parsed.image_len, parsed.build_id),
        (9, (2 * SECTOR + 17) as u32, [0x5a; 32])
    );
    assert_eq!(
        ota::parse_manifest(&manifest, &public_bytes, 9),
        Err(ManifestError::Downgrade)
    );
    // The firmware's writer accepts the signed payload end to end.
    let payload = firmware_ota::payload(&image).unwrap();
    let mut writer = ota::Writer::new(parsed, 1024).unwrap();
    while let Some((offset, len)) = writer.next_request() {
        if writer
            .accept(offset, &payload[offset as usize..(offset + len) as usize])
            .unwrap()
            .is_some()
        {
            writer.programmed(true).unwrap();
        }
    }
    assert_eq!(writer.commit(), Ok((ota::BANK1_OFFSET, ota::MARKER)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn signing_refuses_images_the_firmware_would_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.json");
    firmware_ota::keygen(&path).unwrap();
    let key = firmware_ota::load_key(&path).unwrap();
    let mut no_marker = container(SECTOR);
    no_marker[firmware_ota::CONTAINER_HEADER] = b'0';
    assert!(firmware_ota::sign(&key, &no_marker, 1, [0; 32]).is_err());
    assert!(firmware_ota::sign(&key, &container(100), 1, [0; 32]).is_err());
    assert!(
        firmware_ota::sign(&key, &container(ota::BANK1_BYTES as usize + 1), 1, [0; 32]).is_err()
    );
    // A key file whose public half does not match its seed is refused.
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file["public_key"] = serde_json::Value::String("00".repeat(32));
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, serde_json::to_vec(&file).unwrap()).unwrap();
    assert!(firmware_ota::load_key(&bad).is_err());
}

#[test]
fn an_offer_answers_manifest_and_chunks_within_its_image() {
    use firmware_ota::Offer;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.json");
    firmware_ota::keygen(&path).unwrap();
    let key = firmware_ota::load_key(&path).unwrap();
    let image = container(SECTOR + 700);
    let manifest = firmware_ota::sign(&key, &image, 3, [1; 32]).unwrap();
    let offer = Offer::new(&manifest, &image).unwrap();
    assert_eq!(offer.version, 3);
    let answer = Offer::answer(Some(&offer), &ota::encode_get(ota::Part::Manifest, 0, 0)).unwrap();
    let data = ota::decode_data(&answer).unwrap();
    assert_eq!(
        (data.status, data.bytes),
        (ota::Status::Ok, manifest.as_slice())
    );
    let answer = Offer::answer(Some(&offer), &ota::encode_get(ota::Part::Chunk, 512, 512)).unwrap();
    let data = ota::decode_data(&answer).unwrap();
    assert_eq!(
        data.bytes,
        &firmware_ota::payload(&image).unwrap()[512..1024]
    );
    for (offset, len) in [(SECTOR as u32 + 600, 512), (0, 513), (0, 0)] {
        let answer = Offer::answer(
            Some(&offer),
            &ota::encode_get(ota::Part::Chunk, offset, len),
        )
        .unwrap();
        assert_eq!(
            ota::decode_data(&answer).unwrap().status,
            ota::Status::Range
        );
    }
    let none = Offer::answer(None, &ota::encode_get(ota::Part::Manifest, 0, 0)).unwrap();
    assert_eq!(
        ota::decode_data(&none).unwrap().status,
        ota::Status::NoOffer
    );
    assert!(Offer::answer(Some(&offer), &[1, 2]).is_none());
    // A manifest for another image is refused when loading the offer.
    let other = container(SECTOR + 701);
    assert!(Offer::new(&manifest, &other).is_err());
}

#[test]
fn an_offer_is_served_to_completion_once() {
    use firmware_ota::Offer;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("k.json");
    firmware_ota::keygen(&path).unwrap();
    let key = firmware_ota::load_key(&path).unwrap();
    let image = container(SECTOR + 700);
    let manifest = firmware_ota::sign(&key, &image, 3, [1; 32]).unwrap();
    let mut offer = Some(Offer::new(&manifest, &image).unwrap());
    let len = firmware_ota::payload(&image).unwrap().len() as u32;
    let get = |part, offset, n| ota::encode_get(part, offset, n);
    let status = |answer: Option<Vec<u8>>| ota::decode_data(&answer.unwrap()).unwrap().status;

    // A download interrupted before the end may start over.
    let (a, dropped) = Offer::serve(&mut offer, &get(ota::Part::Manifest, 0, 0));
    assert_eq!((status(a), dropped), (ota::Status::Ok, false));
    let (a, _) = Offer::serve(&mut offer, &get(ota::Part::Chunk, 0, 512));
    assert_eq!(status(a), ota::Status::Ok);
    let (_, dropped) = Offer::serve(&mut offer, &get(ota::Part::Manifest, 0, 0));
    assert!(!dropped && !offer.as_ref().unwrap().delivered);

    // The last chunk marks it delivered; re-asks of chunks are still served.
    let last = len - (len - 1) % 512 - 1;
    let (a, _) = Offer::serve(&mut offer, &get(ota::Part::Chunk, last, len - last));
    assert_eq!(status(a), ota::Status::Ok);
    assert!(offer.as_ref().unwrap().delivered);
    let (a, _) = Offer::serve(&mut offer, &get(ota::Part::Chunk, last, len - last));
    assert_eq!(status(a), ota::Status::Ok);

    // A new manifest request after delivery: the candidate fell back.
    let (a, dropped) = Offer::serve(&mut offer, &get(ota::Part::Manifest, 0, 0));
    assert_eq!((status(a), dropped), (ota::Status::NoOffer, true));
    assert!(offer.is_none());
}
