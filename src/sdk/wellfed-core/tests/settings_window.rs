//! The lights and sound windows: stock's inclusive bounds, a wrap past
//! midnight, and an invalid clock leaving every window open.

use wellfed_core::settings::{SettingsFields, SettingsView, Window};

const MINUTE_MS: i64 = 60_000;

/// Local wall time at `minute` of some day, plus `extra_ms`.
fn at(minute: i64, extra_ms: i64) -> Option<i64> {
    Some(1_790_000_000_000 / (1440 * MINUTE_MS) * 1440 * MINUTE_MS + minute * MINUTE_MS + extra_ms)
}

#[test]
fn a_window_includes_both_bounds() {
    let day = Window::Between {
        start: 7 * 60,
        end: 22 * 60,
    };

    assert!(!day.allows(at(7 * 60 - 1, 59_999)));
    assert!(day.allows(at(7 * 60, 0)));
    assert!(day.allows(at(22 * 60, 59_999)));
    assert!(!day.allows(at(22 * 60 + 1, 0)));
}

#[test]
fn a_window_whose_end_is_before_its_start_wraps_midnight() {
    let night = Window::Between {
        start: 22 * 60,
        end: 6 * 60,
    };

    assert!(night.allows(at(23 * 60, 0)));
    assert!(night.allows(at(0, 0)));
    assert!(night.allows(at(6 * 60, 0)));
    assert!(!night.allows(at(12 * 60, 0)));
}

#[test]
fn equal_bounds_are_one_minute() {
    let one = Window::Between { start: 0, end: 0 };

    assert!(one.allows(at(0, 30_000)));
    assert!(!one.allows(at(1, 0)));
    assert!(!one.allows(at(1439, 0)));
}

#[test]
fn every_window_is_open_without_a_local_time() {
    let closed_now = Window::Between { start: 0, end: 0 };

    assert!(closed_now.allows(None));
    assert!(Window::Always.allows(None));
}

#[test]
fn a_bound_must_be_a_minute_of_the_day() {
    let past = |start, end| SettingsFields {
        lights_window: Some(Window::Between { start, end }),
        ..SettingsFields::default()
    };

    assert!(past(0, 1439).valid());
    assert!(!past(1440, 0).valid());
    assert!(!past(0, 1440).valid());
    assert!(
        !SettingsFields {
            volume: Some(101),
            ..SettingsFields::default()
        }
        .valid()
    );
}

#[test]
fn sound_needs_the_switch_a_volume_and_the_window() {
    let on = SettingsView {
        sound: true,
        volume: 50,
        sound_window: Window::Between {
            start: 8 * 60,
            end: 20 * 60,
        },
        ..SettingsView::default()
    };

    assert!(on.sound_at(at(12 * 60, 0)));
    assert!(!on.sound_at(at(21 * 60, 0)));
    assert!(!SettingsView { volume: 0, ..on }.sound_at(at(12 * 60, 0)));
    assert!(!SettingsView { sound: false, ..on }.sound_at(at(12 * 60, 0)));
}

#[test]
fn a_window_reads_as_always_or_its_bounds() {
    assert_eq!(Window::Always.to_string(), "always");
    assert_eq!(
        Window::Between {
            start: 7 * 60 + 5,
            end: 22 * 60
        }
        .to_string(),
        "07:05-22:00"
    );
}
