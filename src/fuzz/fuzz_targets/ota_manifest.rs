//! The update path's parsers: any bytes as a version 2 manifest an offer
//! carries, and as chunks to a writer.
#![no_main]

use wellfed_core::ota::offer::{self, Context, KeyRing};
use wellfed_core::ota::{Manifest, Writer};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // No signer holds this key's secret, so nothing may be admitted.
    let built_in = [[0x5a; 32]];
    let context = Context {
        ring: KeyRing {
            built_in: &built_in,
            added: None,
            enrolled: &[],
            revoked: &[],
        },
        floor: 0,
        device: [0; 32],
        running: None,
    };
    assert!(offer::admit_offer(data, None, &context).is_err());
    if let Some(head) = data.get(..offer::MANIFEST_LEN) {
        assert!(offer::admit_offer(head, None, &context).is_err());
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
