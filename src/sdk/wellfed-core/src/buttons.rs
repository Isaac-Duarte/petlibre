//! The panel buttons as gestures. A press, a hold past each of the
//! firmware's thresholds and the release, with buttons pressed together read
//! as one chord.
//!
//! The engine takes the pressed set each poll and keeps no clock of its own.
//! Each line is debounced on its own: it changes only after reading its new
//! level for `debounce_ms` without a break. On the research unit no line
//! bounced at a 2 ms poll (physical), so the debounce only guards against
//! a noisy contact.
//!
//! A gesture starts when the first button settles pressed and ends when every
//! button has settled released. Its chord is every button that was pressed
//! during it. A hold counts from the moment the chord last grew, and only
//! while the whole chord stays pressed. Letting go of one button of a chord
//! ends its holds but not the gesture, so a two-button hold never turns into
//! a one-button hold. What a gesture means (feed, lock, reset) is the
//! firmware's, and stock's hold durations are the to record.

use crate::deadline_passed;

/// The most hold thresholds a firmware can set.
pub const MAX_HOLDS: usize = 3;

crate::named_enum! {
    /// The panel buttons, in pin order PB4..PB7 (physical).
    pub enum Button {
        Reset = "reset",
        Mic = "mic",
        Dispense = "dispense",
        Lock = "lock",
    }
}

impl Button {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of buttons: bit `n` is the button on PB(4 + n).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons(u8);

impl Buttons {
    pub const NONE: Self = Self(0);
    const ALL_BITS: u8 = 0x0f;

    pub const fn of(button: Button) -> Self {
        Self(button.bit())
    }

    pub const fn with(self, button: Button) -> Self {
        Self(self.0 | button.bit())
    }

    /// The pressed buttons in a port B input word. The buttons are active LOW
    /// on PB4..PB7, with board pull-ups (physical).
    pub const fn from_port_b(levels: u32) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "masked to the four button bits"
        )]
        let bits = ((!levels >> 4) & Self::ALL_BITS as u32) as u8;
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL_BITS)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, button: Button) -> bool {
        self.0 & button.bit() != 0
    }

    const fn covers(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::fmt::Display for Buttons {
    /// `dispense+lock`, or `none`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut first = true;
        for button in Button::ALL {
            if self.contains(*button) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(button.as_str())?;
                first = false;
            }
        }
        Ok(())
    }
}

/// How the firmware reads its buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub debounce_ms: u32,
    /// Hold thresholds in rising order; only the first `hold_count` are used.
    pub holds_ms: [u32; MAX_HOLDS],
    pub hold_count: usize,
}

impl Config {
    /// Bench values for the checks, not stock's: the panel card records those.
    pub const BENCH: Self = Self {
        debounce_ms: 30,
        holds_ms: [2_000, 5_000, 0],
        hold_count: 2,
    };

    /// Rising thresholds, each longer than the debounce.
    pub fn valid(&self) -> bool {
        let Some(holds) = self.holds_ms.get(..self.hold_count) else {
            return false;
        };
        self.debounce_ms > 0
            && holds.first().is_none_or(|first| *first > self.debounce_ms)
            && holds.windows(2).all(|pair| pair[0] < pair[1])
    }

    fn holds(&self) -> &[u32] {
        self.holds_ms.get(..self.hold_count).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// A button joined the gesture; `chord` is everything pressed so far.
    Down { chord: Buttons },
    /// The whole chord has been held past threshold `level` (1-based).
    Hold { chord: Buttons, level: u8 },
    /// Every button is released. `level` is the highest hold reached, 0 for
    /// a short press; `held_ms` runs from the first press.
    Up {
        chord: Buttons,
        level: u8,
        held_ms: u32,
    },
}

impl core::fmt::Display for Gesture {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Down { chord } => write!(f, "down chord={chord}"),
            Self::Hold { chord, level } => write!(f, "hold chord={chord} level={level}"),
            Self::Up {
                chord,
                level,
                held_ms,
            } => write!(f, "up chord={chord} level={level} held_ms={held_ms}"),
        }
    }
}

/// Bits 30..31 the kind (never zero), 26..29 the chord, 24..25 the level,
/// 0..23 the held time, capped at about 4.6 hours.
impl Gesture {
    const HELD_MAX: u32 = (1 << 24) - 1;

    /// One word, so a gesture crosses tasks through an atomic.
    pub fn pack(self) -> u32 {
        let (kind, chord, level, held_ms) = match self {
            Self::Down { chord } => (1, chord, 0, 0),
            Self::Hold { chord, level } => (2, chord, level, 0),
            Self::Up {
                chord,
                level,
                held_ms,
            } => (3, chord, level, held_ms),
        };
        kind << 30
            | u32::from(chord.bits()) << 26
            | u32::from(level.min(3)) << 24
            | held_ms.min(Self::HELD_MAX)
    }

