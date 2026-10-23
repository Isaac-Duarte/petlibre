//! FW34 BLE setup: Wi-Fi credentials for a feeder that cannot reach its
//! network, over Bluetooth LE.
//!
//! Stock 1.1.15 does the same job with the vendor app; this replacement keeps
//! the role and uses the FW25 `wifi` request instead of the vendor protocol.
//! The feeder advertises one private service (UUIDs below) only while it has
//! been offline for [`OFFLINE_MS`], and its control characteristic (write,
//! notify) carries length-prefixed messages each way: a u16 little-endian
//! length, then that many bytes of JSON.
//!
//! 1. When the host enables notifications, the feeder draws a fresh
//!    `request_auth::Session` and sends a [`hello`] naming its nonce.
//! 2. The host sends one signed `wifi` request under that nonce, exactly as
//!    the server would over MQTT (`{"id":…,"op":"wifi","ssid":hex,"psk":hex,
//!    "nonce":…,"seq":…,"mac":…}`, or `"clear":1`).
//! 3. The feeder answers with the transport's own `wifi` reply.
//!
//! Every BLE request must be signed, reads included, because anyone in range
//! can connect. Only `wifi` is served; nothing over BLE can dispense food or
//! change any other setting.

/// The service and its control characteristic, as 128-bit UUIDs. The feeder
/// hands them to the stack little-endian ([`uuid_le`]).
pub const SERVICE: u128 = 0x504c_5345_5455_5000_8000_0000_0000_0000;
pub const CONTROL: u128 = SERVICE + 1;

/// The feeder advertises once it has been offline this long, as stock does
/// after about two minutes.
pub const OFFLINE_MS: u32 = 120_000;

/// The longest message either way: a signed `wifi` request at its limit.
pub const MAX_MESSAGE: usize = crate::transport::MAX_REQUEST;
/// The length prefix.
pub const PREFIX: usize = 2;

pub const fn uuid_le(uuid: u128) -> [u8; 16] {
    uuid.to_le_bytes()
}

/// The prefix for a `len`-byte message; `None` above [`MAX_MESSAGE`].
pub fn prefix(len: usize) -> Option<[u8; PREFIX]> {
    let len = u16::try_from(len)
        .ok()
        .filter(|&n| usize::from(n) <= MAX_MESSAGE)?;
    Some(len.to_le_bytes())
}

/// The length a prefix announces; `None` above [`MAX_MESSAGE`].
pub fn parse_prefix(prefix: [u8; PREFIX]) -> Option<usize> {
    let len = usize::from(u16::from_le_bytes(prefix));
    (len <= MAX_MESSAGE).then_some(len)
}

/// `{"op":"hello","nonce":"<16 hex>"}`.
pub const HELLO_LEN: usize = 41;

pub fn hello(nonce_hex: &[u8; 16]) -> [u8; HELLO_LEN] {
    let mut out = *b"{\"op\":\"hello\",\"nonce\":\"0000000000000000\"}";
    out[23..39].copy_from_slice(nonce_hex);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_round_trip_and_bound_the_message() {
        for len in [0, 1, 300, MAX_MESSAGE] {
            assert_eq!(prefix(len).and_then(parse_prefix), Some(len));
        }
        assert_eq!(prefix(MAX_MESSAGE + 1), None);
        assert_eq!(parse_prefix([0xff, 0xff]), None);
    }

    #[test]
    fn the_hello_names_the_nonce() {
        let hello = hello(b"0123456789abcdef");
        assert_eq!(&hello, br#"{"op":"hello","nonce":"0123456789abcdef"}"#);
        assert_eq!(
            uuid_le(CONTROL)[0],
            1,
            "the stack takes UUIDs little-endian"
        );
    }
}
