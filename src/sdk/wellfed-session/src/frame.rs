//! Length-prefixed frames: a u16 little-endian length, then that many
//! bytes. BLE setup's characteristic and the device API's TCP stream carry
//! the same framing, each with its own limit.

/// The length prefix.
pub const PREFIX: usize = 2;

/// The prefix for a `len`-byte frame; `None` above `max`.
pub fn prefix(len: usize, max: usize) -> Option<[u8; PREFIX]> {
    let len = u16::try_from(len).ok().filter(|&n| usize::from(n) <= max)?;
    Some(len.to_le_bytes())
}

/// The length a prefix announces; `None` above `max`.
pub fn parse_prefix(prefix: [u8; PREFIX], max: usize) -> Option<usize> {
    let len = usize::from(u16::from_le_bytes(prefix));
    (len <= max).then_some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_round_trip_and_bound_the_frame() {
        for len in [0, 1, 300, 448] {
            assert_eq!(
                prefix(len, 448).and_then(|p| parse_prefix(p, 448)),
                Some(len)
            );
        }
        assert_eq!(prefix(449, 448), None);
        assert_eq!(parse_prefix([0xff, 0xff], 448), None);
    }

    proptest::proptest! {
        /// Every length up to the limit round-trips; nothing above it is
        /// framed or announced.
        #[test]
        fn any_length_round_trips_within_its_limit(len in 0usize..70_000, max in 0usize..70_000) {
            let framed = prefix(len, max);
            proptest::prop_assert_eq!(framed.is_some(), len <= max && len <= usize::from(u16::MAX));
            if let Some(framed) = framed {
                proptest::prop_assert_eq!(parse_prefix(framed, max), Some(len));
            }
        }

        #[test]
        fn no_prefix_announces_more_than_the_limit(bytes: [u8; PREFIX], max in 0usize..70_000) {
            if let Some(len) = parse_prefix(bytes, max) {
                proptest::prop_assert!(len <= max);
                proptest::prop_assert_eq!(prefix(len, max), Some(bytes));
            }
        }
    }
}
