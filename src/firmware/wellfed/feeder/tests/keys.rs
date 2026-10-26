//! The identity in the loop: the public key derived from what the
//! settings writer publishes, once per generation, logged by fingerprint.

use wellfed_feeder::feeder::identity::{Identity, Record, describe};
use wellfed_feeder_runtime::keys;
use wellfed_fw_feeder::platform::Plaf103;

/// RFC 7748 section 6.1: Alice's private and public keys.
const SECRET: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
const PUBLIC: &str = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";

fn bytes(hex: &str) -> [u8; 32] {
    wellfed_session::hex::decode_array(hex.as_bytes()).unwrap()
}

#[test]
fn each_published_identity_is_derived_once() {
    keys::reset();
    let mut platform = Plaf103::new();
    let mut identity = Identity::default();
    assert_eq!(identity.step(&mut platform), None, "nothing published");
    assert!(identity.keypair().is_none());

    keys::publish_identity(&bytes(SECRET));
    let record = identity.step(&mut platform).expect("the new identity");
    assert_eq!(identity.keypair().unwrap().public(), &bytes(PUBLIC));
    assert_eq!(identity.step(&mut platform), None, "derived once");

    let mut text = String::new();
    describe(record, &mut text).unwrap();
    assert_eq!(
        text,
        format!(
            "identity generation={} fingerprint={}",
            record.generation,
            &PUBLIC[..16]
        )
    );

    keys::publish_identity(&[9; 32]);
    let Some(Record { generation, .. }) = identity.step(&mut platform) else {
        panic!("a reset's identity is taken too");
    };
    assert_ne!(generation, record.generation);
    assert_ne!(identity.keypair().unwrap().public(), &bytes(PUBLIC));
}
