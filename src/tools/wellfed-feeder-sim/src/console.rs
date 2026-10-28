//! The operator's console: one line of text becomes what the hands on the
//! feeder would do.
//!
//! - `press <button>…` presses each button once, in turn: how a setup or
//!   recovery code is keyed in. A button is its name (`lock`, `dispense`,
//!   `mic`, `reset`) or its position from the left (1-4), as `wellfed`
//!   prints a code.
//! - `hold <button>[+<button>…]` holds the chord past the panel's hold
//!   (5 s): `hold lock` toggles the lock, `hold lock+dispense` opens the
//!   recovery window on an unlocked panel, `hold reset` resets the keys.
//! - `adapter in|out` fits or pulls the adapter; `pack <mV>|out` fits a
//!   pack reading that many millivolts (three fresh D cells read about
//!   4757) or takes the cells out. Both last across restarts.
//! - `empty` empties the hopper and `fill` fills it, for the next feed's
//!   food beam.
//! - `block` blocks the outlet and `jam` jams the rotor once the next
//!   feed has driven a portion: that feed ends there, one portion in
//!   (none for a feed of one).
//! - `reboot` restarts the feeder; `quit` stops the simulator.

use wellfed_client::setup::panel_position;
use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_hal::power::DIVIDER_PER_MILLE;

use crate::serve::{Control, FeedFault};

/// A short press's length, as the button service would report it.
const PRESS_MS: u32 = 120;
/// Past the panel's 5 s hold.
const HOLD_MS: u32 = 5_200;

pub const HELP: &str = "commands: press <button>... | hold <button>[+<button>...] | adapter in|out | pack <mV>|out | empty | fill | block | jam | reboot | quit\n\
     buttons: lock dispense mic reset, or their positions 1-4";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
    #[error("{0}: not a button (lock, dispense, mic, reset or 1-4)")]
    Button(String),
    #[error("name at least one button")]
    NoButtons,
    #[error("{0}: not a command")]
    Command(String),
    #[error("adapter takes `in` or `out`")]
    Adapter,
    #[error("pack takes the pack's millivolts or `out`")]
    Pack,
}

/// What one line asks for, in order.
pub fn parse(line: &str) -> Result<Vec<Control>, ConsoleError> {
    let mut words = line.split_whitespace();
    let Some(command) = words.next() else {
        return Ok(Vec::new());
    };
    match command {
        "press" => {
            let mut controls = Vec::new();
            for word in words {
                let chord = Buttons::of(button(word)?);
                controls.push(Control::Gesture(Gesture::Down { chord }));
                controls.push(Control::Gesture(Gesture::Up {
                    chord,
                    level: 0,
                    held_ms: PRESS_MS,
                }));
            }
            if controls.is_empty() {
                return Err(ConsoleError::NoButtons);
            }
            Ok(controls)
        }
        "hold" => {
            let mut controls = Vec::new();
            let mut chord: Option<Buttons> = None;
            for word in words.flat_map(|w| w.split('+')).filter(|w| !w.is_empty()) {
                let button = button(word)?;
                let next = chord.map_or(Buttons::of(button), |c| c.with(button));
                chord = Some(next);
                controls.push(Control::Gesture(Gesture::Down { chord: next }));
            }
            let chord = chord.ok_or(ConsoleError::NoButtons)?;
            controls.push(Control::Gesture(Gesture::Hold { chord, level: 1 }));
            controls.push(Control::Gesture(Gesture::Up {
                chord,
                level: 1,
                held_ms: HOLD_MS,
            }));
            Ok(controls)
        }
        "adapter" => match words.next() {
            Some("in") => Ok(vec![Control::Adapter(true)]),
            Some("out") => Ok(vec![Control::Adapter(false)]),
            _ => Err(ConsoleError::Adapter),
        },
        "pack" => match words.next() {
            Some("out") => Ok(vec![Control::Pack(None)]),
            Some(mv) => {
                let mv: u32 = mv.parse().map_err(|_| ConsoleError::Pack)?;
                // The battery channel reads the pack through the board's
                // divider.
                let pin_mv = mv.saturating_mul(1000) / DIVIDER_PER_MILLE;
                Ok(vec![Control::Pack(Some(pin_mv))])
            }
            None => Err(ConsoleError::Pack),
        },
        "empty" => Ok(vec![Control::Hopper(false)]),
        "fill" => Ok(vec![Control::Hopper(true)]),
        "block" => Ok(vec![Control::FaultFeed(FeedFault::OutletBlocked)]),
        "jam" => Ok(vec![Control::FaultFeed(FeedFault::Jam)]),
        "reboot" => Ok(vec![Control::Reboot]),
        "quit" | "exit" => Ok(vec![Control::Quit]),
        other => Err(ConsoleError::Command(other.to_owned())),
    }
}

fn button(word: &str) -> Result<Button, ConsoleError> {
    Button::ALL
        .iter()
        .copied()
        .find(|&b| word == b.as_str() || word == panel_position(b).to_string())
        .ok_or_else(|| ConsoleError::Button(word.to_owned()))
}
