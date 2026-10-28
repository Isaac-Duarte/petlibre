//! The speaker. Stock 1.1.15 plays through the codec's line output and
//! holds PB23 HIGH while it plays; this port does the same, one built-in tune
//! at a time (`wellfed_core::tone`).
//!
//! A signed `play` request queues a tune in a one-slot mailbox; the audio task
//! opens the codec, enables the amplifier, streams the tune into the DMA ring,
//! waits for the ring to drain and closes everything again. The slot stays
//! taken until then, so a second request is `busy`, never queued behind. The
//! `sound` setting, a volume of 0 and the sound window (on the local clock,
//! open while the clock is invalid) mute requests, and `volume` sets
//! loudness.
//! Nothing here touches the motor, the emitters or flash, and the device
//! API only reaches [`request`].

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wellfed_core::deadline_passed;
use wellfed_core::named_enum;
pub use wellfed_core::tone::PlayResult;
use wellfed_core::tone::{Player, SAMPLE_RATE, Tune};

use crate::sdk;
use crate::{clock, settings};

/// The amplifier enable (stock writes it HIGH around playback).
pub const AMP_PIN: u32 = crate::plaf103::pins::AMP;
pub const POLL_MS: u32 = 20;
/// Zero pages the amplifier gets to settle before the first note. 60 ms
/// swallowed the 50 ms `key` tick whole while the 160 ms `alert` beeps were
/// heard (physical, Oct 24, `fw45-code-06`), so the amplifier needs longer.
const WARM_MS: u32 = 150;
/// Longer than the ring (8 pages of 16 ms) takes to play out.
const DRAIN_MS: u32 = 400;
/// Beyond the tune's own length: a DMA that stops taking pages still ends
/// playback, so the amplifier is never left on.
const LATE_MS: u32 = 1000;

const EMPTY: u8 = 0;

/// `EMPTY`, or the queued or playing tune's code + 1.
static SLOT: AtomicU8 = AtomicU8::new(EMPTY);
static AMP_READY: AtomicBool = AtomicBool::new(false);

/// Open the codec at `rate_hz`, then enable the amplifier; false (amplifier
/// off) when the codec would not open.
pub fn open_output(rate_hz: u32) -> bool {
    if !AMP_READY.swap(true, Ordering::AcqRel) {
        // Configure the enable as an output, LOW, before its first use.
        amp(false);
        sdk::gpio_no_pull(AMP_PIN);
        // SAFETY: the amplifier enable, which this module owns.
        unsafe { sdk::gpio_init(AMP_PIN, true) };
        amp(false);
    }
    if !sdk::audio_open(rate_hz) {
        return false;
    }
    amp(true);
    true
}

/// Amplifier off first, then the codec.
pub fn close_output() {
    amp(false);
    sdk::audio_close();
}

/// The device API's half: queue `tune` unless muted or busy.
pub fn request(tune: Tune) -> PlayResult {
    let Some(view) = settings::current() else {
        return PlayResult::Busy;
    };
    if !view.sound_at(local_ms()) {
        return PlayResult::Muted;
    }
    match SLOT.compare_exchange(EMPTY, tune.code() + 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => PlayResult::Started,
        Err(_) => PlayResult::Busy,
    }
}

/// Local wall time now, for the sound window; `None` while the clock is
/// invalid or has no offset.
fn local_ms() -> Option<i64> {
    clock::state()
        .reading
        .and_then(|reading| reading.local_ms())
}

/// Forget everything (host tests).
pub fn reset() {
    SLOT.store(EMPTY, Ordering::Release);
    AMP_READY.store(false, Ordering::Release);
}

named_enum! {
    /// What failed to start a tune (`error stage=`).
    pub enum ErrorStage {
        Open = "open",
    }
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
        stage: ErrorStage,
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

/// The playing stages own the tune and its player, so a stage that plays
/// always has them.
#[derive(Clone, Debug, Default)]
enum Stage {
    /// The startup chime waits for the settings to load.
    #[default]
    Startup,
    Idle,
    Warm {
        tune: Tune,
        player: Player,
        until: u32,
    },
    Play {
        tune: Tune,
        player: Player,
    },
    Drain {
        tune: Tune,
        since: u32,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Speaker {
    stage: Stage,
    started: u32,
    /// The DMA's underrun count when streaming began, then the tune's own.
    underruns: u32,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

fn amp(on: bool) {
    // SAFETY: the amplifier enable, which this module owns.
    unsafe { sdk::gpio_write_bit(AMP_PIN, on) };
}

impl Speaker {
    /// A speaker that skips the startup chime, for a firmware that steps it
    /// in its own loop for a while rather than in a task from boot.
    pub const fn quiet() -> Self {
        Self {
            stage: Stage::Idle,
            started: 0,
            underruns: 0,
        }
    }

    /// Nothing queued or playing: stepping can wait.
    pub fn is_idle(&self) -> bool {
        matches!(self.stage, Stage::Idle) && SLOT.load(Ordering::Acquire) == EMPTY
    }

    fn finish(&mut self, tune: Tune, drained: bool) -> Record {
        close_output();
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
        match &mut self.stage {
            Stage::Startup => {
                let Some(view) = settings::current() else {
                    return next(POLL_MS * 5, None);
                };
                self.stage = Stage::Idle;
                if !view.sound_at(local_ms()) {
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
                if !open_output(SAMPLE_RATE) {
                    SLOT.store(EMPTY, Ordering::Release);
                    return next(
                        POLL_MS,
                        Some(Record::Error {
                            stage: ErrorStage::Open,
                        }),
                    );
                }
                self.started = now;
                self.stage = Stage::Warm {
                    tune,
                    player: Player::new(tune, volume),
                    until: now.wrapping_add(WARM_MS),
                };
                next(POLL_MS, Some(Record::Play { tune, volume }))
            }

            Stage::Warm {
                tune,
                player,
                until,
            } => {
                if !deadline_passed(now, *until) {
                    return next(POLL_MS, None);
                }
                self.underruns = sdk::audio_underruns();
                self.stage = Stage::Play {
                    tune: *tune,
                    player: player.clone(),
                };
                self.step()
            }

            Stage::Play { tune, player } => {
                let tune = *tune;
                let bound = WARM_MS + tune.duration_ms() + LATE_MS;
                if now.wrapping_sub(self.started) >= bound {
                    self.underruns = sdk::audio_underruns().wrapping_sub(self.underruns);
                    return next(POLL_MS, Some(self.finish(tune, false)));
                }
                // The slot makes this the only filler; a handle out elsewhere
                // reads as a full ring.
                if let Some(mut audio) = sdk::Audio::take() {
                    while let Some(mut page) = audio.page() {
                        player.fill(&mut page[..]);
                        page.submit();
                        if player.done() {
                            break;
                        }
                    }
                }
                if player.done() {
                    // Zero pages the DMA played for want of a filled one.
                    self.underruns = sdk::audio_underruns().wrapping_sub(self.underruns);
                    self.stage = Stage::Drain { tune, since: now };
                }
                next(POLL_MS, None)
            }

            Stage::Drain { tune, since } => {
                let tune = *tune;
                let drained = sdk::audio_drained();
                if drained || now.wrapping_sub(*since) >= DRAIN_MS {
                    return next(POLL_MS, Some(self.finish(tune, drained)));
                }
                next(POLL_MS, None)
            }
        }
    }
}
