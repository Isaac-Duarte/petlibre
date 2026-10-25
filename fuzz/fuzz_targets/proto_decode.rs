//! The device protocol's codec: any bytes as every message either side
//! decodes, and whatever decodes re-encodes to the same value.
#![no_main]

use wellfed_proto::api::{
    ClientMessage, EventBody, FRAME_MAX, FeederMessage, Hello, Join, Joined, Open, Welcome,
};
use wellfed_proto::setup::{Answer, Reply, Request};
use wellfed_proto::{decode, decode_padded, encode};

macro_rules! stable {
    ($ty:ty, $bytes:expr) => {
        if let Ok(value) = decode::<$ty>($bytes) {
            let mut out = [0u8; 4 * FRAME_MAX];
            let len = encode(&value, &mut out).expect("a decoded message re-encodes");
            assert_eq!(decode::<$ty>(&out[..len]), Ok(value));
        }
    };
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    stable!(Open, data);
    stable!(Hello, data);
    stable!(Join<'_>, data);
    stable!(ClientMessage<'_>, data);
    stable!(Welcome<'_>, data);
    stable!(Joined, data);
    stable!(FeederMessage<'_>, data);
    stable!(Request<'_>, data);
    stable!(Answer<'_>, data);
    stable!(Reply, data);
    let _ = decode_padded::<EventBody>(data);
});
