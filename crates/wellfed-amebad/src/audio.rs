//! FW24: the speaker. Stock 1.1.15 plays through the codec's line output and
//! holds PB23 HIGH while it plays; this port does the same, one built-in tune
//! at a time (`wellfed_core::tone`).
//!
//! A signed `play` request queues a tune in a one-slot mailbox; the audio task
//! opens the codec, enables the amplifier, streams the tune into the DMA ring,
//! waits for the ring to drain and closes everything again. The slot stays
//! taken until then, so a second request is `busy`, never queued behind. The
//! FW15 `sound` setting mutes requests and its `volume` sets loudness.
//! Nothing here touches the motor, the emitters or flash, and the transport
//! only reaches [`request`].

use core::fmt;
use core::sync::atomic::{AtomicU8, Ordering};

use wellfed_core::tone::{Player, SAMPLE_RATE, Tune};
use wellfed_core::transport::PlayResult;

use crate::sdk;
use crate::settings;

/// The amplifier enable (stock writes it HIGH around playback).
pub const AMP_PIN: u32 = 32 + 23;
pub const POLL_MS: u32 = 20;
/// Zero pages the amplifier gets to settle before the first note.
const WARM_MS: u32 = 60;
/// Longer than the ring (8 pages of 16 ms) takes to play out.
const DRAIN_MS: u32 = 400;
/// Beyond the tune's own length: a DMA that stops taking pages still ends
/// playback, so the amplifier is never left on.
const LATE_MS: u32 = 1000;

const EMPTY: u8 = 0;

/// `EMPTY`, or the queued or playing tune's code + 1.
static SLOT: AtomicU8 = AtomicU8::new(EMPTY);

/// The transport's half: queue `tune` unless muted or busy.
pub fn request(tune: Tune) -> PlayResult {
    let Some(view) = settings::current() else {
        return PlayResult::Busy;
    };
    if !view.sound {
        return PlayResult::Muted;
    }
    match SLOT.compare_exchange(EMPTY, tune.code() + 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => PlayResult::Started,
        Err(_) => PlayResult::Busy,
    }
}

/// Forget everything (host tests).
pub fn reset() {
    SLOT.store(EMPTY, Ordering::Release);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Play {
        tune: Tune,
        volume: u8,
    },
    Done {
        tune: Tune,
        ms: u32,
        underruns: u32,
        drained: bool,
    },
    Muted {
        tune: Tune,
    },
    Error {
        stage: &'static str,
    },
}

/// The body of a `PLAUD1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Play { tune, volume } => write!(f, "play tune={} volume={volume}", tune.name()),
        Record::Done {
            tune,
            ms,
            underruns,
            drained,
        } => write!(
            f,
            "done tune={} ms={ms} underruns={underruns} drained={}",
            tune.name(),
            u8::from(drained)
        ),
        Record::Muted { tune } => write!(f, "muted tune={}", tune.name()),
        Record::Error { stage } => write!(f, "error stage={stage}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

#[derive(Clone, Debug, Default)]
enum Stage {
    /// The startup chime waits for the settings to load.
    #[default]
    Startup,
    Idle,
    Warm {
        until: u32,
    },
    Play,
    Drain {
        since: u32,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Speaker {
    stage: Stage,
    playing: Option<(Tune, Player)>,
    started: u32,
    /// The DMA's underrun count when streaming began, then the tune's own.
    underruns: u32,
    amp_ready: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn amp(on: bool) {
    sdk::gpio_write_bit(AMP_PIN, on);
}

impl Speaker {
    /// Configure the amplifier enable as an output, LOW, before its first use.
    fn prepare_amp(&mut self) {
        if !self.amp_ready {
            amp(false);
            sdk::gpio_no_pull(AMP_PIN);
            sdk::gpio_init(AMP_PIN, true);
            amp(false);
            self.amp_ready = true;
        }
    }

    fn finish(&mut self, drained: bool) -> Record {
        amp(false);
        sdk::audio_close();
        let (tune, _) = self.playing.take().expect("finish while playing");
        SLOT.store(EMPTY, Ordering::Release);
        self.stage = Stage::Idle;
        Record::Done {
            tune,
            ms: sdk::uptime_ms().wrapping_sub(self.started),
            underruns: self.underruns,
            drained,
        }
    }

    pub fn step(&mut self) -> Step {
        let now = sdk::uptime_ms();
        match self.stage {
            Stage::Startup => {
                let Some(view) = settings::current() else {
                    return next(POLL_MS * 5, None);
                };
                self.stage = Stage::Idle;
                if !view.sound {
                    return next(POLL_MS, Some(Record::Muted { tune: Tune::Chime }));
                }
                let _ = SLOT.compare_exchange(
                    EMPTY,
                    Tune::Chime.code() + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                next(POLL_MS, None)
            }

            Stage::Idle => {
                let slot = SLOT.load(Ordering::Acquire);
                let Some(tune) = slot.checked_sub(1).and_then(Tune::from_code) else {
                    return next(POLL_MS, None);
                };
                let volume = settings::current().map_or(0, |view| view.volume);
                self.prepare_amp();
                if !sdk::audio_open(SAMPLE_RATE) {
                    SLOT.store(EMPTY, Ordering::Release);
                    return next(POLL_MS, Some(Record::Error { stage: "open" }));
                }
                amp(true);
                self.playing = Some((tune, Player::new(tune, volume)));
                self.started = now;
                self.stage = Stage::Warm {
                    until: now.wrapping_add(WARM_MS),
                };
                next(POLL_MS, Some(Record::Play { tune, volume }))
            }

            Stage::Warm { until } => {
                if (now.wrapping_sub(until) as i32) < 0 {
                    return next(POLL_MS, None);
                }
                self.underruns = sdk::audio_underruns();
                self.stage = Stage::Play;
                self.step()
            }

            Stage::Play => {
                let (tune, player) = self.playing.as_mut().expect("playing");
                let bound = WARM_MS + tune.duration_ms() + LATE_MS;
                if now.wrapping_sub(self.started) >= bound {
                    self.underruns = sdk::audio_underruns().wrapping_sub(self.underruns);
                    return next(POLL_MS, Some(self.finish(false)));
                }
                while let Some(page) = sdk::audio_page() {
                    player.fill(page);
                    sdk::audio_submit();
                    if player.done() {
                        break;
                    }
                }
                if player.done() {
                    // Zero pages the DMA played for want of a filled one.
                    self.underruns = sdk::audio_underruns().wrapping_sub(self.underruns);
                    self.stage = Stage::Drain { since: now };
                }
                next(POLL_MS, None)
            }

            Stage::Drain { since } => {
                let drained = sdk::audio_drained();
                if drained || now.wrapping_sub(since) >= DRAIN_MS {
                    return next(POLL_MS, Some(self.finish(drained)));
                }
                next(POLL_MS, None)
            }
        }
    }
}
