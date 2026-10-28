//! The meal call's clip on the client's side: the owner's recording
//! converted to the feeder's stream, and the upload's decisions, sans-IO.
//!
//! **Conversion.** A WAV file (integer PCM in 8- to 32-bit containers, or
//! 32- or 64-bit float, any rate, any number of channels; a data chunk of
//! unknown length, as a recorder streaming its file leaves it, runs to the
//! end of the file) is mixed down to mono,
//! resampled to [`CALL_RATE_HZ`] through a windowed-sinc low-pass, trimmed
//! of near-silence at either end, scaled so its peak sits just under full
//! scale (the feeder's volume setting then decides how loud it plays), and
//! encoded as 4-bit IMA ADPCM with the feeder's own encoder
//! (`wellfed_core::clip`). Other formats are converted to WAV first, for
//! example with `ffmpeg -i call.m4a call.wav`.
//!
//! **Upload.** [`Clip::start`] erases the feeder's clip and announces the
//! new one's length and CRC; [`Clip::chunk`] sends the stream a chunk at a
//! time, each where the upload stands. [`Clip::after_chunk`] reads each
//! answer: the next offset, the same chunk again while the feeder is still
//! writing the last (`Busy`), or a read of where it stands after a chunk
//! it did not want. Once the last chunk is in, the feeder reads the stream
//! back against the CRC: [`Clip::settled`] says whether it stored it.
//! Nothing here feeds, and a chunk is idempotent by offset, so resending
//! one is always safe.

use wellfed_core::clip;
use wellfed_core::store::slot::crc32;
use wellfed_model::{
    Answer, CALL_BYTES_MAX, CALL_CHUNK_MAX, CALL_RATE_HZ, CallClip, CallView, Op, Refusal,
};

/// The loudest sample after scaling: a little headroom under full scale.
const PEAK: f64 = 0.9 * 32_767.0;
/// Near-silence at either end, against the recording's own peak.
const SILENCE: f64 = 0.01;
/// Zero crossings of the resampling kernel on each side.
const ZEROS: f64 = 12.0;
/// The low-pass's cutoff against the lower rate's Nyquist frequency.
const CUTOFF: f64 = 0.9;

const _: () = assert!(CALL_RATE_HZ == clip::RATE_HZ && CALL_BYTES_MAX == clip::BYTES_MAX);

/// Why a file cannot become a meal call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClipError {
    #[error("not a WAV file (RIFF/WAVE)")]
    NotWav,
    #[error("the WAV file ends inside a chunk")]
    Truncated,
    #[error("the WAV file has no {0} chunk")]
    Missing(&'static str),
    #[error(
        "WAV format {format} with {bits}-bit samples is not supported: use integer PCM or float"
    )]
    Unsupported { format: u16, bits: u16 },
    #[error("the WAV file names {channels} channels at {rate} Hz")]
    Shape { channels: u16, rate: u32 },
    #[error("the recording is silent")]
    Silent,
    #[error("the recording plays {ms} ms once trimmed; the feeder keeps at most {max_ms} ms")]
    TooLong { ms: u32, max_ms: u32 },
}

/// Why an upload stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UploadError {
    #[error("the feeder refused the upload: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered something other than the meal call")]
    Unexpected,
    #[error("the feeder no longer receives this upload (restarted, or another began)")]
    Lost,
    #[error("the feeder read the stream back and it did not match: it keeps no clip")]
    Rejected,
}

/// A converted clip, ready to upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    stream: Vec<u8>,
    crc: u32,
}

/// What to do after a chunk's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    /// Send the chunk at this offset next; the stream's length once every
    /// chunk is in.
    Next(u32),
    /// The feeder is still writing the chunk before: send this one again
    /// shortly.
    Retry,
    /// The feeder did not want that chunk: ask where the upload stands
    /// ([`Op::GetCall`]) and resume there ([`Clip::resume`]).
    Ask,
}

