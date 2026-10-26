//! The device API from a peer that has not authenticated: any bytes on a
//! new connection, into the feeder's server (opening frame, IK, invite and
//! recovery handshakes).
#![no_main]

#[path = "../../product/wellfed-device-api/tests/support/mod.rs"]
mod support;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    support::connection(data);
    support::crowd(data);
});
