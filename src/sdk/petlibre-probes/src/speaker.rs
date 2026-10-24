//! Speaker records as line bodies: the startup chime and tunes
//! (`PLAUD1`, `petlibre_hal::audio`) and the signed network stream
//! (`PLSPK1`, `petlibre_hal::stream`). The players themselves are the HAL's;
//! this only lets a firmware log their records through [`crate::log`].

use core::fmt;

use petlibre_hal::{audio, stream};

/// A `PLAUD1 …` body.
#[derive(Clone, Copy, Debug)]
pub struct Audio(pub audio::Record);

impl fmt::Display for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        audio::describe(self.0, f)
    }
}

/// A `PLSPK1 …` body.
#[derive(Clone, Copy, Debug)]
pub struct Stream(pub stream::Record);

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        stream::describe(self.0, f)
    }
}