impl Clip {
    /// The clip in a WAV file's bytes.
    pub fn from_wav(file: &[u8]) -> Result<Self, ClipError> {
        let wav = Wav::parse(file)?;
        let mono = wav.mono();
        let resampled = resample(&mono, wav.rate, CALL_RATE_HZ);
        let samples = scaled(trimmed(&resampled))?;

        let max = usize::try_from(CALL_BYTES_MAX * 2).unwrap_or(usize::MAX);
        if samples.len() > max {
            let rate = usize::try_from(CALL_RATE_HZ).unwrap_or(usize::MAX);
            let ms = |samples: usize| u32::try_from(samples * 1000 / rate).unwrap_or(u32::MAX);
            return Err(ClipError::TooLong {
                ms: ms(samples.len()),
                max_ms: ms(max),
            });
        }
        let mut stream = Vec::with_capacity(samples.len().div_ceil(2));
        clip::encode(&samples, |byte| stream.push(byte));
        let crc = crc32(&stream);
        Ok(Self { stream, crc })
    }

    /// The stream's bytes.
    pub fn bytes(&self) -> u32 {
        u32::try_from(self.stream.len()).unwrap_or(u32::MAX)
    }

    pub fn crc(&self) -> u32 {
        self.crc
    }

    /// How long it plays once, in milliseconds.
    pub fn ms(&self) -> u32 {
        clip::ms(self.bytes())
    }

    /// The feeder's stream.
    pub fn stream(&self) -> &[u8] {
        &self.stream
    }

    /// The op that erases the feeder's clip and starts this upload.
    pub fn start(&self) -> Op {
        Op::StartCall {
            bytes: self.bytes(),
            crc: self.crc,
        }
    }

    /// The chunk at `offset`; `None` past the stream's end.
    pub fn chunk(&self, offset: u32) -> Option<Op> {
        let start = usize::try_from(offset).ok()?;
        let data = self.stream.get(start..)?;
        if data.is_empty() {
            return None;
        }
        let data = &data[..data.len().min(CALL_CHUNK_MAX)];
        Some(Op::CallChunk {
            offset,
            data: data.to_vec(),
        })
    }

    /// What follows the answer to the chunk at `offset`.
    pub fn after_chunk(&self, offset: u32, answer: &Answer) -> Result<Sent, UploadError> {
        match answer {
            Answer::Call(_) => {
                let len = self.bytes().saturating_sub(offset);
                let sent = len.min(u32::try_from(CALL_CHUNK_MAX).unwrap_or(u32::MAX));
                Ok(Sent::Next(offset.saturating_add(sent)))
            }
            Answer::Refused(Refusal::Busy) => Ok(Sent::Retry),
            Answer::Refused(Refusal::Invalid) => Ok(Sent::Ask),
            Answer::Refused(refusal) => Err(UploadError::Refused(*refusal)),
            _ => Err(UploadError::Unexpected),
        }
    }

    /// Where to resume, from the feeder's view of the call.
    pub fn resume(&self, view: &CallView) -> Result<u32, UploadError> {
        match view.clip {
            CallClip::Receiving {
                bytes,
                crc,
                received,
            } if self.ours(bytes, crc) => Ok(received),
            _ => Err(UploadError::Lost),
        }
    }

    /// Once every chunk is in: whether the feeder has stored the clip
    /// (`false` while it is still reading it back).
    pub fn settled(&self, view: &CallView) -> Result<bool, UploadError> {
        match view.clip {
            CallClip::Stored { bytes, crc } if self.ours(bytes, crc) => Ok(true),
            CallClip::Receiving { bytes, crc, .. } if self.ours(bytes, crc) => Ok(false),
            CallClip::None => Err(UploadError::Rejected),
            CallClip::Stored { .. } | CallClip::Receiving { .. } => Err(UploadError::Lost),
        }
    }

    fn ours(&self, bytes: u32, crc: u32) -> bool {
        bytes == self.bytes() && crc == self.crc
    }
}

/// A WAV file's samples, as floats in -1 to 1, interleaved.
struct Wav {
    channels: usize,
    rate: u32,
    samples: Vec<f64>,
}

