use wellfed_core::mac::{KEY_LEN, hmac_sha256};

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
