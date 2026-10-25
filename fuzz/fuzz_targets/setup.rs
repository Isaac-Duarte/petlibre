//! BLE setup, the feeder's side: any length-byte-prefixed messages to a
//! fresh responder, and a genuine client whose third message and Wi-Fi come
//! from the input.
#![no_main]

#[path = "../../src/product/wellfed-feeder/tests/support/mod.rs"]
mod support;

use wellfed_proto::setup::Wifi;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let messages = support::split(data);
    support::setup_messages(&messages);

    if let [request, codes, ssid, psk, ..] = messages[..] {
        let feeder = u16::from_le_bytes([codes.first().copied().unwrap_or(0), 0]);
        let client = u16::from_le_bytes([codes.get(1).copied().unwrap_or(0), 0]);
        let wifi = core::str::from_utf8(psk).ok().map(|psk| Wifi { ssid, psk });
        support::setup_client(request, feeder, client, wifi);
    }
});
