//! Speaker records as line bodies: the startup chime and tunes (`PLAUD1`,
//! `wellfed_hal::audio`). The player itself is the HAL's; this only lets
//! a firmware log its records through `wellfed_sdk::log`.

use core::fmt;

use wellfed_hal::audio;

/// A `PLAUD1 …` body.
#[derive(Clone, Copy, Debug)]
pub struct Audio(pub audio::Record);

impl fmt::Display for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        audio::describe(self.0, f)
    }
}
