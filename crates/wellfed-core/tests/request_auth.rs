use wellfed_core::request_auth::{KEY_LEN, Session, hmac_sha256, parse_key, split};

fn padded(key: &[u8]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    out[..key.len()].copy_from_slice(key);
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn hmac_matches_rfc_4231() {
    // Keys shorter than the block are zero-padded by HMAC itself.
    assert_eq!(
        hex(&hmac_sha256(&padded(&[0x0b; 20]), &[b"Hi There"])),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        hex(&hmac_sha256(
            &padded(b"Jefe"),
            &[b"what do ya want ", b"for nothing?"]
        )),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

const KEY: [u8; KEY_LEN] = [7; KEY_LEN];
const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn sign(key: &[u8; KEY_LEN], prefix: &str) -> String {
    format!(
        "{prefix},\"mac\":\"{}\"}}",
        hex(&hmac_sha256(key, &[prefix.as_bytes()]))
    )
}

#[test]
fn a_signed_request_verifies_once_in_its_own_session() {
    let mut session = Session::new(KEY, b"entropy one");
    let nonce = core::str::from_utf8(&session.nonce_hex())
        .unwrap()
        .to_owned();
    assert_eq!(nonce, format!("{:016x}", session.nonce()));

    let prefix = format!(r#"{{"id":"{ID}","op":"ping","nonce":"{nonce}","seq":1"#);
    let payload = sign(&KEY, &prefix);
    let signed = split(payload.as_bytes()).unwrap();
    assert_eq!(signed.prefix, prefix.as_bytes());

    assert!(session.verify(&signed, &nonce, 1));
    assert!(
        !session.verify(&signed, &nonce, 1),
        "replay within the session"
    );

    let next = sign(&KEY, &prefix.replace("\"seq\":1", "\"seq\":5"));
    assert!(session.verify(&split(next.as_bytes()).unwrap(), &nonce, 5));

    // Another session (another nonce) refuses the same bytes.
    let mut other = Session::new(KEY, b"entropy two");
    assert_ne!(other.nonce(), session.nonce());
    assert!(!other.verify(&split(next.as_bytes()).unwrap(), &nonce, 6));
}

#[test]
fn wrong_keys_tampering_and_malformed_suffixes_are_refused() {
    let mut session = Session::new(KEY, b"x");
    let nonce = core::str::from_utf8(&session.nonce_hex())
        .unwrap()
        .to_owned();
    let prefix = format!(r#"{{"id":"{ID}","op":"time","nonce":"{nonce}","seq":1"#);

    let forged = sign(&[8; KEY_LEN], &prefix);
    assert!(!session.verify(&split(forged.as_bytes()).unwrap(), &nonce, 1));

    let mut tampered = sign(&KEY, &prefix).into_bytes();
    tampered[10] ^= 1;
    assert!(!session.verify(&split(&tampered).unwrap(), &nonce, 1));
    assert!(session.verify(&split(sign(&KEY, &prefix).as_bytes()).unwrap(), &nonce, 1));

    let good = sign(&KEY, &prefix);
    for bad in [
        good.replace("\"}", "\" }"),
        good.to_uppercase(),
        good[..good.len() - 3].to_owned() + "\"}",
        format!("{prefix}}}"),
        String::new(),
    ] {
        assert!(split(bad.as_bytes()).is_none(), "{bad}");
    }
    assert!(parse_key(b"00").is_none());
    assert_eq!(parse_key(hex(&KEY).as_bytes()), Some(KEY));
}