    pub fn unpack(word: u32) -> Option<Self> {
        let chord = Buttons::from_bits(u8::try_from((word >> 26) & 0x0f).ok()?);
        let level = u8::try_from((word >> 24) & 0x03).ok()?;
        match word >> 30 {
            1 => Some(Self::Down { chord }),
            2 => Some(Self::Hold { chord, level }),
            3 => Some(Self::Up {
                chord,
                level,
                held_ms: word & Self::HELD_MAX,
            }),
            _ => None,
        }
    }
}

/// A gesture in progress.
#[derive(Clone, Copy, Debug)]
struct Active {
    chord: Buttons,
    started_ms: u32,
    /// When the chord last grew; holds count from here.
    grown_ms: u32,
    /// Holds already reported.
    level: u8,
    /// A button of the chord was let go, so no further hold counts.
    broken: bool,
}

/// The debounced lines and the gesture in progress.
#[derive(Clone, Copy, Debug)]
pub struct Engine {
    config: Config,
    settled: Buttons,
    /// Per line: since when it has read unlike `settled`.
    differing_since: [Option<u32>; 4],
    active: Option<Active>,
}

impl Engine {
    /// Lines start released, so a button already held settles as a press.
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            settled: Buttons::NONE,
            differing_since: [None; 4],
            active: None,
        }
    }

    /// The debounced pressed set.
    pub const fn settled(&self) -> Buttons {
        self.settled
    }

    /// One poll: `pressed` is the raw set read at `now_ms`. At most one
    /// gesture comes out of a poll, as the rules above never produce two.
    pub fn update(&mut self, now_ms: u32, pressed: Buttons) -> Option<Gesture> {
        self.debounce(now_ms, pressed);
        let after = self.settled;

        let Some(mut active) = self.active else {
            if after.is_empty() {
                return None;
            }
            self.active = Some(Active {
                chord: after,
                started_ms: now_ms,
                grown_ms: now_ms,
                level: 0,
                broken: false,
            });
            return Some(Gesture::Down { chord: after });
        };

        let gesture = if after.is_empty() {
            self.active = None;
            return Some(Gesture::Up {
                chord: active.chord,
                level: active.level,
                held_ms: now_ms.wrapping_sub(active.started_ms),
            });
        } else if !active.chord.covers(after) {
            active.chord = Buttons(active.chord.0 | after.0);
            active.grown_ms = now_ms;
            active.level = 0;
            active.broken = !after.covers(active.chord);
            Some(Gesture::Down {
                chord: active.chord,
            })
        } else {
            active.broken |= !after.covers(active.chord);
            self.hold(&mut active, now_ms)
        };

        self.active = Some(active);
        gesture
    }

    fn debounce(&mut self, now_ms: u32, pressed: Buttons) {
        for (index, button) in Button::ALL.iter().enumerate() {
            let raw = pressed.contains(*button);
            let since = &mut self.differing_since[index];
            if raw == self.settled.contains(*button) {
                *since = None;
                continue;
            }

            let first = *since.get_or_insert(now_ms);
            if deadline_passed(now_ms, first.wrapping_add(self.config.debounce_ms)) {
                self.settled = Buttons(self.settled.0 ^ button.bit());
                *since = None;
            }
        }
    }

    /// The highest threshold newly passed, if any. A poll that comes late
    /// reports only the highest, so a stalled task never floods holds.
    fn hold(&self, active: &mut Active, now_ms: u32) -> Option<Gesture> {
        if active.broken {
            return None;
        }

        let held = now_ms.wrapping_sub(active.grown_ms);
        let reached = self.config.holds().iter().filter(|ms| held >= **ms).count();
        let reached = u8::try_from(reached).unwrap_or(u8::MAX);
        if reached <= active.level {
            return None;
        }
        active.level = reached;
        Some(Gesture::Hold {
            chord: active.chord,
            level: reached,
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::ToString;
    use std::vec::Vec;

    use super::*;

    const POLL: u32 = 5;

    fn dispense() -> Buttons {
        Buttons::of(Button::Dispense)
    }

    /// Polls `pressed` for `ms`, collecting gestures.
    fn run(engine: &mut Engine, now: &mut u32, pressed: Buttons, ms: u32, out: &mut Vec<Gesture>) {
        for _ in 0..ms / POLL {
            *now = now.wrapping_add(POLL);
            out.extend(engine.update(*now, pressed));
        }
    }

    fn engine() -> (Engine, u32, Vec<Gesture>) {
        (Engine::new(Config::BENCH), 1_000, Vec::new())
    }

    #[test]
    fn port_levels_are_active_low_on_pb4_to_pb7() {
        assert_eq!(Buttons::from_port_b(0xf0), Buttons::NONE);
        assert_eq!(Buttons::from_port_b(0xe0), Buttons::of(Button::Reset));
        assert_eq!(Buttons::from_port_b(0x70), Buttons::of(Button::Lock));
        assert_eq!(
            Buttons::from_port_b(0x30).to_string(),
            "dispense+lock",
            "two lines low read as both"
        );
    }

    #[test]
    fn a_short_press_is_down_then_up() {
        let (mut engine, mut now, mut out) = engine();

        run(&mut engine, &mut now, dispense(), 300, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);

        assert_eq!(
            out,
            [
                Gesture::Down { chord: dispense() },
                Gesture::Up {
                    chord: dispense(),
                    level: 0,
                    held_ms: 300
                },
            ]
        );
    }

    #[test]
    fn bounce_and_glitches_shorter_than_the_debounce_are_ignored() {
        let (mut engine, mut now, mut out) = engine();

        // A 20 ms glitch, then a press that chatters for 15 ms before holding.
        run(&mut engine, &mut now, dispense(), 20, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);
        assert!(out.is_empty());

        for pressed in [dispense(), Buttons::NONE, dispense()] {
            run(&mut engine, &mut now, pressed, 5, &mut out);
        }
        run(&mut engine, &mut now, dispense(), 200, &mut out);
        assert_eq!(out, [Gesture::Down { chord: dispense() }]);
        assert_eq!(engine.settled(), dispense());
    }

    #[test]
    fn holds_fire_once_per_threshold_and_the_release_carries_the_level() {
        let (mut engine, mut now, mut out) = engine();

        run(&mut engine, &mut now, dispense(), 6_000, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);

        assert_eq!(
            out,
            [
                Gesture::Down { chord: dispense() },
                Gesture::Hold {
                    chord: dispense(),
                    level: 1
                },
                Gesture::Hold {
                    chord: dispense(),
                    level: 2
                },
                Gesture::Up {
                    chord: dispense(),
                    level: 2,
                    held_ms: 6_000
                },
            ]
        );
    }

    #[test]
    fn a_second_button_grows_the_chord_and_restarts_the_hold() {
        let (mut engine, mut now, mut out) = engine();
        let both = dispense().with(Button::Lock);

        run(&mut engine, &mut now, dispense(), 1_500, &mut out);
        run(&mut engine, &mut now, both, 2_500, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);

        assert_eq!(
            out,
            [
                Gesture::Down { chord: dispense() },
                Gesture::Down { chord: both },
                Gesture::Hold {
                    chord: both,
                    level: 1
                },
                Gesture::Up {
                    chord: both,
                    level: 1,
                    held_ms: 4_000
                },
            ]
        );
    }

    #[test]
    fn letting_go_of_part_of_a_chord_ends_its_holds_but_not_the_gesture() {
        let (mut engine, mut now, mut out) = engine();
        let both = dispense().with(Button::Lock);

        run(&mut engine, &mut now, both, 500, &mut out);
        run(&mut engine, &mut now, dispense(), 4_000, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);

        assert_eq!(
            out,
            [
                Gesture::Down { chord: both },
                Gesture::Up {
                    chord: both,
                    level: 0,
                    held_ms: 4_500
                },
            ]
        );
    }

    #[test]
    fn a_late_poll_reports_only_the_highest_hold() {
        let (mut engine, mut now, mut out) = engine();

        run(&mut engine, &mut now, dispense(), 100, &mut out);
        now += 10_000;
        out.extend(engine.update(now, dispense()));

        assert_eq!(
            out.last(),
            Some(&Gesture::Hold {
                chord: dispense(),
                level: 2
            })
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_clock_may_wrap_during_a_gesture() {
        let mut engine = Engine::new(Config::BENCH);
        let mut now = u32::MAX - 1_000;
        let mut out = Vec::new();

        run(&mut engine, &mut now, dispense(), 2_500, &mut out);
        run(&mut engine, &mut now, Buttons::NONE, 100, &mut out);

        assert!(matches!(out[1], Gesture::Hold { level: 1, .. }));
        assert!(matches!(out[2], Gesture::Up { held_ms: 2_500, .. }));
    }

    #[test]
    fn thresholds_must_rise_past_the_debounce() {
        assert!(Config::BENCH.valid());
        let bad = [
            Config {
                holds_ms: [20, 5_000, 0],
                ..Config::BENCH
            },
            Config {
                holds_ms: [5_000, 2_000, 0],
                ..Config::BENCH
            },
            Config {
                hold_count: 4,
                ..Config::BENCH
            },
            Config {
                debounce_ms: 0,
                ..Config::BENCH
            },
        ];
        for config in bad {
            assert!(!config.valid(), "{config:?}");
        }
    }

    #[test]
    fn gestures_survive_packing() {
        let both = dispense().with(Button::Lock);
        for gesture in [
            Gesture::Down { chord: both },
            Gesture::Hold {
                chord: dispense(),
                level: 2,
            },
            Gesture::Up {
                chord: Buttons::of(Button::Reset),
                level: 1,
                held_ms: 123_456,
            },
        ] {
            assert_eq!(Gesture::unpack(gesture.pack()), Some(gesture));
        }
        assert_eq!(Gesture::unpack(0), None);
    }

    #[test]
    fn gestures_print_as_records() {
        let both = dispense().with(Button::Lock);
        assert_eq!(
            Gesture::Down { chord: both }.to_string(),
            "down chord=dispense+lock"
        );
        assert_eq!(
            Gesture::Up {
                chord: Buttons::of(Button::Reset),
                level: 1,
                held_ms: 2_400
            }
            .to_string(),
            "up chord=reset level=1 held_ms=2400"
        );
    }
}
