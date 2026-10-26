//! The device API's request decoding: the owner's open session, then each
//! length-byte-prefixed piece of the input sealed as one request frame.
#![no_main]

#[path = "../../product/wellfed-device-api/tests/support/mod.rs"]
mod support;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    support::session(&support::split(data));
});
