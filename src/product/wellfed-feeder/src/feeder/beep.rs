//! The panel's beep: stock pulses its buzzer (PA25, an active buzzer) HIGH
//! for 200 ms, and every button press beeps (owner, October 28). A press
//! beeps only when sound may play: the `sound` switch on, a volume above 0
//! and inside the sound window, as the speaker's tunes
//! ([`wellfed_core::settings::SettingsView::sound_at`]). The buzzer has
//! one pitch and one loudness, so the volume only mutes it at 0.
//!
//! A step is at most one masked port write, made whatever the motor does
//! (the buzzer line shares no pin with the motor, emitters or lights), so a
//! beep never waits on a feed or delays one.

use wellfed_core::deadline_passed;

use super::platform::Platform;

/// How long one beep lasts, as stock's one-shot timer.
pub const BEEP_MS: u32 = 200;

#[derive(Clone, Copy, Debug, Default)]
pub struct Beeper {
    /// The line is an output: enabled on the first step at rest.
    enabled: bool,
    /// When the beep that sounds now ends.
    until: Option<u32>,
}

impl Beeper {
    pub const fn new() -> Self {
        Self {
            enabled: false,
            until: None,
        }
    }

    /// One step at `now_ms`: `pressed` when a button went down since the
    /// last, `allowed` when sound may play now, `at_rest` when no feed is
    /// queued or runs. The delay until the beep ends, or `u32::MAX` while
    /// silent. A press during a beep restarts it rather than stacking
    /// another.
    ///
    /// The line is made an output on the first step at rest, as the
    /// bring-up probe did, so a feed resumed at boot never sees port A's
    /// direction register rewritten under it; a press before then is
    /// silent.
    pub fn step<P: Platform>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        pressed: bool,
        allowed: bool,
        at_rest: bool,
    ) -> u32 {
        if !self.enabled && at_rest {
            platform.buzzer_enable();
            self.enabled = true;
        }

        if pressed && allowed && self.enabled {
            if self.until.is_none() {
                platform.buzzer(true);
            }
            self.until = Some(now_ms.wrapping_add(BEEP_MS));
        }

        let Some(until) = self.until else {
            return u32::MAX;
        };
        if deadline_passed(now_ms, until) {
            platform.buzzer(false);
            self.until = None;
            return u32::MAX;
        }
        until.wrapping_sub(now_ms)
    }

    /// Whether a beep sounds now.
    pub const fn sounding(&self) -> bool {
        self.until.is_some()
    }
}