/// WAVE_FORMAT_PCM, WAVE_FORMAT_IEEE_FLOAT and WAVE_FORMAT_EXTENSIBLE,
/// whose subformat's first two bytes are one of the first two.
const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xfffe;

impl Wav {
    fn parse(file: &[u8]) -> Result<Self, ClipError> {
        if file.len() < 12 || &file[..4] != b"RIFF" || &file[8..12] != b"WAVE" {
            return Err(ClipError::NotWav);
        }
        let mut format = None;
        let mut data = None;
        let mut at = 12;
        while at + 8 <= file.len() {
            let id = &file[at..at + 4];
            let len = u32::from_le_bytes([file[at + 4], file[at + 5], file[at + 6], file[at + 7]]);
            // A recorder that streams its file cannot know the length when
            // it writes the header, and leaves 0 or all ones: the samples
            // then run to the end of the file.
            if id == b"data" && (len == 0 || len == u32::MAX) {
                data = file.get(at + 8..);
                break;
            }
            let len = usize::try_from(len).map_err(|_| ClipError::Truncated)?;
            let body = file.get(at + 8..at + 8 + len).ok_or(ClipError::Truncated)?;
            match id {
                b"fmt " => format = Some(body),
                b"data" => data = Some(body),
                _ => {}
            }
            // Chunks are padded to an even length.
            at += 8 + len + len % 2;
        }
        let format = format.ok_or(ClipError::Missing("fmt "))?;
        let data = data.ok_or(ClipError::Missing("data"))?;
        if format.len() < 16 {
            return Err(ClipError::Truncated);
        }

        let word = |at: usize| u16::from_le_bytes([format[at], format[at + 1]]);
        let mut tag = word(0);
        let channels = word(2);
        let rate = u32::from_le_bytes([format[4], format[5], format[6], format[7]]);
        let align = word(12);
        let mut bits = word(14);
        if tag == EXTENSIBLE && format.len() >= 26 {
            // The container's bits are the field above; the valid ones,
            // aligned to its top, come here.
            let valid = word(18);
            if valid != 0 {
                bits = valid;
            }
            tag = word(24);
        }
        if channels == 0 || !(1_000..=384_000).contains(&rate) {
            return Err(ClipError::Shape { channels, rate });
        }
        // Each sample sits in a container of the block's share of bytes,
        // its valid bits at the top, so the container decides the decode.
        let width = align / channels;
        if align == 0 || width * channels != align || bits == 0 || bits > width * 8 {
            return Err(ClipError::Unsupported { format: tag, bits });
        }
        let decode: fn(&[u8]) -> f64 = match (tag, width) {
            (PCM, 1) => |b| (f64::from(b[0]) - 128.0) / 128.0,
            (PCM, 2) => |b| f64::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
            (PCM, 3) => |b| f64::from(i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) / 8_388_608.0,
            (PCM, 4) => {
                |b| f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])) / 2_147_483_648.0
            }
            (FLOAT, 4) if bits == 32 => |b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            (FLOAT, 8) if bits == 64 => {
                |b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            (format, _) => return Err(ClipError::Unsupported { format, bits }),
        };
        let samples = data
            .chunks_exact(usize::from(width))
            .map(|bytes| decode(bytes).clamp(-1.0, 1.0))
            .collect();
        Ok(Self {
            channels: usize::from(channels),
            rate,
            samples,
        })
    }

    /// Every channel averaged.
    fn mono(&self) -> Vec<f64> {
        self.samples
            .chunks_exact(self.channels)
            .map(|frame| frame.iter().sum::<f64>() / frame.len() as f64)
            .collect()
    }
}

