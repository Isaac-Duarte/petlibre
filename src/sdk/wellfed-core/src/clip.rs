//! The meal call: one clip the owner uploads, stored on flash as 8 kHz
//! 4-bit IMA ADPCM and played through the speaker before a plan's meal.
//!
//! **Why this format.** A client converts the owner's recording on the
//! host, so the feeder only decodes. IMA ADPCM packs two samples a byte and
//! decodes with a table lookup and a few adds per sample, with no buffer:
//! 8 kHz voice fits about 8 s into the eight sectors the store has. The
//! codec runs at 16 kHz ([`crate::tone::SAMPLE_RATE`]), so the player
//! doubles each sample, the second of each pair halfway to the next.
//!
//! **The stream.** Byte `i` holds sample `2i` in its low nibble and
//! `2i + 1` in its high one. The encoder and decoder both start from a
//! predictor of 0 and step index 0, so the stream carries no header and a
//! repeat starts the decoder afresh.
//!
//! **The store** keeps a header in the first [`DATA_AT`] bytes of its first
//! sector and the stream from there on, so every [`CHUNK_MAX`]-byte chunk
//! of an upload lands within one sector.

use crate::tone;

/// The clip's sample rate.
pub const RATE_HZ: u32 = 8_000;
/// Where the stream starts in the store: the header's share of its first
/// sector, a whole chunk.
pub const DATA_AT: u32 = 512;
/// The most stream bytes one upload chunk carries.
pub const CHUNK_MAX: u32 = 512;
/// Sectors the store holds, header included.
pub const SECTORS: u32 = 8;
/// A storage sector's bytes.
pub const SECTOR_BYTES: u32 = 4096;
/// The longest stream the store holds: 64,512 samples, about 8.06 s.
pub const BYTES_MAX: u32 = SECTORS * SECTOR_BYTES - DATA_AT;
/// The most times one meal plays the call.
pub const PLAYS_MAX: u8 = 5;

const _: () = assert!(DATA_AT.is_multiple_of(CHUNK_MAX) && SECTOR_BYTES.is_multiple_of(CHUNK_MAX));

/// How long a stream of `bytes` plays once, in ms.
pub const fn ms(bytes: u32) -> u32 {
    // Two samples a byte at 8 kHz: a quarter of a millisecond a byte.
    bytes.div_ceil(4)
}

/// IMA ADPCM's step sizes.
const STEPS: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
/// How each code's magnitude moves the step index.
const INDEX_STEP: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const INDEX_MAX: u8 = 88;

/// The state both directions keep: the last sample and the step index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct State {
    predictor: i32,
    index: u8,
}

impl State {
    fn step(self) -> i32 {
        i32::from(STEPS[usize::from(self.index.min(INDEX_MAX))])
    }

    /// Apply `code` (4 bits) and return the new sample.
    fn apply(&mut self, code: u8) -> i16 {
        let step = self.step();
        let mut diff = step >> 3;
        if code & 4 != 0 {
            diff += step;
        }
        if code & 2 != 0 {
            diff += step >> 1;
        }
        if code & 1 != 0 {
            diff += step >> 2;
        }
        if code & 8 != 0 {
            diff = -diff;
        }
        let predictor = (self.predictor + diff).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        self.predictor = predictor;

        let index = i16::from(self.index) + i16::from(INDEX_STEP[usize::from(code & 7)]);
        self.index = u8::try_from(index.clamp(0, i16::from(INDEX_MAX))).unwrap_or(0);
        i16::try_from(predictor).unwrap_or(0)
    }
}

/// Encode `samples` (8 kHz, 16-bit mono) as the stream, two a byte, low
/// nibble first; an odd count pads its last byte with a silent code.
pub fn encode(samples: &[i16], mut out: impl FnMut(u8)) {
    let mut state = State::default();
    let mut code = |sample: i16| {
        let step = state.step();
        let mut diff = i32::from(sample) - state.predictor;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        if diff >= step {
            code |= 4;
            diff -= step;
        }
        if diff >= step >> 1 {
            code |= 2;
            diff -= step >> 1;
        }
        if diff >= step >> 2 {
            code |= 1;
        }
        // The decoder's own arithmetic, so both sides track one predictor.
        state.apply(code);
        code
    };

    for pair in samples.chunks(2) {
        let low = code(pair[0]);
        let high = pair.get(1).map_or(0, |&sample| code(sample));
        out(low | high << 4);
    }
}

/// Plays the stored stream `plays` times as 16 kHz pages, a byte read at a
/// time, so it needs no buffer of its own.
#[derive(Clone, Debug)]
pub struct ClipPlayer {
    bytes: u32,
    plays_left: u8,
    /// The next byte to read, and the byte being decoded.
    at: u32,
    byte: Option<u8>,
    state: State,
    /// The last sample out, which the next one's halfway sample starts
    /// from.
    last: i16,
    /// The second of a pair, when a page ended between the two.
    held: Option<i16>,
    gain: i32,
    done: bool,
}

