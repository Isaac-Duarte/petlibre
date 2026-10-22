//! FW24: built-in tunes rendered as 16 kHz, 16-bit mono PCM for the codec's
//! DMA pages.
//!
//! A tune is a short, fixed list of notes; nothing here comes from the
//! network except the tune's code, so playback is bounded by the longest
//! tune. Each note is a sine with a short attack and release, so starts and
//! stops do not click through the amplifier. Loudness follows the FW15
//! `volume` setting (0-100) on a decibel scale; muting is the caller's
//! decision.

pub const SAMPLE_RATE: u32 = 16_000;
/// Peak amplitude at volume 100: about half full scale, so the small speaker
/// and its amplifier keep headroom.
pub const MAX_AMPLITUDE: i32 = 16_000;
/// Peak amplitude per `volume`: 0 is silent, then 0.3 dB a step from
/// -29.7 dB at 1 to [`MAX_AMPLITUDE`] at 100. A linear scale made 40 and 80
/// sound almost alike on the unit (physical, Oct 22).
const GAIN: [u16; 101] = [
    0, 524, 542, 561, 581, 601, 622, 644, 667, 690, 715, 740, 766, 793, 821, 849, 879, 910, 942,
    975, 1010, 1045, 1082, 1120, 1159, 1200, 1242, 1286, 1331, 1378, 1426, 1476, 1528, 1582, 1637,
    1695, 1754, 1816, 1880, 1946, 2014, 2085, 2158, 2234, 2313, 2394, 2478, 2565, 2655, 2749, 2845,
    2945, 3049, 3156, 3267, 3382, 3500, 3623, 3751, 3883, 4019, 4160, 4306, 4458, 4614, 4777, 4944,
    5118, 5298, 5484, 5677, 5877, 6083, 6297, 6518, 6747, 6984, 7230, 7484, 7747, 8019, 8301, 8593,
    8894, 9207, 9531, 9866, 10212, 10571, 10943, 11327, 11725, 12137, 12564, 13005, 13462, 13935,
    14425, 14932, 15457, 16000,
];
const ATTACK: u32 = 64;
const RELEASE: u32 = 160;

/// One note: `hz` 0 is a rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub hz: u16,
    pub ms: u16,
}

const fn n(hz: u16, ms: u16) -> Note {
    Note { hz, ms }
}

const C5: u16 = 523;
const D5: u16 = 587;
const E5: u16 = 659;
const F5: u16 = 698;
const G5: u16 = 784;
const A5: u16 = 880;
const B5: u16 = 988;
const C6: u16 = 1046;
const E6: u16 = 1318;
const A4: u16 = 440;

const CHIME: [Note; 4] = [n(C5, 90), n(E5, 90), n(G5, 90), n(C6, 260)];
const MEAL: [Note; 7] = [
    n(G5, 130),
    n(E5, 130),
    n(G5, 130),
    n(C6, 320),
    n(0, 120),
    n(G5, 130),
    n(C6, 400),
];
const COIN: [Note; 2] = [n(B5, 80), n(E6, 420)];
const ALERT: [Note; 5] = [n(A4, 160), n(0, 90), n(A4, 160), n(0, 90), n(A4, 160)];
const TEST: [Note; 1] = [n(1000, 1000)];
const TWINKLE: [Note; 14] = [
    n(C5, 280),
    n(C5, 280),
    n(G5, 280),
    n(G5, 280),
    n(A5, 280),
    n(A5, 280),
    n(G5, 520),
    n(F5, 280),
    n(F5, 280),
    n(E5, 280),
    n(E5, 280),
    n(D5, 280),
    n(D5, 280),
    n(C5, 520),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tune {
    /// Startup: a rising major arpeggio.
    Chime,
    /// Mealtime call.
    Meal,
    Coin,
    /// Three short beeps, for faults (FW19/FW23).
    Alert,
    /// One second at 1 kHz, for checking the output path.
    Test,
    Twinkle,
}

impl Tune {
    pub const ALL: [Self; 6] = [
        Self::Chime,
        Self::Meal,
        Self::Coin,
        Self::Alert,
        Self::Test,
        Self::Twinkle,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Chime => "chime",
            Self::Meal => "meal",
            Self::Coin => "coin",
            Self::Alert => "alert",
            Self::Test => "test",
            Self::Twinkle => "twinkle",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tune| tune.name() == name)
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    pub fn notes(self) -> &'static [Note] {
        match self {
            Self::Chime => &CHIME,
            Self::Meal => &MEAL,
            Self::Coin => &COIN,
            Self::Alert => &ALERT,
            Self::Test => &TEST,
            Self::Twinkle => &TWINKLE,
        }
    }

    pub fn duration_ms(self) -> u32 {
        self.notes().iter().map(|note| u32::from(note.ms)).sum()
    }
}

/// A quarter sine period in 64 steps, Q15.
const QUARTER: [i16; 65] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602, 6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530, 18204, 18868, 19519, 20159, 20787,
    21403, 22005, 22594, 23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790, 27245, 27683,
    28105, 28510, 28898, 29268, 29621, 29956, 30273, 30571, 30852, 31113, 31356, 31580, 31785,
    31971, 32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757, 32767,
];