/// `input` at `from` Hz, at `to` Hz: a Hann-windowed sinc, its cutoff
/// under the lower rate's Nyquist frequency, so nothing above it folds
/// back into the call.
#[expect(
    clippy::cast_possible_truncation,
    reason = "positions in the input, non-negative and below its length once clamped"
)]
fn resample(input: &[f64], from: u32, to: u32) -> Vec<f64> {
    if input.is_empty() {
        return Vec::new();
    }
    let step = f64::from(from) / f64::from(to);
    // The cutoff as a fraction of the input's Nyquist frequency.
    let cutoff = CUTOFF * (1.0 / step).min(1.0);
    let half = (ZEROS / cutoff).ceil();
    let len = (input.len() as f64 / step).floor() as usize;

    (0..len)
        .map(|n| {
            let centre = n as f64 * step;
            let first = (centre - half).ceil().max(0.0) as usize;
            let last = ((centre + half).floor() as usize).min(input.len() - 1);
            (first..=last)
                .map(|k| {
                    let d = centre - k as f64;
                    let x = cutoff * d;
                    let sinc = if x.abs() < 1e-12 {
                        1.0
                    } else {
                        (core::f64::consts::PI * x).sin() / (core::f64::consts::PI * x)
                    };
                    let window = 0.5 + 0.5 * (core::f64::consts::PI * d / half).cos();
                    input[k] * cutoff * sinc * window
                })
                .sum()
        })
        .collect()
}

/// `samples` without the near-silence at either end.
fn trimmed(samples: &[f64]) -> &[f64] {
    let peak = samples.iter().fold(0.0_f64, |peak, s| peak.max(s.abs()));
    let loud = |s: &f64| s.abs() > peak * SILENCE;
    let first = samples.iter().position(loud).unwrap_or(samples.len());
    let last = samples
        .iter()
        .rposition(loud)
        .map_or(first, |last| last + 1);
    &samples[first..last.max(first)]
}

