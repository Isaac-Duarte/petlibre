//! Serialized extension of a free-running `u32` hardware counter.
//!
//! Gives the core wrapping millisecond time from raw ticks, carrying the
//! sub-millisecond remainder. It must be read at least every half counter
//! range; a backward or ambiguous step latches it invalid.

use crate::HALF_RANGE;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickClock {
    pub ticks_per_second: u32,
    pub raw_at: u32,
    pub millis: u32,
    pub fraction: u32,
    pub valid: bool,
}

impl TickClock {
    /// Frequency is a caller-established hardware property, not a device default.
    pub fn new(ticks_per_second: u32, raw: u32, initial_ms: u32) -> Option<Self> {
        (ticks_per_second != 0).then_some(Self {
            ticks_per_second,
            raw_at: raw,
            millis: initial_ms,
            fraction: 0,
            valid: true,
        })
    }

    /// Read more often than half the RAW counter range AND half the
    /// millisecond range. Sub-millisecond fractions are preserved; raw and
    /// millisecond wraps work. Backward/ambiguous movement latches invalid until
    /// an explicit stopped reinit. Failures keep the last good coordinates.
    pub fn read(&mut self, raw: u32) -> Option<u32> {
        if !self.valid {
            return None;
        }
        let delta = raw.wrapping_sub(self.raw_at);
        if self.ticks_per_second == 0
            || self.fraction >= self.ticks_per_second
            || delta >= HALF_RANGE
        {
            self.valid = false;
            return None;
        }

        let scaled = u64::from(delta) * 1000 + u64::from(self.fraction);
        let elapsed = scaled / u64::from(self.ticks_per_second);
        if elapsed >= u64::from(HALF_RANGE) {
            self.valid = false;
            return None;
        }

        self.raw_at = raw;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a remainder of a u32 divisor"
        )]
        let fraction = (scaled % u64::from(self.ticks_per_second)) as u32;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "checked below HALF_RANGE above"
        )]
        let elapsed = elapsed as u32;
        self.fraction = fraction;
        self.millis = self.millis.wrapping_add(elapsed);
        Some(self.millis)
    }

    /// Mark the clock invalid, as a failed read with no output would.
    pub fn invalidate(&mut self) {
        self.valid = false;
    }
}
