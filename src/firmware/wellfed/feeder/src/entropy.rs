//! The noise measurement (the `entropy` variant): streams raw samples
//! from each noise source over UART, so the host can estimate their
//! min-entropy (SP 800-90B) before the generator claims any.
//!
//! Each source gives [`SAMPLES`] one-byte samples, [`LINE`] to a record,
//! numbered so the host can tell a lost line from a short run. Records are
//! paced: logging holds the scheduler while UART drains (about 16 ms a
//! line at 115200), so the loop waits between lines to leave the other
//! tasks their time. The health tests run beside the stream at a placeholder
//! claim of one bit a sample, and the last record says whether they passed:
//! the first physical check of the tests themselves.

use wellfed_core::base64;
use wellfed_core::named_enum;
use wellfed_core::rng::Health;
use wellfed_sdk::hal::entropy::{self, Jitter};
use wellfed_sdk::hal::{adc, sdk};

/// Samples per record: the longest line, the jitter source's last record
/// with 152 base64 characters, fills 190 of the 192-byte line buffer with
/// its NUL. A longer line would fail to format and vanish.
pub const LINE: usize = 112;
/// Records per source.
pub const LINES: u32 = 8_929;
/// Samples per source: SP 800-90B's assessment wants a million.
pub const SAMPLES: u32 = LINES * 112;
const _: () = assert!(LINE == 112 && SAMPLES >= 1_000_000);
/// The placeholder claim for the on-device health tests, in eighths of a bit.
const CLAIM_EIGHTHS: u32 = 8;
/// Between records, for the other tasks.
const PACE_MS: u32 = 25;
/// Missed samples (no 32 kHz edge, a conversion that never completed) a
/// record may skip before its source is given up.
const MISS_LIMIT: u32 = 1_000;
/// The chip's internal supply channel: every AmebaD board has it.
const ADC_CHANNEL: u32 = sdk::board::ADC_SUPPLY;
const ADC_POLLS: u32 = 10_000;

named_enum! {
    pub enum Source {
        Jitter = "jitter",
        Adc = "adc",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start {
        source: Source,
    },
    Raw {
        source: Source,
        seq: u32,
        bytes: [u8; LINE],
    },
    Done {
        source: Source,
        samples: u32,
        missed: u32,
        /// The sample at which a health test failed, if one did.
        failed_at: Option<u32>,
    },
    /// The source stopped giving samples, or the ADC did not open.
    GaveUp {
        source: Source,
        samples: u32,
    },
    Finished,
}

pub fn describe(record: Record, f: &mut impl core::fmt::Write) -> core::fmt::Result {
    match record {
        Record::Start { source } => write!(f, "start src={source} samples={SAMPLES} line={LINE}"),
        Record::Raw { source, seq, bytes } => {
            let mut text = [0; base64::encoded_len(LINE)];
            let text = base64::encode(&bytes, &mut text).ok_or(core::fmt::Error)?;
            write!(f, "raw src={source} seq={seq} b64={text}")
        }
        Record::Done {
            source,
            samples,
            missed,
            failed_at,
        } => {
            write!(
                f,
                "done src={source} samples={samples} missed={missed} health="
            )?;
            match failed_at {
                Some(at) => write!(f, "failed at={at}"),
                None => f.write_str("passed"),
            }
        }
        Record::GaveUp { source, samples } => write!(f, "error src={source} samples={samples}"),
        Record::Finished => f.write_str("finished"),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Capture {
    /// `None` once every source is done.
    source: Option<Source>,
    started: bool,
    /// The next record's number within the source.
    seq: u32,
    samples: u32,
    missed: u32,
    failed_at: Option<u32>,
    health: Health,
    jitter: Jitter,
    /// Whether the ADC opened, once tried.
    adc: Option<bool>,
}

impl Default for Capture {
    fn default() -> Self {
        Self::new()
    }
}

impl Capture {
    pub const fn new() -> Self {
        Self {
            source: Some(Source::Jitter),
            started: false,
            seq: 0,
            samples: 0,
            missed: 0,
            failed_at: None,
            health: Health::new(CLAIM_EIGHTHS),
            jitter: Jitter::new(),
            adc: None,
        }
    }

    /// One record; returns the delay before the next step, or `None` once
    /// finished.
    pub fn step(&mut self, mut record: impl FnMut(Record)) -> Option<u32> {
        let source = self.source?;
        if !self.started {
            self.started = true;
            record(Record::Start { source });
            return Some(PACE_MS);
        }
        if self.samples >= SAMPLES {
            record(Record::Done {
                source,
                samples: self.samples,
                missed: self.missed,
                failed_at: self.failed_at,
            });
            self.next(&mut record);
            return Some(PACE_MS);
        }

        let mut bytes = [0; LINE];
        let mut misses = 0;
        let mut filled = 0;
        while filled < LINE {
            let Some(value) = self.sample(source) else {
                misses += 1;
                if misses > MISS_LIMIT {
                    record(Record::GaveUp {
                        source,
                        samples: self.samples,
                    });
                    self.next(&mut record);
                    return Some(PACE_MS);
                }
                continue;
            };
            if self.health.sample(value).is_err() && self.failed_at.is_none() {
                self.failed_at = Some(self.samples);
            }
            bytes[filled] = value;
            filled += 1;
            self.samples += 1;
        }
        self.missed += misses;

        record(Record::Raw {
            source,
            seq: self.seq,
            bytes,
        });
        self.seq += 1;
        Some(PACE_MS)
    }

    fn sample(&mut self, source: Source) -> Option<u8> {
        match source {
            Source::Jitter => self.jitter.sample(),
            Source::Adc => {
                let open = *self
                    .adc
                    .get_or_insert_with(|| adc::open().calibration.is_some());
                open.then(|| entropy::adc_sample(ADC_CHANNEL, ADC_POLLS))?
            }
        }
    }

    fn next(&mut self, record: &mut impl FnMut(Record)) {
        self.source = match self.source {
            Some(Source::Jitter) => Some(Source::Adc),
            _ => None,
        };
        self.started = false;
        self.seq = 0;
        self.samples = 0;
        self.missed = 0;
        self.failed_at = None;
        self.health = const { Health::new(CLAIM_EIGHTHS) };
        if self.source.is_none() {
            if self.adc == Some(true) {
                adc::close();
            }
            record(Record::Finished);
        }
    }
}