/// sin(2π · phase / 2^32), Q15, by the nearest of 256 steps per period.
fn sine(phase: u32) -> i32 {
    let step = (phase >> 24) as usize;
    let (quadrant, at) = (step / 64, step % 64);
    i32::from(match quadrant {
        0 => QUARTER[at],
        1 => QUARTER[64 - at],
        2 => -QUARTER[at],
        _ => -QUARTER[64 - at],
    })
}

/// Renders one tune, a page at a time.
#[derive(Clone, Debug)]
pub struct Player {
    notes: &'static [Note],
    index: usize,
    /// Samples into the current note, and its length.
    at: u32,
    length: u32,
    phase: u32,
    increment: u32,
    amplitude: i32,
}

impl Player {
    pub fn new(tune: Tune, volume: u8) -> Self {
        let mut player = Self {
            notes: tune.notes(),
            index: 0,
            at: 0,
            length: 0,
            phase: 0,
            increment: 0,
            amplitude: i32::from(GAIN[usize::from(volume.min(100))]),
        };
        player.begin();
        player
    }

    fn begin(&mut self) {
        let Some(note) = self.notes.get(self.index) else {
            return;
        };
        self.at = 0;
        self.length = u32::from(note.ms) * SAMPLE_RATE / 1000;
        self.phase = 0;
        self.increment = ((u64::from(note.hz) << 32) / u64::from(SAMPLE_RATE)) as u32;
    }

    pub fn done(&self) -> bool {
        self.index >= self.notes.len()
    }

    fn envelope(&self) -> i32 {
        let left = self.length - self.at;
        let ramp = self.at.min(ATTACK) * RELEASE / ATTACK;
        ramp.min(left).min(RELEASE) as i32
    }

    /// Fill `out` with the next samples; silence after the tune ends.
    pub fn fill(&mut self, out: &mut [i16]) {
        for sample in out {
            *sample = 0;
            if self.done() {
                continue;
            }
            if self.increment != 0 {
                let level = self.amplitude * self.envelope() / RELEASE as i32;
                *sample = (sine(self.phase) * level / 32768) as i16;
                self.phase = self.phase.wrapping_add(self.increment);
            }
            self.at += 1;
            if self.at >= self.length {
                self.index += 1;
                self.begin();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;

    fn render(tune: Tune, volume: u8) -> Vec<i16> {
        let mut player = Player::new(tune, volume);
        let mut out = Vec::new();
        while !player.done() {
            let mut page = [0i16; 256];
            player.fill(&mut page);
            out.extend_from_slice(&page);
        }
        out
    }

    #[test]
    fn names_and_codes_round_trip() {
        for tune in Tune::ALL {
            assert_eq!(Tune::from_name(tune.name()), Some(tune));
            assert_eq!(Tune::from_code(tune.code()), Some(tune));
        }
        assert_eq!(Tune::from_code(Tune::ALL.len() as u8), None);
        assert_eq!(Tune::from_name("siren"), None);
    }

    #[test]
    fn every_tune_is_short() {
        for tune in Tune::ALL {
            assert!((100..=6_000).contains(&tune.duration_ms()), "{tune:?}");
        }
    }

    #[test]
    fn renders_the_tune_length_then_silence() {
        let samples = render(Tune::Test, 100);
        assert_eq!(samples.len(), 16_000usize.div_ceil(256) * 256);
        assert!(samples[16_000..].iter().all(|&s| s == 0));
    }

    #[test]
    fn test_tone_is_1_khz_at_the_volume_peak() {
        let samples = render(Tune::Test, 100);
        let middle = &samples[4_000..12_000];
        let rising = middle
            .windows(2)
            .filter(|pair| pair[0] < 0 && pair[1] >= 0)
            .count();
        assert!((499..=501).contains(&rising), "{rising} cycles in 0.5 s");

        let peak = middle.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        assert!((MAX_AMPLITUDE - 200..=MAX_AMPLITUDE).contains(&peak));
    }

    #[test]
    fn notes_start_and_end_quietly() {
        for tune in Tune::ALL {
            let samples = render(tune, 100);
            let mut at = 0usize;
            for note in tune.notes() {
                let end = at + usize::from(note.ms) * 16;
                for edge in [at, end - 1] {
                    assert!(i32::from(samples[edge]).abs() < 1_000, "{tune:?} at {edge}");
                }
                at = end;
            }
        }
    }

    #[test]
    fn volume_scales_and_zero_is_silent() {
        let peak = |volume| {
            render(Tune::Test, volume)
                .iter()
                .map(|s| i32::from(*s).abs())
                .max()
                .unwrap()
        };
        assert_eq!(peak(0), 0);
        assert!(peak(1) > 0);
        // 0.3 dB a step: 40 below 80 by 12 dB, a factor of about 4.
        let ratio = f64::from(peak(80)) / f64::from(peak(40));
        assert!((3.9..4.1).contains(&ratio), "{ratio}");
        assert_eq!(peak(255), peak(100));
        assert!(GAIN.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(i32::from(GAIN[100]), MAX_AMPLITUDE);
    }

    #[test]
    fn rests_are_silent() {
        let samples = render(Tune::Alert, 100);
        let rest = 160 * 16..(160 + 90) * 16;
        assert!(samples[rest].iter().all(|&s| s == 0));
    }
}
