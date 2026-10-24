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
}
