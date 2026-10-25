//! The update path's parsers: any bytes as a manifest, as `ota/get` and
//! `ota/data` messages, and as chunks to a writer.
#![no_main]

use wellfed_core::ota::{
    MANIFEST_LEN, Manifest, Writer, decode_data, decode_get, encode_get, parse_manifest,
};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // No signer holds this key's secret, so nothing may parse.
    assert!(parse_manifest(data, &[0x5a; 32], 0).is_err());
    if let Some(head) = data.get(..MANIFEST_LEN) {
        assert!(parse_manifest(head, &[0x5a; 32], 0).is_err());
    }

    if let Some((part, offset, len)) = decode_get(data) {
        assert_eq!(encode_get(part, offset, len).as_slice(), data);
    }
    if let Some(chunk) = decode_data(data) {
        assert_eq!(chunk.bytes.len() + 10, data.len());
    }

    // Chunks: each piece's first two bytes pick its offset on the grid.
    let manifest = Manifest {
        version: 2,
        image_len: 0x3000,
        target_bank: 1,
        image_sha256: [0; 32],
        build_id: [0; 32],
    };
    let Some(mut writer) = Writer::new(manifest, 512) else {
        return;
    };
    for piece in data.chunks(515) {
        let [a, b, rest @ ..] = piece else {
            break;
        };
        let offset = u32::from(u16::from_le_bytes([*a, *b])) * 512 % 0x4000;
        if let Ok(Some(_)) = writer.accept(offset, rest) {
            let _ = writer.programmed(true);
        }
    }
    assert!(writer.commit().is_err());
});
