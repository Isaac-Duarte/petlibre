use proptest::prelude::*;
use wellfed_core::clock::zone::{Rule, TEXT_MAX, Zone};

const H: i32 = 3600;

fn rule(text: &str) -> Rule {
    Zone::parse(text.as_bytes()).unwrap().rule()
}

#[test]
fn fixed_zones_have_one_offset() {
    assert_eq!(rule("UTC0").offset_at(0), 0);
    assert_eq!(rule("<+0530>-5:30").offset_at(1_800_000_000), 5 * H + 1800);
    assert_eq!(rule("<-03>3").offset_at(1_800_000_000), -3 * H);
    assert_eq!(rule("MST7").next_change(1_800_000_000), None);
}

#[test]
fn denver_changes_on_the_second_sunday_of_march_and_first_of_november() {
    let denver = rule("MST7MDT,M3.2.0,M11.1.0");
    // 2026-03-08T09:00:00Z is 02:00 MST: daylight time from then.
    assert_eq!(denver.offset_at(1_772_960_399), -7 * H);
    assert_eq!(denver.offset_at(1_772_960_400), -6 * H);
    // 2026-11-01T08:00:00Z is 02:00 MDT: standard time from then.
    assert_eq!(denver.offset_at(1_793_519_999), -6 * H);
    assert_eq!(denver.offset_at(1_793_520_000), -7 * H);
    assert_eq!(
        denver.next_change(1_792_674_000),
        Some((1_793_520_000, -7 * H))
    );
    assert_eq!(
        denver.next_change(1_793_520_000),
        Some((1_805_014_800, -6 * H))
    );
}

#[test]
fn the_southern_hemisphere_spans_new_year() {
    // Sydney: daylight from the first Sunday of October to the first of April.
    let sydney = rule("AEST-10AEDT,M10.1.0,M4.1.0/3");
    // 2026-01-15: summer.
    assert_eq!(sydney.offset_at(1_768_435_200), 11 * H);
    // 2026-07-01: winter.
    assert_eq!(sydney.offset_at(1_782_864_000), 10 * H);
}

#[test]
fn last_weekday_and_julian_dates() {
    // Europe/Berlin: last Sunday of March 01:00 UTC.
    let berlin = rule("CET-1CEST,M3.5.0,M10.5.0/3");
    // 2026-03-29T01:00:00Z.
    assert_eq!(berlin.offset_at(1_774_745_999), H);
    assert_eq!(berlin.offset_at(1_774_746_000), 2 * H);

    // Permanent daylight time written as all-year rules.
    let permanent = rule("EST5EDT,0/0,J365/25");
    assert_eq!(permanent.offset_at(1_782_864_000), -4 * H);
}

#[test]
fn bad_rules_are_refused() {
    for text in [
        "",
        "U0",
        "UTC",
        "UTC25",
        "<-13>13",
        "<+15>-15",
        "<+14>-14<+15>,M3.2.0,M11.1.0",
        "MST7MDT",
        "MST7MDT,M3.2.0",
        "MST7MDT,M13.2.0,M11.1.0",
        "MST7MDT,M3.6.0,M11.1.0",
        "MST7MDT,M3.2.7,M11.1.0",
        "MST7MDT,J0,J365",
        "MST7MDT,M3.2.0,M11.1.0/168",
        "<+05>-5x",
        "<+5>-5",
    ] {
        assert!(Zone::parse(text.as_bytes()).is_err(), "{text:?}");
    }
    assert!(Zone::parse(&[b'A'; TEXT_MAX + 1]).is_err());
}

#[test]
fn the_text_is_kept_as_sent() {
    let zone = Zone::parse(b"MST7MDT,M3.2.0,M11.1.0").unwrap();
    assert_eq!(zone.text(), b"MST7MDT,M3.2.0,M11.1.0");
}

proptest! {
    #[test]
    fn no_text_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64), at in any::<i64>()) {
        if let Ok(zone) = Zone::parse(&bytes) {
            let at = at.clamp(-10_000_000_000, 10_000_000_000);
            let _ = zone.rule().offset_at(at);
            let _ = zone.rule().next_change(at);
        }
    }
}
