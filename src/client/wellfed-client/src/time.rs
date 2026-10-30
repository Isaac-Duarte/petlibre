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
//!
//! A plan's next meal ([`next_meal`]) is the feeder's own reckoning
//! (`wellfed_core::schedule`) run on the host's zone, so a skipped or
//! repeated minute lands where the feeder will feed.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_core::clock::zone::{Rule, Zone};
use wellfed_core::schedule;
use wellfed_model::{NextMeal, Plan, Plans, TimeSync, TimeView};

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

/// The earliest occurrence after `now` of any enabled plan in `plans`, in
/// `zone`; `None` when no plan is enabled. A tie goes to the lower slot,
/// which the feeder runs first.
pub fn next_meal(plans: &Plans, now: Timestamp, zone: &TimeZone) -> Option<NextMeal> {
    let now_ms = u64::try_from(now.as_millisecond()).ok()?;
    let offset_at = |ms: u64| {
        let at = Timestamp::from_millisecond(i64::try_from(ms).ok()?).ok()?;
        Some(zone.to_offset(at).seconds())
    };

    let mut next: Option<NextMeal> = None;
    for (slot, plan) in plans.plans.iter().enumerate() {
        let Some(plan) = plan.filter(|plan| plan.enabled) else {
            continue;
        };
        let Some(due) = schedule::next_due(&core_plan(&plan), now_ms, offset_at) else {
            continue;
        };
        let Some(at) = i64::try_from(due)
            .ok()
            .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        else {
            continue;
        };
        if next.is_none_or(|next| at < next.at) {
            next = Some(NextMeal {
                at,
                slot: u8::try_from(slot).ok()?,
                portions: plan.portions,
            });
        }
    }
    next
}

/// The feeder's zone as its time settings give it: its POSIX rule, or the
/// offset a sync left it, or `fallback` (a host's own zone) with neither, when
/// the feeder has no idea of local time and the host's is the best guess.
pub fn feeder_zone(view: &TimeView, fallback: TimeZone) -> TimeZone {
    if let Some(zone) = view
        .zone
        .as_deref()
        .and_then(|rule| TimeZone::posix(rule).ok())
    {
        return zone;
    }
    view.offset_s
        .and_then(|seconds| jiff::tz::Offset::from_seconds(seconds).ok())
        .map_or(fallback, TimeZone::fixed)
}

/// The model's plan as the feeder's schedule holds it: the same fields,
/// and the same weekday bits.
const fn core_plan(plan: &Plan) -> schedule::Plan {
    schedule::Plan {
        minute: plan.minute,
        portions: plan.portions,
        days: schedule::Weekdays(plan.days.bits()),
        enabled: plan.enabled,
        call: plan.call,
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
    use wellfed_model::{Generation, PLANS, Weekdays};

    // 2026-10-26T00:00:00Z.
    const NOW: i64 = 1_793_059_200;
    /// Denver: 2026-03-08 02:00 MST jumps to 03:00 MDT at 09:00Z.
    const SPRING: i64 = 1_772_960_400;
    /// Denver: 2026-11-01 02:00 MDT falls back to 01:00 MST at 08:00Z.
    const FALL: i64 = 1_793_520_000;
    const HOUR: i64 = 3600;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    fn daily(minute: u16, portions: u8) -> Plan {
        Plan {
            minute,
            portions,
            days: Weekdays::DAILY,
            enabled: true,
            call: 0,
        }
    }

    fn table(plans: &[(usize, Plan)]) -> Plans {
        let mut table = [None; PLANS];
        for &(slot, plan) in plans {
            table[slot] = Some(plan);
        }
        Plans {
            generation: Generation(1),
            writable: true,
            plans: table,
        }
    }

    fn meal(plans: &Plans, now: i64) -> Option<(i64, u8)> {
        let denver = TimeZone::get("America/Denver").unwrap();
        next_meal(plans, at(now), &denver).map(|meal| (meal.at.as_second(), meal.slot))
    }

    /// The minute the spring change skips runs as far past the change; the
    /// one the fall change repeats runs on its first pass, so after it the
    /// next is the day after's. The feeder's own schedule tests pin the
    /// same instants.
    #[test]
    fn the_next_meal_follows_the_daylight_rules() {
        let skipped = table(&[(0, daily(2 * 60 + 30, 1))]);
        assert_eq!(meal(&skipped, SPRING - HOUR), Some((SPRING + 30 * 60, 0)));

        let repeated = table(&[(3, daily(90, 1))]);
        let first = FALL - 30 * 60;
        assert_eq!(meal(&repeated, first - 1), Some((first, 3)));
        assert_eq!(
            meal(&repeated, first),
            Some((FALL + 30 * 60 + 24 * HOUR, 3)),
            "01:30 MST on November 2"
        );
    }

    #[test]
    fn the_earliest_enabled_plan_is_next() {
        let disabled = Plan {
            enabled: false,
            ..daily(6 * 60, 4)
        };
        let plans = table(&[(0, daily(18 * 60, 2)), (1, disabled), (2, daily(7 * 60, 1))]);
        // Midnight MDT; 07:00 MDT is 13:00Z.
        let midnight = NOW + 6 * HOUR;
        let next = next_meal(
            &plans,
            at(midnight),
            &TimeZone::get("America/Denver").unwrap(),
        );
        assert_eq!(
            next,
            Some(NextMeal {
                at: at(midnight + 7 * HOUR),
                slot: 2,
                portions: 1,
            })
        );

        assert_eq!(meal(&table(&[(1, disabled)]), NOW), None);
        assert_eq!(meal(&table(&[]), NOW), None);
    }

    /// Plans due at the same instant run lowest slot first, so that one
    /// is named.
    #[test]
    fn a_tie_goes_to_the_lower_slot() {
        let plans = table(&[(5, daily(7 * 60, 3)), (2, daily(7 * 60, 1))]);
        let midnight = NOW + 6 * HOUR;
        assert_eq!(meal(&plans, midnight), Some((midnight + 7 * HOUR, 2)));
    }

    /// An enabled plan that runs once occurs at its next minute, as the
    /// feeder runs it; once it ran, the feeder disables it.
    #[test]
    fn a_plan_that_runs_once_is_next_at_its_minute() {
        let once = Plan {
            days: Weekdays::ONCE,
            ..daily(7 * 60, 1)
        };
        let midnight = NOW + 6 * HOUR;
        let plans = table(&[(0, once)]);
        assert_eq!(meal(&plans, midnight), Some((midnight + 7 * HOUR, 0)));
        assert_eq!(
            meal(&plans, midnight + 8 * HOUR),
            Some((midnight + 31 * HOUR, 0)),
            "past its minute today, tomorrow's"
        );
        let ran = Plan {
            enabled: false,
            ..once
        };
        assert_eq!(meal(&table(&[(0, ran)]), midnight), None);
    }

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
