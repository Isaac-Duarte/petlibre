//! FW45 BLE setup mode's GATT layout and framing.
//!
//! In setup mode the feeder advertises one private service (UUIDs below)
//! whose control characteristic (write, notify) carries length-prefixed
//! messages each way: a u16 little-endian length, then that many bytes. The
//! messages are a Noise XX handshake and the setup exchange sealed after it
//! (`petlibre_proto::setup`); the feeder's link (`petlibre_hal::ble`) and the
//! host client (`petlibre-client`) share this framing.
//!
//! Stock 1.1.15 uses BLE for the same job with the vendor app; nothing over
//! BLE can dispense food or change anything but Wi-Fi and the client list.

/// The service and its control characteristic, as 128-bit UUIDs. The feeder
/// hands them to the stack little-endian ([`uuid_le`]).
pub const SERVICE: u128 = 0x504c_5345_5455_5000_8000_0000_0000_0000;
pub const CONTROL: u128 = SERVICE + 1;

/// The longest message either way. The longest is the handshake's third
/// message: two sealed fields and a request with the longest label and
/// credentials, well under this.
pub const MAX_MESSAGE: usize = 448;
/// The length prefix.
pub const PREFIX: usize = crate::frame::PREFIX;

pub const fn uuid_le(uuid: u128) -> [u8; 16] {
    uuid.to_le_bytes()
}

/// The prefix for a `len`-byte message; `None` above [`MAX_MESSAGE`].
pub fn prefix(len: usize) -> Option<[u8; PREFIX]> {
    crate::frame::prefix(len, MAX_MESSAGE)
}

/// The length a prefix announces; `None` above [`MAX_MESSAGE`].
pub fn parse_prefix(prefix: [u8; PREFIX]) -> Option<usize> {
    crate::frame::parse_prefix(prefix, MAX_MESSAGE)
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
    fn uuids_go_to_the_stack_little_endian() {
        assert_eq!(uuid_le(CONTROL)[0], 1);
    }
}