/// `samples` scaled so the peak sits at [`PEAK`], as 16-bit samples.
#[expect(
    clippy::cast_possible_truncation,
    reason = "each sample is rounded and clamped to the 16-bit range first"
)]
fn scaled(samples: &[f64]) -> Result<Vec<i16>, ClipError> {
    let peak = samples.iter().fold(0.0_f64, |peak, s| peak.max(s.abs()));
    if peak < 1e-6 {
        return Err(ClipError::Silent);
    }
    let gain = PEAK / peak;
    Ok(samples
        .iter()
        .map(|s| (s * gain).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_model::{CALL_CHUNK_MAX, Refusal};

    /// A 16-bit WAV file of `samples` per channel at `rate`.
    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut file = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        file.extend(16u32.to_le_bytes());
        file.extend(PCM.to_le_bytes());
        file.extend(channels.to_le_bytes());
        file.extend(rate.to_le_bytes());
        file.extend((rate * 2 * u32::from(channels)).to_le_bytes());
        file.extend((2 * channels).to_le_bytes());
        file.extend(16u16.to_le_bytes());
        // A chunk the reader skips, odd-sized, so its padding counts.
        file.extend(b"LIST");
        file.extend(3u32.to_le_bytes());
        file.extend([1, 2, 3, 0]);
        file.extend(b"data");
        file.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
        file.extend(data);
        file
    }

    fn tone(rate: u32, hz: f64, seconds: f64, amplitude: f64) -> Vec<i16> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                (amplitude * (2.0 * core::f64::consts::PI * hz * t).sin()) as i16
            })
            .collect()
    }

    /// The stream decoded back to 8 kHz samples.
    fn decoded(clip: &Clip) -> Vec<i16> {
        let stream = clip.stream();
        let mut player = clip::ClipPlayer::new(clip.bytes(), 1, 100);
        let mut out = vec![0; stream.len() * 4 + 256];
        player.fill(&mut out, |at| stream.get(at as usize).copied());
        out.iter().skip(1).step_by(2).copied().collect()
    }

    #[test]
    fn a_tone_at_44_1_khz_becomes_8_khz_at_the_peak() {
        let file = wav(44_100, 1, &tone(44_100, 440.0, 1.0, 8_000.0));
        let clip = Clip::from_wav(&file).unwrap();
        // One second at 8 kHz, two samples a byte, give or take the ends.
        assert!((3_980..=4_000).contains(&clip.bytes()), "{}", clip.bytes());
        assert!((995..=1_000).contains(&clip.ms()));
        assert_eq!(clip.crc(), crc32(clip.stream()));

        let samples = decoded(&clip);
        let rising = samples[1_000..7_000]
            .windows(2)
            .filter(|pair| pair[0] < 0 && pair[1] >= 0)
            .count();
        assert!((327..=333).contains(&rising), "{rising} cycles in 0.75 s");
        // Scaled to just under full scale, then through the player's gain.
        let peak = samples.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        let expected = PEAK * f64::from(wellfed_core::tone::gain_q15(100)) / 32_768.0;
        assert!(
            (f64::from(peak) - expected).abs() < expected * 0.08,
            "{peak}"
        );
    }

    #[test]
    fn a_tone_above_the_new_nyquist_is_filtered_out() {
        // 6 kHz would fold to 2 kHz at 8 kHz without the low-pass. A 440 Hz
        // tone beside it keeps the recording's peak, so the remainder of
        // 6 kHz shows against it.
        let mixed: Vec<i16> = tone(48_000, 6_000.0, 0.5, 8_000.0)
            .iter()
            .zip(tone(48_000, 440.0, 0.5, 8_000.0))
            .map(|(a, b)| a + b)
            .collect();
        let resampled = resample(
            &mixed
                .iter()
                .map(|&s| f64::from(s) / 32_768.0)
                .collect::<Vec<_>>(),
            48_000,
            8_000,
        );
        let clean = resample(
            &tone(48_000, 440.0, 0.5, 8_000.0)
                .iter()
                .map(|&s| f64::from(s) / 32_768.0)
                .collect::<Vec<_>>(),
            48_000,
            8_000,
        );
        let worst = resampled[200..3_800]
            .iter()
            .zip(&clean[200..3_800])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn stereo_is_mixed_down_and_silence_trimmed() {
        let mut frames = vec![0i16; 2 * 8_000];
        frames.extend(
            tone(8_000, 300.0, 0.5, 10_000.0)
                .iter()
                .flat_map(|&s| [s, s / 2]),
        );
        frames.extend(vec![0i16; 2 * 8_000]);
        let clip = Clip::from_wav(&wav(8_000, 2, &frames)).unwrap();
        // Half a second, not two and a half.
        assert!((1_950..=2_020).contains(&clip.bytes()), "{}", clip.bytes());
    }

    #[test]
    fn what_is_not_a_clip_is_named() {
        assert_eq!(Clip::from_wav(b"not a wav"), Err(ClipError::NotWav));
        assert_eq!(
            Clip::from_wav(&wav(8_000, 1, &[0; 800])),
            Err(ClipError::Silent)
        );
        let long = tone(8_000, 300.0, 9.0, 10_000.0);
        assert!(matches!(
            Clip::from_wav(&wav(8_000, 1, &long)),
            Err(ClipError::TooLong { max_ms: 8_064, .. })
        ));
        // ADPCM in, say, is a format of its own.
        let mut adpcm = wav(8_000, 1, &[1, 2]);
        adpcm[20] = 2;
        assert_eq!(
            Clip::from_wav(&adpcm),
            Err(ClipError::Unsupported {
                format: 2,
                bits: 16
            })
        );
        // More valid bits than the container holds.
        let mut wide = wav(8_000, 1, &[1, 2]);
        wide[34] = 17;
        assert_eq!(
            Clip::from_wav(&wide),
            Err(ClipError::Unsupported {
                format: PCM,
                bits: 17
            })
        );
        let file = wav(8_000, 1, &tone(8_000, 300.0, 0.1, 10_000.0));
        assert_eq!(
            Clip::from_wav(&file[..file.len() - 10]),
            Err(ClipError::Truncated)
        );
    }

    #[test]
    fn a_streamed_data_length_runs_to_the_end_of_the_file() {
        let samples = tone(8_000, 300.0, 0.5, 10_000.0);
        let whole = Clip::from_wav(&wav(8_000, 1, &samples)).unwrap();
        for unknown in [0u32, u32::MAX] {
            let mut file = wav(8_000, 1, &samples);
            let at = file.len() - samples.len() * 2 - 4;
            file[at..at + 4].copy_from_slice(&unknown.to_le_bytes());
            assert_eq!(Clip::from_wav(&file), Ok(whole.clone()), "{unknown:#x}");
        }
    }

    #[test]
    fn samples_are_read_by_their_container() {
        // 20 valid bits in 3-byte containers (WAVE_FORMAT_EXTENSIBLE), and
        // 12 in 2-byte ones: each read as its container, valid bits on top.
        let samples = tone(8_000, 300.0, 0.5, 10_000.0);
        let sixteen = Clip::from_wav(&wav(8_000, 1, &samples)).unwrap();

        let mut twelve = wav(8_000, 1, &samples);
        twelve[34] = 12;
        assert_eq!(Clip::from_wav(&twelve), Ok(sixteen.clone()));

        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| {
                let [lo, hi] = s.to_le_bytes();
                [0, lo, hi]
            })
            .collect();
        let mut file = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        file.extend(40u32.to_le_bytes());
        file.extend(EXTENSIBLE.to_le_bytes());
        file.extend(1u16.to_le_bytes());
        file.extend(8_000u32.to_le_bytes());
        file.extend(24_000u32.to_le_bytes());
        file.extend(3u16.to_le_bytes());
        file.extend(24u16.to_le_bytes());
        file.extend(22u16.to_le_bytes());
        file.extend(20u16.to_le_bytes());
        file.extend(4u32.to_le_bytes());
        file.extend(PCM.to_le_bytes());
        file.extend([0; 14]);
        file.extend(b"data");
        file.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
        file.extend(data);
        assert_eq!(Clip::from_wav(&file), Ok(sixteen));
    }

    #[test]
    fn the_upload_walks_the_stream_and_reads_each_answer() {
        let clip = Clip::from_wav(&wav(8_000, 1, &tone(8_000, 300.0, 0.3, 10_000.0))).unwrap();
        let bytes = clip.bytes();
        assert_eq!(
            clip.start(),
            Op::StartCall {
                bytes,
                crc: clip.crc()
            }
        );

        let Some(Op::CallChunk { offset: 0, data }) = clip.chunk(0) else {
            panic!("the first chunk");
        };
        assert_eq!(data.len(), CALL_CHUNK_MAX);
        let view = CallView {
            writable: true,
            clip: CallClip::Receiving {
                bytes,
                crc: clip.crc(),
                received: 512,
            },
        };
        let chunk = u32::try_from(CALL_CHUNK_MAX).unwrap();
        assert_eq!(
            clip.after_chunk(0, &Answer::Call(view)),
            Ok(Sent::Next(chunk))
        );
        assert_eq!(
            clip.after_chunk(chunk, &Answer::Refused(Refusal::Busy)),
            Ok(Sent::Retry)
        );
        assert_eq!(
            clip.after_chunk(chunk, &Answer::Refused(Refusal::Invalid)),
            Ok(Sent::Ask)
        );
        assert_eq!(clip.resume(&view), Ok(512));
        let last = bytes - (bytes - 1) % chunk - 1;
        assert!(clip.chunk(last).is_some());
        assert_eq!(
            clip.after_chunk(last, &Answer::Call(view)),
            Ok(Sent::Next(bytes))
        );
        assert_eq!(clip.chunk(bytes), None);

        let stored = CallView {
            writable: true,
            clip: CallClip::Stored {
                bytes,
                crc: clip.crc(),
            },
        };
        assert_eq!(clip.settled(&view), Ok(false));
        assert_eq!(clip.settled(&stored), Ok(true));
        let none = CallView {
            writable: true,
            clip: CallClip::None,
        };
        assert_eq!(clip.settled(&none), Err(UploadError::Rejected));
        assert_eq!(clip.resume(&none), Err(UploadError::Lost));
    }
}
