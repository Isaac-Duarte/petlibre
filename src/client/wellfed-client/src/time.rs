//! The clock sync as the device API carries it (`Op::TimeSync`): the
//! wall clock, the zone's offset now, and its next change, so the feeder
//! keeps local time across a DST change without another sync.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_model::TimeSync;

/// The sync for `zone` at `now`. A zone with no change ahead (UTC, or one
/// without DST) sends a next change of 0 with the current offset.
pub fn sync(now: Timestamp, zone: &TimeZone) -> TimeSync {
    let offset_s = zone.to_offset(now).seconds();
    let (next_change_s, next_offset_s) = zone
        .following(now)
        .next()
        .and_then(|transition| {
            let at = u64::try_from(transition.timestamp().as_second()).ok()?;
            Some((at, transition.offset().seconds()))
        })
        .unwrap_or((0, offset_s));
    TimeSync {
        unix_ms: u64::try_from(now.as_millisecond()).unwrap_or(0),
        offset_s,
        next_change_s,
        next_offset_s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same case `wellfed-local` pinned for the MQTT request.
    #[test]
    fn denver_carries_its_next_change_and_utc_none() {
        let now = Timestamp::from_millisecond(1_792_674_000_000).unwrap();
        let denver = TimeZone::get("America/Denver").unwrap();
        assert_eq!(
            sync(now, &denver),
            TimeSync {
                unix_ms: 1_792_674_000_000,
                offset_s: -21_600,
                next_change_s: 1_793_520_000,
                next_offset_s: -25_200,
            }
        );
        let utc = sync(now, &TimeZone::UTC);
        assert_eq!(
            (utc.offset_s, utc.next_change_s, utc.next_offset_s),
            (0, 0, 0)
        );
    }
}
