//! The virtual panel: button presses typed on the log UART, for a bench
//! with nobody at it. A line `press <button> [hold <ms>]` holds that button
//! down for the hold (150 ms by default), and the button service ORs the
//! held set into the levels it reads from the pins, so the press goes
//! through the same debounce and gesture engine as a finger. Setup codes,
//! recovery windows and update-key enrollment see nothing different.
//!
//! Only the feeder's `virtual-panel` variant reads the UART; no other image
//! links a reader, so typing on a production feeder's log UART does
//! nothing. The bench, not the client under test, types the line: a test
//! that pairs with a press here proves what pairing with a person there
//! would.

use crate::buttons::{Button, Buttons};
use crate::deadline_passed;

/// The hold when a line names none: well past the engine's debounce and
/// short of every hold threshold, so it reads as a press.
pub const DEFAULT_HOLD_MS: u32 = 150;
/// The longest hold a line may ask for: past every hold threshold a
/// firmware sets, short of a stuck button.
pub const MAX_HOLD_MS: u32 = 15_000;
/// The longest line kept; a longer one is dropped whole at its end.
const LINE: usize = 40;

/// A press a line asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub button: Button,
    pub hold_ms: u32,
}

/// What a finished line was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Press(Press),
    /// Not a press line: an unknown word, a button that isn't one, a hold
    /// out of range, or too long. Nothing is pressed.
    Refused,
}

impl Line {
    /// The line `text` (without its end).
    pub fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_ascii_whitespace();
        let first = words.next()?;
        if first != "press" {
            return Some(Self::Refused);
        }

        let refused = Some(Self::Refused);
        let Some(button) = words.next().and_then(Button::parse) else {
            return refused;
        };
        let hold_ms = match (words.next(), words.next()) {
            (None, _) => DEFAULT_HOLD_MS,
            (Some("hold"), Some(ms)) => match ms.parse::<u32>() {
                Ok(ms) if (1..=MAX_HOLD_MS).contains(&ms) => ms,
                _ => return refused,
            },
            _ => return refused,
        };
        if words.next().is_some() {
            return refused;
        }
        Some(Self::Press(Press { button, hold_ms }))
    }
}

/// Bytes from the UART into lines. Ends at `\r` or `\n`; empty lines are
/// skipped.
#[derive(Debug)]
pub struct Reader {
    buf: [u8; LINE],
    len: usize,
    overflow: bool,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE],
            len: 0,
            overflow: false,
        }
    }

    /// Takes one byte; a finished line comes back.
    pub fn byte(&mut self, byte: u8) -> Option<Line> {
        if byte != b'\r' && byte != b'\n' {
            match self.buf.get_mut(self.len) {
                Some(slot) => {
                    *slot = byte;
                    self.len += 1;
                }
                None => self.overflow = true,
            }
            return None;
        }

        let text = self.buf.get(..self.len).unwrap_or_default();
        let line = if self.overflow {
            Some(Line::Refused)
        } else {
            core::str::from_utf8(text).map_or(Some(Line::Refused), Line::parse)
        };
        self.len = 0;
        self.overflow = false;
        line
    }
}

/// The buttons held down by presses, each until its deadline.
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    /// Per button, in `Button::ALL` order: when it lets go.
    until: [Option<u32>; 4],
}

impl Held {
    pub const fn new() -> Self {
        Self { until: [None; 4] }
    }

    /// Holds `press.button` from `now_ms` for its hold. A press of a
    /// button already held restarts its hold.
    pub fn press(&mut self, now_ms: u32, press: Press) {
        if let Some(slot) = self.slot(press.button) {
            *slot = Some(now_ms.wrapping_add(press.hold_ms));
        }
    }

    /// The buttons held at `now_ms`; a hold past its deadline lets go.
    pub fn levels(&mut self, now_ms: u32) -> Buttons {
        let mut held = Buttons::NONE;
        for (button, until) in Button::ALL.iter().zip(self.until.iter_mut()) {
            match *until {
                Some(deadline) if deadline_passed(now_ms, deadline) => *until = None,
                Some(_) => held = held.with(*button),
                None => {}
            }
        }
        held
    }

    fn slot(&mut self, button: Button) -> Option<&mut Option<u32>> {
        let index = Button::ALL.iter().position(|b| *b == button)?;
        self.until.get_mut(index)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::format;
    use std::vec::Vec;

    use super::*;

    fn lines(text: &str) -> Vec<Line> {
        let mut reader = Reader::new();
        text.bytes().filter_map(|b| reader.byte(b)).collect()
    }

    #[test]
    fn a_press_line_names_its_button_and_hold() {
        assert_eq!(
            lines("press dispense\n"),
            [Line::Press(Press {
                button: Button::Dispense,
                hold_ms: DEFAULT_HOLD_MS
            })]
        );
        assert_eq!(
            lines("press lock hold 5200\r\n"),
            [Line::Press(Press {
                button: Button::Lock,
                hold_ms: 5200
            })]
        );
    }

    #[test]
    fn anything_else_presses_nothing() {
        for text in [
            "pres lock\n",
            "press\n",
            "press bowl\n",
            "press lock hold\n",
            "press lock hold 0\n",
            "press lock hold 15001\n",
            "press lock hold x\n",
            "press lock for 100\n",
            "press lock hold 100 more\n",
        ] {
            assert_eq!(lines(text), [Line::Refused], "{text:?}");
        }
        // Empty lines, and a CRLF's second end, are no lines at all.
        assert!(lines("\r\n\n").is_empty());
    }

    #[test]
    fn a_line_too_long_is_refused_whole_and_the_next_one_reads() {
        let long = format!("press {}\npress mic\n", "x".repeat(LINE));
        assert_eq!(
            lines(&long),
            [
                Line::Refused,
                Line::Press(Press {
                    button: Button::Mic,
                    hold_ms: DEFAULT_HOLD_MS
                })
            ]
        );
    }

    #[test]
    fn a_press_holds_until_its_deadline_across_the_clock_wrap() {
        let mut held = Held::new();
        let start = u32::MAX - 50;
        held.press(
            start,
            Press {
                button: Button::Reset,
                hold_ms: 100,
            },
        );
        held.press(
            start,
            Press {
                button: Button::Mic,
                hold_ms: 300,
            },
        );

        let both = Buttons::of(Button::Reset).with(Button::Mic);
        assert_eq!(held.levels(start), both);
        assert_eq!(held.levels(start.wrapping_add(99)), both);
        assert_eq!(
            held.levels(start.wrapping_add(100)),
            Buttons::of(Button::Mic)
        );
        assert_eq!(held.levels(start.wrapping_add(300)), Buttons::NONE);
    }
}
