//! The server answers `ota/get` from one offer, within its image.

mod common;

use common::{container, signed};
use ed25519_dalek::SigningKey;
use wellfed_core::ota::{self, SECTOR};
use wellfed_local::ota::firmware;

fn update(len: usize) -> Vec<u8> {
    container(len, |i| (i % 253) as u8)
}

#[test]
fn an_offer_answers_manifest_and_chunks_within_its_image() {
    use firmware::Offer;
    let key = SigningKey::from_bytes(&[3; 32]);

    let image = update(SECTOR + 700);
    let manifest = signed(&key, &image, 3, [1; 32]);
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
    assert_eq!(data.bytes, &firmware::payload(&image).unwrap()[512..1024]);

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
    let other = update(SECTOR + 701);
    assert!(Offer::new(&manifest, &other).is_err());
}

#[test]
fn an_offer_is_served_to_completion_once() {
    use firmware::Offer;
    let key = SigningKey::from_bytes(&[3; 32]);
    let image = update(SECTOR + 700);
    let manifest = signed(&key, &image, 3, [1; 32]);
    let mut offer = Some(Offer::new(&manifest, &image).unwrap());
    let len = firmware::payload(&image).unwrap().len() as u32;
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