impl ClipPlayer {
    /// A player for a stream of `bytes`, `plays` times at `volume`.
    pub fn new(bytes: u32, plays: u8, volume: u8) -> Self {
        Self {
            bytes,
            plays_left: plays,
            at: 0,
            byte: None,
            state: State::default(),
            last: 0,
            held: None,
            gain: tone::gain_q15(volume),
            done: bytes == 0 || plays == 0,
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// How long the clip plays, every play together, in ms.
    pub fn duration_ms(&self) -> u32 {
        ms(self.bytes).saturating_mul(u32::from(self.plays_left))
    }

    /// The next 8 kHz sample, `None` once the last play ends or a read
    /// fails: a stream that cannot be read stops rather than plays noise.
    fn next(&mut self, read: &mut impl FnMut(u32) -> Option<u8>) -> Option<i16> {
        let code = match self.byte.take() {
            Some(byte) => byte >> 4,
            None => {
                if self.at >= self.bytes {
                    self.plays_left = self.plays_left.saturating_sub(1);
                    if self.plays_left == 0 {
                        return None;
                    }
                    self.at = 0;
                    self.state = State::default();
                }
                let byte = read(self.at)?;
                self.at += 1;
                self.byte = Some(byte);
                byte & 0x0f
            }
        };
        Some(self.state.apply(code))
    }

    fn scaled(&self, sample: i16) -> i16 {
        let scaled = i32::from(sample) * self.gain / 32768;
        i16::try_from(scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
    }

    /// Fill `out` with the next 16 kHz samples, read through `read` (the
    /// stream's byte at an offset); silence once the clip ends.
    pub fn fill(&mut self, out: &mut [i16], mut read: impl FnMut(u32) -> Option<u8>) {
        for sample in out {
            *sample = 0;
            if let Some(held) = self.held.take() {
                *sample = held;
                continue;
            }
            if self.done {
                continue;
            }
            let Some(next) = self.next(&mut read) else {
                self.done = true;
                continue;
            };

            let next = self.scaled(next);
            let half = (i32::from(self.last) + i32::from(next)) / 2;
            *sample = i16::try_from(half).unwrap_or(0);
            self.held = Some(next);
            self.last = next;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;

    fn sine(hz: u32, samples: usize, amplitude: f64) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                let t = i as f64 / f64::from(RATE_HZ);
                (amplitude * (2.0 * core::f64::consts::PI * f64::from(hz) * t).sin()) as i16
            })
            .collect()
    }

    fn encoded(samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        encode(samples, |byte| out.push(byte));
        out
    }

    fn play(stream: &[u8], plays: u8, volume: u8) -> Vec<i16> {
        let mut player = ClipPlayer::new(u32::try_from(stream.len()).unwrap(), plays, volume);
        let mut out = Vec::new();
        while !player.done() {
            let mut page = [0i16; 256];
            player.fill(&mut page, |at| stream.get(at as usize).copied());
            out.extend_from_slice(&page);
        }
        out
    }

    #[test]
    fn the_store_fits_whole_chunks_and_eight_seconds() {
        assert!(BYTES_MAX.is_multiple_of(CHUNK_MAX));
        assert_eq!(ms(BYTES_MAX), 8_064);
        assert_eq!(ms(1), 1);
    }

    #[test]
    fn encodes_two_samples_a_byte() {
        assert_eq!(encoded(&[0; 10]).len(), 5);
        assert_eq!(encoded(&[0; 11]).len(), 6);
        assert!(encoded(&[]).is_empty());
    }

    #[test]
    fn a_tone_survives_the_round_trip() {
        let source = sine(440, 8_000, 20_000.0);
        let stream = encoded(&source);
        let mut player = ClipPlayer::new(u32::try_from(stream.len()).unwrap(), 1, 100);
        let mut out = vec_of(32_000);
        player.fill(&mut out, |at| stream.get(at as usize).copied());

        // Every second output is the decoded sample at full volume's gain;
        // ADPCM tracks a steady tone to within a few percent of full scale
        // once its step has adapted.
        let gain = f64::from(tone::gain_q15(100)) / 32768.0;
        let error = source[400..]
            .iter()
            .zip(out[801..].iter().step_by(2))
            .map(|(&a, &b)| (f64::from(a) * gain - f64::from(b)).abs())
            .fold(0.0, f64::max);
        assert!(error < 1_500.0, "{error}");
    }

    fn vec_of(len: usize) -> Vec<i16> {
        std::vec![0; len]
    }

    #[test]
    fn doubles_the_rate_with_halfway_samples() {
        let stream = encoded(&sine(200, 400, 10_000.0));
        let out = play(&stream, 1, 100);
        for i in (2..798).step_by(2) {
            let mid = (i32::from(out[i - 1]) + i32::from(out[i + 1])) / 2;
            assert!((i32::from(out[i]) - mid).abs() <= 1, "at {i}");
        }
    }

    #[test]
    fn plays_the_asked_number_of_times_then_silence() {
        let stream = encoded(&sine(300, 1_000, 10_000.0));
        let once = play(&stream, 1, 100);
        let thrice = play(&stream, 3, 100);
        let sounding = |samples: &[i16]| samples.iter().rposition(|&s| s != 0).unwrap_or(0);
        assert!(sounding(&once) < 2_000);
        assert!((5_900..6_000).contains(&sounding(&thrice)));
        // Each repeat starts the decoder afresh, so it plays the same.
        assert_eq!(thrice[2_100..2_900], thrice[100..900]);
        assert!(ClipPlayer::new(500, 3, 100).duration_ms() == 375);
    }

    #[test]
    fn volume_zero_is_silent_and_no_plays_is_done() {
        let stream = encoded(&sine(300, 1_000, 30_000.0));
        assert!(play(&stream, 1, 0).iter().all(|&s| s == 0));
        assert!(ClipPlayer::new(100, 0, 50).done());
        assert!(ClipPlayer::new(0, 2, 50).done());
    }

    #[test]
    fn a_failed_read_stops_the_clip() {
        let mut player = ClipPlayer::new(1_000, 2, 100);
        let mut page = [1i16; 256];
        player.fill(&mut page, |at| (at < 10).then_some(0x77));
        assert!(player.done());
        assert!(page[40..].iter().all(|&s| s == 0));
    }

    #[test]
    fn decoding_never_overflows() {
        // Every code, repeated, drives the predictor to either rail.
        for code in 0..=255u8 {
            let stream = [code; 400];
            let out = play(&stream, 1, 100);
            assert_eq!(out.len() % 256, 0);
        }
    }
}
