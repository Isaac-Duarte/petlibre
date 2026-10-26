//! Time for the feeder: the clock sync as the device API carries it
//! (`Op::TimeSync`), and the zone setting's rule (`Op::PutTime`).
//!
//! A sync carries the wall clock, the zone's offset now and its next
//! change, so a feeder without a zone setting keeps local time across one
//! DST change. The zone setting is better: a POSIX TZ rule, which the
//! feeder evaluates for any year without a zone database. The rule is the
//! footer of the zone's TZif file (RFC 8536) in the tzdb bundled here, so
//! the app, the CLI and the server send the same rule for the same name
//! whatever their host's own zone files say.
//!
//! A footer describes the zone only after the file's last listed
//! transition. A few zones list irregular changes years ahead (Morocco's
//! Ramadan changes run to 2087), so a rule is offered only once it agrees
//! with the full zone at every transition of the next ten years; such a
//! zone stays on the syncs' offsets.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_core::clock::zone::{Rule, Zone};
use wellfed_model::TimeSync;

/// How far ahead a rule must agree with its zone.
const AGREE_S: i64 = 10 * 366 * 86_400;

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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuleError {
    #[error("{0:?} is not a time zone in the bundled tzdb")]
    Unknown(String),
    #[error("{0}'s zone file has no POSIX rule")]
    NoRule(String),
    #[error("{name}'s rule {rule:?} is not one the feeder keeps")]
    Unsupported { name: String, rule: String },
    #[error("{name} changes irregularly at {at}: its rule {rule:?} does not describe it")]
    Irregular {
        name: String,
        rule: String,
        at: Timestamp,
    },
}

/// The POSIX TZ rule the feeder keeps for the IANA zone `name` from
/// `now`, checked by the feeder's own parser, against the bundled zone and
/// against `host`, the host's own view of the zone if it has one (the
/// caller reads it: this crate does no IO).
pub fn rule(name: &str, now: Timestamp, host: Option<&TimeZone>) -> Result<String, RuleError> {
    let (canonical, tzif) =
        jiff_tzdb::get(name).ok_or_else(|| RuleError::Unknown(name.to_owned()))?;
    let footer = footer(tzif).ok_or_else(|| RuleError::NoRule(canonical.to_owned()))?;
    let rule = String::from_utf8_lossy(footer).into_owned();
    let Ok(zone) = Zone::parse(footer) else {
        return Err(RuleError::Unsupported {
            name: canonical.to_owned(),
            rule,
        });
    };

    // Against the bundled zone and the host's (which `sync` uses): two
    // tzdb releases can disagree about a zone's coming changes, and the
    // feeder must not keep a rule either one contradicts.
    let bundled =
        TimeZone::tzif(canonical, tzif).map_err(|_| RuleError::NoRule(canonical.to_owned()))?;
    for full in core::iter::once(&bundled).chain(host) {
        if let Some(at) = disagreement(zone.rule(), full, now) {
            return Err(RuleError::Irregular {
                name: canonical.to_owned(),
                rule,
                at,
            });
        }
    }
    Ok(rule)
}

/// The first instant in the next ten years where `rule` and `zone` differ,
/// checked at `now` and on both sides of each of the zone's transitions.
fn disagreement(rule: Rule, zone: &TimeZone, now: Timestamp) -> Option<Timestamp> {
    let end = now.as_second().saturating_add(AGREE_S);
    let differs = |t: Timestamp| rule.offset_at(t.as_second()) != zone.to_offset(t).seconds();
    if differs(now) {
        return Some(now);
    }
    for transition in zone.following(now) {
        let t = transition.timestamp();
        if t.as_second() > end {
            break;
        }
        let before = Timestamp::from_second(t.as_second() - 1).ok()?;
        if differs(before) {
            return Some(before);
        }
        if differs(t) {
            return Some(t);
        }
    }
    None
}

/// A version 2+ TZif file ends with its POSIX rule between two newlines.
fn footer(tzif: &[u8]) -> Option<&[u8]> {
    let text = tzif.strip_suffix(b"\n")?;
    let start = text.iter().rposition(|&b| b == b'\n')? + 1;
    let footer = &text[start..];
    (!footer.is_empty()).then_some(footer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_core::clock::zone::TEXT_MAX;

    // 2026-10-26T00:00:00Z.
    const NOW: i64 = 1_793_059_200;

    /// The case the server's former time request pinned.
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

    #[test]
    fn rules_come_from_the_bundled_tzdb() {
        let now = Timestamp::from_second(NOW).unwrap();
        assert_eq!(
            rule("America/Denver", now, None).unwrap(),
            "MST7MDT,M3.2.0,M11.1.0"
        );
        assert_eq!(rule("Asia/Kolkata", now, None).unwrap(), "IST-5:30");
        assert_eq!(
            rule("Mars/Olympus", now, None),
            Err(RuleError::Unknown("Mars/Olympus".to_owned()))
        );
    }

    /// A rule that misses a change the zone lists is caught at it.
    #[test]
    fn a_rule_that_misses_a_change_disagrees_there() {
        let now = Timestamp::from_second(NOW).unwrap();
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let winter_only = Zone::parse(b"CET-1").unwrap().rule();
        // 2027-03-28T01:00:00Z, when Berlin moves to summer time.
        assert_eq!(
            disagreement(winter_only, &berlin, now),
            Some(Timestamp::from_second(1_806_195_600).unwrap())
        );
        let full = Zone::parse(b"CET-1CEST,M3.5.0,M10.5.0/3").unwrap().rule();
        assert_eq!(disagreement(full, &berlin, now), None);
    }

    /// Every zone's rule fits the feeder and agrees with jiff at every
    /// transition of the next ten years, but for the few zones that list
    /// irregular changes ahead.
    #[test]
    fn every_zone_rule_fits_and_agrees_with_jiff() {
        let now = Timestamp::from_second(NOW).unwrap();
        let (mut regular, mut irregular) = (0, Vec::new());
        for name in jiff_tzdb::available() {
            let host = TimeZone::get(name).ok();
            match rule(name, now, host.as_ref()) {
                Ok(rule) => {
                    assert!(rule.len() <= TEXT_MAX, "{name}: {rule}");
                    regular += 1;
                }
                Err(RuleError::Irregular { name, .. }) => irregular.push(name),
                Err(error) => panic!("{error}"),
            }
        }
        assert!(regular > 300, "{regular} zones");
        assert!(irregular.len() <= 4, "{irregular:?}");
    }
}
