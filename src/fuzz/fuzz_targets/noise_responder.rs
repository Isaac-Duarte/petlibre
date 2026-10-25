//! The Noise handshake reader: the first byte picks a pattern and the
//! payload buffer's size, the rest is the peer's first message.
#![no_main]

use wellfed_core::noise::{Handshake, Keypair, Pattern, Role};

const PATTERNS: [Pattern; 4] = [Pattern::Xx, Pattern::Ik, Pattern::Kk, Pattern::XxPsk3];

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((&pick, message)) = data.split_first() else {
        return;
    };
    let pattern = PATTERNS[usize::from(pick & 3)];
    let identity = Keypair::from_secret([0x22; 32]);
    let peer = *Keypair::from_secret([0x11; 32]).public();
    let remote = (pattern == Pattern::Kk).then_some(peer);
    let Ok(mut responder) = Handshake::new(
        pattern,
        Role::Responder,
        b"fuzz",
        &identity,
        remote,
        [0x44; 32],
    ) else {
        return;
    };
    responder.set_psk([0x66; 32]);

    let mut payload = vec![0; usize::from(pick >> 2) * 8];
    if let Ok(read) = responder.read_message(message, &mut payload) {
        assert!(read <= payload.len());
        let mut reply = [0; 256];
        let _ = responder.write_message(&[], &mut reply);
    }
});
