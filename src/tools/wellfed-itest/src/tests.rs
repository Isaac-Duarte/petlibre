//! No device I/O: the scenarios against a scripted feeder whose records
//! follow the firmware's formats, on a virtual clock.

use std::path::Path;
use std::time::Duration;

use serde_json::json;
use wellfed_model::{
    Counters, Counts, Generation, MealReminder, Memory, NtpServer, OpenRefusal, Refusal,
    ResetCause, Settings, Stats, StatsSection, TimeView, WifiSource, Window,
};

use super::log::Log;
use super::scenarios::ended_for;
use super::*;
use wellfed_core::feed::storage::OwnerOutcome;
use wellfed_rig::commands::ota::Variant;
use wellfed_rig::device::DeviceError;

const GOOD: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const FAILING: &str = "f00df00df00df00df00df00df00df00df00df00df00df00df00df00df00df00d";
const TWIN: &str = "7e157e157e157e157e157e157e157e157e157e157e157e157e157e157e157e15";
const OLD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const JAM: &str = "1a111a111a111a111a111a111a111a111a111a111a111a111a111a111a111a11";
const SENSOR: &str = "5e115e115e115e115e115e115e115e115e115e115e115e115e115e115e115e11";
const STALL: &str = "57a157a157a157a157a157a157a157a157a157a157a157a157a157a157a157a1";

enum Event {
    /// The feeder restarts, for `cause`.
    Restart(ResetCause),
    /// An activation reset into bank 1, logged by the install itself.
    Booted,
    Line(String),
    Running(String),
    /// The device API answers, or not (a boot, a lost hotspot).
    Online(bool),
    /// The booted image reports where its Wi-Fi credentials came from.
    Source(WifiSource),
    /// The Wi-Fi link drops and comes back on its own.
    LinkLoss,
    /// Bank 1 gave way: bank 0 takes the push that is still running.
    Recovery,
}

/// A feed under way: the motor drives from `start` until `end`.
#[derive(Debug, Clone, Copy)]
struct Feeding {
    id: u32,
    portions: u8,
    start: f64,
    end: f64,
}

/// About 9 s a portion, as on the bench.
const PORTION_SECONDS: f64 = 9.0;

/// A feeder that answers the way the hardware did in the update and settings drills.
struct Feeder {
    now: f64,
    pending: Vec<(f64, Event)>,
    uart: Vec<u8>,
    running: String,
    /// The build bank 1 last confirmed; reinstalling it skips the trial.
    confirmed: String,
    online: bool,
    losses: u32,
    boots: u32,
    /// Until the booted feeder has joined, the device API does not answer.
    ready_at: f64,
    /// When the clock was last synced this boot.
    synced: bool,
    lights: bool,
    sound: bool,
    volume: u8,
    lights_window: Window,
    sound_window: Window,
    desiccant_days: u8,
    meal_reminder: MealReminder,
    generation: u32,
    stored: bool,
    wifi_generation: u32,
    source: WifiSource,
    version: u32,
    /// The push running now, with its fault.
    offered: Option<(Staged, Fault)>,
    /// Bank 0 runs and answers (after a failed trial or a stopped install).
    bank0: bool,
    /// When a failing trial gives way; a later offer installs after it.
    fallback_at: Option<f64>,
    /// The last install's start and build, so a power cut can land in it.
    installing: Option<(f64, String)>,
    server: String,
    locked: bool,
    /// The second owner session is open.
    held: bool,
    feeding: Option<Feeding>,
    next_feed: u32,
    /// The interlock's `writes_deferred`.
    deferred: u32,
    actions: Vec<String>,
    notes: Vec<String>,
    /// Why the running boot started.
    reset: ResetCause,
    /// This boot's counts, and the feeder's over its life.
    counts: Counts,
    lifetime: Counts,
    heap: u32,
    /// The owner holds a fault until a restart.
    faulted: bool,
    /// A feed a reset interrupts, resolved as uncertain at the next boot.
    interrupted: Option<u32>,
    /// How far the clock is behind the host's, and the bound it states.
    clock_skew_ms: i64,
    clock_error_ms: u32,
    /// Relay 1 was cut for good.
    killed: bool,

    // Faults to inject.
    /// The counters miss one feed.
    miscounts: bool,
    /// Each memory reading has 6 KiB less free.
    leaks: bool,
    /// A feed a reset interrupted is handed to the owner again.
    reruns_interrupted: bool,
    silent: bool,
    admits_strangers: bool,
    settings_lost_on_boot: bool,
    /// A hotspot bounce changes the volume, which nothing in it checks.
    volume_drifts: bool,
    /// A settings change during a feed is saved at once.
    saves_during_feed: bool,
    /// A cancel is answered but the feed runs on.
    ignores_cancel: bool,
    /// A feed starts but its answer never arrives.
    loses_feed_answer: bool,
    /// A feed request goes unanswered and no feed starts.
    drops_feed: bool,
    /// The held session dies once a feed drives.
    held_dies_mid_feed: bool,
    /// A feed never reports its result.
    feed_never_ends: bool,
}

impl Feeder {
    fn new(running: &str) -> Self {
        Self {
            now: 0.0,
            pending: Vec::new(),
            uart: Vec::new(),
            running: running.into(),
            confirmed: running.into(),
            online: true,
            losses: 0,
            boots: 7,
            ready_at: 0.0,
            synced: false,
            lights: true,
            sound: true,
            volume: 40,
            lights_window: Window::Always,
            sound_window: Window::Always,
            desiccant_days: 14,
            meal_reminder: MealReminder::Off,
            generation: 2979,
            stored: false,
            wifi_generation: 4,
            source: WifiSource::Build,
            version: 19,
            offered: None,
            bank0: false,
            fallback_at: None,
            installing: None,
            server: "serve".into(),
            locked: false,
            held: false,
            feeding: None,
            next_feed: 12,
            deferred: 0,
            actions: Vec::new(),
            notes: Vec::new(),
            reset: ResetCause::PowerOn,
            counts: Counts::default(),
            lifetime: Counts::default(),
            heap: 140_000,
            faulted: false,
            interrupted: None,
            clock_skew_ms: 300,
            clock_error_ms: 1_000,
            killed: false,
            miscounts: false,
            leaks: false,
            reruns_interrupted: false,
            silent: false,
            admits_strangers: false,
            volume_drifts: false,
            settings_lost_on_boot: false,
            saves_during_feed: false,
            ignores_cancel: false,
            loses_feed_answer: false,
            drops_feed: false,
            held_dies_mid_feed: false,
            feed_never_ends: false,
        }
    }

    fn at(&mut self, delay: f64, event: Event) {
        if !self.silent {
            self.pending.push((self.now + delay, event));
        }
    }

    fn line(&mut self, delay: f64, text: &str) {
        self.at(delay, Event::Line(text.into()));
    }

    fn deliver(&mut self) {
        let now = self.now;
        let (due, later): (Vec<_>, Vec<_>) = self.pending.drain(..).partition(|(at, _)| *at <= now);
        self.pending = later;
        let mut due = due;
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, event) in due {
            match event {
                Event::Restart(cause) => self.boot_as(cause),
                Event::Booted => {
                    self.boots += 1;
                    self.counts = Counts::default();
                    self.reset = ResetCause::Software;
                    self.faulted = false;
                }
                Event::Line(text) => {
                    self.counted(&text);
                    self.uart.extend_from_slice(b"[sdk] ");
                    self.uart.extend_from_slice(text.as_bytes());
                    self.uart.extend_from_slice(b"\r\n");
                }
                Event::Running(build) => {
                    self.confirmed = build.clone();
                    self.running = build;
                }
                Event::Source(source) => self.source = source,
                Event::Online(online) => self.online = online,
                Event::LinkLoss => self.losses += 1,
                Event::Recovery => {
                    if let Some((offer, fault)) = self.offered.clone() {
                        self.push(self.now, &offer, fault);
                    }
                }
            }
        }
    }

    /// A result counts once it is logged, as the feeder counts its `Fed`
    /// event: one a reset drops never counts.
    fn counted(&mut self, text: &str) {
        use wellfed_core::record::{Event as Record, Owner};
        for event in super::log::events(text) {
            if let Record::Owner(Owner::Fed {
                outcome, completed, ..
            }) = event
            {
                let failed = u32::from(outcome != Some(OwnerOutcome::Done));
                for counts in [&mut self.counts, &mut self.lifetime] {
                    counts.feeds += 1;
                    counts.feeds_failed += failed;
                    counts.portions += u32::from(completed);
                }
            }
        }
    }

    /// Bank 0 takes the push at `start`: a fault stops it before the marker
    /// and bank 0 keeps answering; otherwise it installs.
    fn push(&mut self, start: f64, offer: &Staged, fault: Fault) {
        match fault.stage() {
            Some(stage) => {
                self.bank0 = true;
                self.line(
                    start - self.now + 90.0,
                    &format!("PLOTA1 error stage={stage}"),
                );
            }
            None => self.install(start, offer.version, &offer.build.clone()),
        }
    }

    /// Download from `start`, activate, trial-boot `build` on bank 1, then
    /// confirm (or, for the failing build, give way three boots later).
    fn install(&mut self, start: f64, version: u32, build: &str) {
        // Activated into bank 1; a failing build gives way back later.
        self.bank0 = false;
        let delay = start - self.now;
        self.installing = Some((start, build.to_string()));
        self.line(
            delay + 20.0,
            &format!("PLOTA1 manifest version={version} bytes=663552"),
        );
        self.line(delay + 30.0, "PLOTA1 progress sectors=16/162 leases=260");
        self.line(delay + 50.0, "PLOTA1 progress sectors=81/162 leases=1300");
        self.line(
            delay + 100.0,
            &format!("PLOTA1 activated version={version} sectors=162"),
        );
        if build == self.confirmed {
            // As on the bench: KM0 takes a build it confirmed before at once.
            self.line(delay + 104.0, "PLTRL1 confirmed=1");
            self.line(
                delay + 105.0,
                &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
            );
            self.line(delay + 106.0, "PLCNF1 confirmed=1 already=1");
            self.at(delay + 105.0, Event::Booted);
            self.at(delay + 106.0, Event::Running(build.into()));
            return;
        }
        self.line(
            delay + 105.0,
            &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
        );
        self.at(delay + 105.0, Event::Booted);
        // The build runs from the moment it logs its confirmation, as on the
        // bench, where a firmware without the device API has only that line.
        self.line(delay + 106.0, "PLTRL1 confirmed=0 boot=1 limit=3");
        if build == FAILING {
            self.line(delay + 140.0, "PLTRL1 confirmed=0 boot=3 limit=3");
            self.line(delay + 170.0, "PLTRL1 give_way=1 marker=00 reset=1");
            self.line(
                delay + 175.0,
                &format!("PLFW1 start build={OLD} bank=0 reset=0x0 mmu=0"),
            );
            self.fallback_at = Some(self.now + delay + 175.0);
        } else {
            self.line(delay + 140.0, "PLCNF1 confirmed=1 healthy_ms=30000");
            self.at(delay + 140.0, Event::Running(build.into()));
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            generation: Generation(self.generation),
            lights: self.lights,
            sound: self.sound,
            volume: self.volume,
            lights_window: self.lights_window,
            sound_window: self.sound_window,
            desiccant_days: self.desiccant_days,
            desiccant_replaced: None,
            meal_reminder: self.meal_reminder,
            writable: true,
        }
    }

    fn device_status(&self) -> DeviceStatus {
        DeviceStatus {
            build: self.running.clone(),
            bank: 1,
            uptime_ms: 61_000,
            boot_count: self.boots,
            wifi: self.source,
            losses: self.losses,
            // As the firmware keeps it: 300 ms behind the host after a sync.
            clock_ms: self.clock_ms(),
            settings_generation: self.generation,
            wifi_generation: self.wifi_generation,
            locked: Some(self.locked),
        }
    }

    /// The feed whose outputs are claimed now.
    fn feeding(&self) -> Option<Feeding> {
        self.feeding.filter(|feed| self.now < feed.end)
    }

    fn clock_ms(&self) -> Option<u64> {
        self.synced
            .then(|| u64::try_from(self.unix_ms() - self.clock_skew_ms).unwrap())
    }

    /// The owner's records for feed `id`, from its reservation to Idle, as
    /// the running build's injected faults have it end.
    fn feed(&mut self, portions: u8) -> u32 {
        let id = self.next_feed;
        self.next_feed += 1;
        let drive = PORTION_SECONDS * f64::from(portions);
        self.line(
            0.1,
            &format!("PLOWN1 feed id={id} portions={portions} submit=0"),
        );
        self.line(
            0.2,
            "PLOWN1 state=3 fault=0 supervisor=0 stopped=1 optics_off=0",
        );
        let driving = "PLOWN1 state=4 fault=0 supervisor=0 stopped=0 optics_off=0";
        // (seconds to the result, completed, outcome, reason) for a fault
        // the build injects.
        let injected = match (self.running.as_str(), portions) {
            (JAM, 1) => Some((11.5, 1, 1, 1)),
            (JAM, 2) => Some((7.2, 0, 4, 13)),
            (JAM, _) => Some((6.1, 0, 4, 14)),
            (SENSOR, 1) => Some((11.0, 0, 4, 6)),
            (SENSOR, 2) => {
                // The outlet reads blocked: failed before any drive.
                self.finish(1.0, id, portions, 0, 4, 9);
                self.feeding = Some(Feeding {
                    id,
                    portions,
                    start: self.now + 1.0,
                    end: self.now + 1.02,
                });
                return id;
            }
            (SENSOR, _) => {
                // The ADC lost: failed, and the owner holds the fault. As
                // the feeder logs it, the fault state comes before the
                // result.
                self.line(2.0, driving);
                self.line(
                    4.0,
                    "PLOWN1 state=7 fault=7 supervisor=3 stopped=1 optics_off=1",
                );
                self.line(
                    4.01,
                    &format!(
                        "PLOWN1 fed id={id} requested={portions} completed=0 outcome=4 reason=8 \
                         durable=1 drive_ms=2000 rising=0 falling=0"
                    ),
                );
                self.line(
                    4.015,
                    &format!(
                        "PLOWN1 current id={id} first_edge_ms=none current_max=168 \
                         current_mean=59 samples=60 reversals=0 hopper=present"
                    ),
                );
                self.faulted = true;
                self.feeding = Some(Feeding {
                    id,
                    portions,
                    start: self.now + 2.0,
                    end: self.now + 4.02,
                });
                return id;
            }
            (STALL, 1) => {
                // The motor lease stops the drive, then the faulted owner
                // resets the board.
                self.line(2.0, driving);
                self.finish(2.7, id, portions, 0, 4, 12);
                self.at(6.0, Event::Restart(ResetCause::Watchdog));
                self.feeding = Some(Feeding {
                    id,
                    portions,
                    start: self.now + 2.0,
                    end: self.now + 2.72,
                });
                return id;
            }
            (STALL, _) => {
                // The lapsed beat resets the board mid-drive.
                self.line(2.0, driving);
                self.interrupted = Some(id);
                self.at(10.0, Event::Restart(ResetCause::Watchdog));
                self.feeding = Some(Feeding {
                    id,
                    portions,
                    start: self.now + 2.0,
                    end: self.now + 10.0,
                });
                return id;
            }
            _ => None,
        };
        self.line(2.0, driving);
        let end = match injected {
            _ if self.feed_never_ends => f64::INFINITY,
            Some((after, completed, outcome, reason)) => {
                self.finish(after, id, portions, completed, outcome, reason);
                self.now + after + 0.02
            }
            None => {
                self.finish(2.0 + drive, id, portions, portions, 1, 1);
                self.now + 2.0 + drive + 0.02
            }
        };
        self.feeding = Some(Feeding {
            id,
            portions,
            start: self.now + 2.0,
            end,
        });
        id
    }

    fn finish(
        &mut self,
        delay: f64,
        id: u32,
        requested: u8,
        completed: u8,
        outcome: u8,
        reason: u8,
    ) {
        self.line(
            delay,
            "PLOWN1 state=6 fault=0 supervisor=0 stopped=1 optics_off=1",
        );
        self.line(
            delay + 0.01,
            &format!(
                "PLOWN1 fed id={id} requested={requested} completed={completed} outcome={outcome} \
                 reason={reason} durable=1 drive_ms=9000 rising={completed} falling={completed}"
            ),
        );
        self.line(
            delay + 0.015,
            &format!(
                "PLOWN1 current id={id} first_edge_ms=7840 current_max=168 current_mean=59 \
                 samples=300 reversals=0 hopper=present"
            ),
        );
        self.line(
            delay + 0.02,
            "PLOWN1 state=1 fault=0 supervisor=0 stopped=1 optics_off=1",
        );
    }

    /// Stop the running feed now, with the portions its marks counted.
    fn cancel(&mut self, feed: Feeding) {
        let now = self.now;
        self.pending.retain(|(at, event)| {
            !(*at >= now && matches!(event, Event::Line(text) if text.starts_with("PLOWN1")))
        });
        let done = ((now - feed.start).max(0.0) / PORTION_SECONDS) as u8;
        self.finish(0.2, feed.id, feed.portions, done, 2, 2);
        self.feeding = Some(Feeding {
            end: now + 0.22,
            ..feed
        });
    }

    /// The device API's answer on either session.
    fn answer(&mut self, ask: &Ask) -> Result<Answer> {
        if self.silent || !self.online || self.now < self.ready_at {
            return Err(DeviceError::Closed.into());
        }
        self.logged(ask.name());
        if matches!(ask, Ask::Feed { .. }) && self.drops_feed {
            return Err(DeviceError::Closed.into());
        }
        if let Ask::Feed { portions } = ask
            && self.loses_feed_answer
            && self.feeding().is_none()
        {
            self.feed(*portions);
            return Err(DeviceError::Closed.into());
        }
        // An event recorded during a feed waits for it, and the interlock
        // logs the writer's wait.
        if matches!(ask, Ask::SetLock { .. } | Ask::SyncClock) && self.feeding().is_some() {
            self.deferred += 1;
            let deferred = self.deferred;
            self.line(
                0.3,
                &format!(
                    "PLOWN1 interlock writes_deferred={deferred} writes_forced=0 \
                     write_waited_max_ms=0 feeds_delayed=0"
                ),
            );
        }
        Ok(match ask {
            Ask::Status => Answer::Status(self.device_status()),
            Ask::GetSettings => Answer::Settings(self.settings()),
            Ask::PutSettings { base, .. } if *base != self.generation => {
                Answer::Refused(Refusal::Stale {
                    current: Generation(self.generation),
                })
            }
            Ask::PutWifi { base, .. } if *base != self.wifi_generation => {
                Answer::Refused(Refusal::Stale {
                    current: Generation(self.wifi_generation),
                })
            }
            Ask::PutSettings { .. } if self.feeding().is_some() && !self.saves_during_feed => {
                Answer::Refused(Refusal::Busy)
            }
            Ask::PutSettings { changes, .. } => {
                if let Some(on) = changes.lights {
                    self.lights = on;
                }
                if let Some(on) = changes.sound {
                    self.sound = on;
                }
                if let Some(volume) = changes.volume {
                    self.volume = volume;
                }
                if let Some(window) = changes.lights_window {
                    self.lights_window = window;
                }
                if let Some(window) = changes.sound_window {
                    self.sound_window = window;
                }
                if let Some(days) = changes.desiccant_days {
                    self.desiccant_days = days;
                }
                if let Some(reminder) = changes.meal_reminder {
                    self.meal_reminder = reminder;
                }
                self.generation += 1;
                let saved = format!(
                    "PLCFG1 saved generation={} lights={} sound={} volume={}",
                    self.generation,
                    u8::from(self.lights),
                    u8::from(self.sound),
                    self.volume
                );
                self.line(1.0, &saved);
                Answer::Done {
                    generation: Some(self.generation),
                }
            }
            Ask::PutWifi { wifi, .. } => {
                self.stored = wifi.is_some();
                self.wifi_generation += 1;
                let saved = format!(
                    "PLCFG1 net saved generation={} stored={}",
                    self.wifi_generation,
                    u8::from(self.stored)
                );
                self.line(1.0, &saved);
                Answer::Done {
                    generation: Some(self.wifi_generation),
                }
            }
            Ask::SyncClock => {
                self.synced = true;
                Answer::Done { generation: None }
            }
            Ask::SetLock { locked } => {
                self.locked = *locked;
                Answer::Done { generation: None }
            }
            Ask::Feed { .. } if self.feeding().is_some() => Answer::Refused(Refusal::Busy),
            Ask::Feed { portions } if self.faulted => {
                // As the firmware logs it: the next id, refused, not consumed.
                let id = self.next_feed;
                self.actions.push(format!("refused feed {id}"));
                self.line(
                    0.1,
                    &format!("PLOWN1 feed id={id} portions={portions} submit=5"),
                );
                Answer::Refused(Refusal::Fault)
            }
            Ask::Reboot if self.feeding().is_some() => Answer::Refused(Refusal::Busy),
            Ask::Reboot => {
                // The firmware restarts by letting its watchdog lapse.
                self.at(1.0, Event::Restart(ResetCause::Watchdog));
                Answer::Done { generation: None }
            }
            Ask::GetTime => Answer::Time(Box::new(self.time())),
            Ask::Stats {
                section: StatsSection::Counters,
            } => {
                let mut boot = self.counts;
                if self.miscounts {
                    boot.feeds = boot.feeds.saturating_sub(1);
                }
                Answer::Stats(Box::new(Stats::Counters(Counters {
                    boot,
                    reset: self.reset,
                    lifetime: Some(self.lifetime),
                    ..Counters::default()
                })))
            }
            Ask::Stats {
                section: StatsSection::Memory,
            } => {
                if self.leaks {
                    self.heap -= 6 * 1024;
                }
                Answer::Stats(Box::new(Stats::Memory(Memory {
                    heap_free: self.heap,
                    heap_min: self.heap - 10_000,
                    tasks: Vec::new(),
                    tasks_total: 3,
                })))
            }
            Ask::Stats { .. } => Answer::Unexpected,
            Ask::Feed { portions } => Answer::Feeding {
                feed: self.feed(*portions),
            },
            Ask::CancelFeed { feed } => match self.feeding() {
                Some(running) if running.id == *feed => {
                    if !self.ignores_cancel {
                        self.cancel(running);
                    }
                    Answer::Done { generation: None }
                }
                _ => Answer::Refused(Refusal::NotFound),
            },
            Ask::UpdateState | Ask::CancelUpdate | Ask::Clients => Answer::Unexpected,
        })
    }

    /// A request record, as the firmware logs one after answering.
    fn logged(&mut self, op: &str) {
        self.line(0.5, &format!("PLAPI1 request slot=0 op={op} result=ok"));
    }

    fn time(&self) -> TimeView {
        TimeView {
            generation: Generation(3),
            writable: true,
            zone: None,
            ntp: NtpServer::Off,
            clock_ms: self.clock_ms(),
            error_ms: self.synced.then_some(self.clock_error_ms),
            offset_s: None,
            rate_ppb: 0,
            rate_bound_ppb: 5_000_000,
            learned: false,
            syncs: 1,
            last: None,
            last_age_s: None,
            ntp_last: None,
        }
    }

    fn boot(&mut self) {
        self.boot_as(ResetCause::PowerOn);
    }

    /// A restart for `cause`: only a power cut loses the clock, and a feed
    /// it interrupted is resolved as uncertain once the owner opens.
    fn boot_as(&mut self, cause: ResetCause) {
        if self.interrupted.is_some() {
            // The interrupted feed's own records die with it.
            self.pending.retain(
                |(_, event)| !matches!(event, Event::Line(text) if text.starts_with("PLOWN1")),
            );
        }
        self.feeding = None;
        self.faulted = false;
        self.reset = cause;
        self.counts = Counts::default();
        self.line(
            3.0,
            &format!("PLFW1 start build={} bank=1 reset=0x0 mmu=1", self.running),
        );
        if let Some(id) = self.interrupted.take() {
            self.line(
                6.0,
                &format!(
                    "PLOWN1 fed id={id} requested=3 completed=0 outcome=5 reason=0 durable=1 \
                     drive_ms=0 rising=0 falling=0"
                ),
            );
            self.line(
                6.005,
                &format!(
                    "PLOWN1 current id={id} first_edge_ms=none current_max=0 \
                     current_mean=none samples=0 reversals=0 hopper=unread"
                ),
            );
            self.line(
                6.01,
                "PLOWN1 state=1 fault=0 supervisor=0 stopped=1 optics_off=1",
            );
            if self.reruns_interrupted {
                self.line(6.5, &format!("PLOWN1 feed id={id} portions=3 submit=0"));
            }
        }
        if self.settings_lost_on_boot {
            self.lights = true;
            self.sound = true;
            self.volume = 40;
        }
        self.boots += 1;
        if cause == ResetCause::PowerOn {
            self.synced = false;
        }

        // Stored credentials that never join cost five attempts first.
        let ready = if self.stored { 200.0 } else { 20.0 };
        self.ready_at = self.now + ready;
        let source = if self.stored {
            WifiSource::Fallback
        } else {
            WifiSource::Build
        };
        self.at(ready, Event::Source(source));
    }
}

impl Rig for Feeder {
    fn stage(&mut self, args: &OtaArgs) -> Result<Option<Staged>> {
        self.version += 1;
        let review = args
            .review
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("private/verification/{}-review", args.name)));
        // A re-signed review keeps its build.
        let failing = args.variant == Some(Variant::Fail)
            || review.to_string_lossy().ends_with("-fail-review");
        let twin = args.variant == Some(Variant::Twin) || review.to_string_lossy().contains("twin");
        let named = |variant: Variant, label: &str| {
            args.variant == Some(variant)
                || review
                    .to_string_lossy()
                    .ends_with(&format!("-{label}-review"))
        };
        let build = match (failing, twin) {
            (true, _) => FAILING,
            (_, true) => TWIN,
            _ if named(Variant::FeedJam, "jam") => JAM,
            _ if named(Variant::SensorFault, "sensor") => SENSOR,
            _ if named(Variant::FeedStall, "stall") => STALL,
            _ => GOOD,
        };
        self.actions
            .push(format!("stage {} v{}", args.name, self.version));
        Ok(Some(Staged {
            build: build.into(),
            target: ota::Target::Replacement,
            version: self.version,
            review: review.clone(),
            manifest: format!(
                "private/verification/{}-v{}.manifest",
                args.name, self.version
            ),
            image: review.join("image.bin").display().to_string(),
        }))
    }

    fn review_build(&mut self, _: &Path) -> Result<String> {
        Ok(GOOD.into())
    }

    fn server(&mut self, server: Server) -> Result<()> {
        self.actions
            .push(format!("server {server:?}").chars().take(40).collect());
        self.server = "serve".into();
        Ok(())
    }

    fn intrude(&mut self, intruder: Intruder) -> Result<Rebuffed> {
        self.actions.push(format!("intrude {intruder:?}"));
        Ok(match intruder {
            Intruder::UnenrolledKey if self.admits_strangers => Rebuffed::Opened,
            Intruder::UnenrolledKey => {
                self.line(0.5, "PLAPI1 session slot=1 state=ended reason=unknown-key");
                Rebuffed::Refused(OpenRefusal::Unknown)
            }
            Intruder::Tampered => {
                self.line(0.5, "PLAPI1 session slot=1 state=ended reason=noise");
                Rebuffed::Dropped
            }
        })
    }

    fn offer(&mut self, staged: Option<&Staged>, fault: Fault) -> Result<()> {
        self.actions.push(match (staged, fault) {
            (Some(s), Fault::None) => format!("offer v{}", s.version),
            (Some(s), fault) => format!("offer v{} {fault:?}", s.version),
            (None, _) => "offer clear".into(),
        });
        self.offered = staged.cloned().map(|staged| (staged, fault));
        let Some(staged) = staged else { return Ok(()) };
        if let Some(fallback) = self.fallback_at.take() {
            // Bank 0 answers once the failing trial has given way to it.
            self.push(fallback, &staged.clone(), fault);
        } else if self.bank0 {
            self.push(self.now + 10.0, &staged.clone(), fault);
        } else if staged.build == self.running {
            // Refused as the running build; the suite never offers it.
            self.line(1.0, "PLOTA1 refused reason=same");
        } else {
            self.line(
                30.0,
                &format!("PLWCH1 give_way=1 version={} restart=1", staged.version),
            );
            self.at(38.0, Event::Recovery);
        }
        Ok(())
    }

    fn offered(&mut self) -> Option<std::result::Result<(), Arc<PushError>>> {
        None
    }

    fn device(&mut self, ask: &Ask) -> Result<Answer> {
        self.deliver();
        self.actions.push(format!("ask {}", ask.name()));
        // Only the held session writes on purpose while a feed runs.
        if self.feeding().is_some() && matches!(ask, Ask::PutSettings { .. } | Ask::SetLock { .. })
        {
            self.actions.push("write during feed".into());
        }
        self.answer(ask)
    }

    fn hold(&mut self) -> Result<()> {
        self.deliver();
        self.actions.push("hold".into());
        // No handshake while a feed holds the outputs.
        if self.feeding().is_some() {
            return Err(DeviceError::Timeout(Duration::from_secs(10)).into());
        }
        self.held = true;
        Ok(())
    }

    fn held(&mut self, ask: &Ask) -> Result<Answer> {
        self.deliver();
        self.actions.push(format!("held {}", ask.name()));
        if self.held_dies_mid_feed && self.feeding().is_some() {
            self.held = false;
        }
        if !self.held {
            return Err(DeviceError::NotHeld.into());
        }
        self.answer(ask)
    }

    fn release(&mut self) {
        self.actions.push("release".into());
        self.held = false;
    }

    fn unix_ms(&self) -> i64 {
        // 2026-10-22T12:00:00Z plus the virtual clock.
        1_792_670_400_000 + (self.now * 1000.0) as i64
    }

    /// The server hears nothing from the replacement firmware.
    fn status(&mut self) -> Result<StockStatus> {
        self.deliver();
        Ok(StockStatus::default())
    }

    fn hotspot(&mut self, hotspot: Hotspot) -> Result<()> {
        self.actions.push(format!("hotspot {}", hotspot.verb()));
        if hotspot == Hotspot::Up {
            if self.volume_drifts {
                self.volume = 50;
            }
            self.at(16.0, Event::Online(true));
        } else {
            self.at(40.0, Event::LinkLoss);
            self.at(40.0, Event::Online(false));
        }
        Ok(())
    }

    fn power_cycle(&mut self) -> Result<()> {
        self.actions.push("power cycle".into());
        if let Some(feed) = self.feeding() {
            self.interrupted = Some(feed.id);
            self.boot();
            return Ok(());
        }
        let since = self
            .installing
            .clone()
            .map(|(start, build)| (self.now - start, build));
        match since {
            // Mid-download, bank 1's marker is still clear: bank 0 boots and
            // downloads whatever is offered from the start.
            Some((at, _)) if (20.0..100.0).contains(&at) => {
                self.pending.clear();
                self.line(
                    3.0,
                    &format!("PLFW1 start build={OLD} bank=0 reset=0x0 mmu=0"),
                );
                // The push reconnects to bank 0 and offers again.
                if let Some((offer, fault)) = self.offered.clone() {
                    self.push(self.now + 10.0, &offer, fault);
                }
            }
            // Early in a good build's trial: the count restarts at boot 1.
            Some((at, build))
                if (105.0..140.0).contains(&at) && build != FAILING && build != self.confirmed =>
            {
                self.pending.clear();
                self.installing = None;
                self.line(
                    3.0,
                    &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
                );
                self.line(4.0, "PLTRL1 confirmed=0 boot=1 limit=3");
                self.line(39.0, "PLCNF1 confirmed=1 healthy_ms=30000");
                self.at(39.0, Event::Running(build));
            }
            _ => self.boot(),
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        self.actions.push("kill".into());
        self.killed = true;
        self.online = false;
        self.pending.clear();
        Ok(())
    }

    fn uart(&mut self) -> Result<Vec<u8>> {
        self.deliver();
        Ok(std::mem::take(&mut self.uart))
    }

    fn wait(&mut self, span: Duration) -> Result<()> {
        self.now += span.as_secs_f64();
        Ok(())
    }

    fn clock(&self) -> Duration {
        Duration::from_secs_f64(self.now)
    }

    fn note(&mut self, text: &str) {
        self.notes.push(text.into());
    }
}

fn args(extra: &[&str]) -> ItestArgs {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ItestArgs,
    }
    let argv = ["itest", "run1"].iter().chain(extra);
    Cli::try_parse_from(argv).unwrap().args
}

fn suite(feeder: &mut Feeder, flags: &[&str]) -> (Vec<Report>, Option<Silent>) {
    let args = args(flags);
    let scenarios = select(&args).unwrap();
    let mut ctx = Ctx::new(feeder, &args);
    run(&mut ctx, &scenarios).unwrap()
}

fn verdicts(reports: &[Report]) -> Vec<(&str, &str)> {
    reports
        .iter()
        .map(|r| {
            let v = match r.verdict {
                Verdict::Pass => "pass",
                Verdict::Fail(_) => "fail",
                Verdict::Skip(_) => "skip",
            };
            (r.scenario, v)
        })
        .collect()
}

#[test]
fn the_full_suite_passes_on_a_well_behaved_feeder() {
    let mut feeder = Feeder::new(OLD);
    let flags = [
        "--build",
        "--power",
        "--destructive",
        "--feeds",
        "--tier",
        "soak",
        "--soak-minutes",
        "5",
        // Every fault of the sustained run once.
        "--sustained-cycles",
        "8",
    ];
    let (reports, silent) = suite(&mut feeder, &flags);

    // The round trip to stock needs --stock (it has its own scripted feeder).
    let failures: Vec<_> = reports
        .iter()
        .filter_map(|r| match &r.verdict {
            Verdict::Pass => None,
            Verdict::Skip(_) if r.scenario == "stock-roundtrip" => None,
            other => Some(format!("{}: {other:?}", r.scenario)),
        })
        .collect();
    assert!(failures.is_empty(), "{failures:#?}\n{:#?}", feeder.notes);
    assert_eq!(reports.len(), SCENARIOS.len());
    assert!(silent.is_none());
    assert_eq!(exit_code(&reports, None), 0);

    // Everything changed was put back.
    assert_eq!(feeder.running, GOOD);
    assert_eq!(
        (feeder.lights, feeder.sound, feeder.volume),
        (true, true, 40)
    );
    assert!(!feeder.stored);
    assert!(!feeder.locked && !feeder.held);
    assert_eq!(feeder.server, "serve");
    assert!(feeder.offered.is_none());
    assert!(!feeder.killed);
    // The baseline's clock was set, and only the scripted strangers knocked.
    assert!(feeder.actions.iter().any(|a| a == "ask time-sync"));
    assert_eq!(
        feeder
            .actions
            .iter()
            .filter(|a| a.starts_with("intrude"))
            .count(),
        2
    );
}

#[test]
fn without_power_or_build_those_scenarios_are_skipped_not_passed() {
    let mut feeder = Feeder::new(OLD);
    let (reports, _) = suite(&mut feeder, &["--review", "private/verification/x-review"]);

    assert_eq!(
        verdicts(&reports),
        [
            ("ota-update", "pass"),
            ("requests", "pass"),
            ("request-auth", "pass"),
            ("settings-persist", "skip"),
            ("wifi-fallback", "skip"),
            ("network-bounce", "pass"),
            ("feed-writes", "skip"),
            ("ota-recovery", "skip"),
            ("ota-power-cuts", "skip"),
            ("stock-roundtrip", "skip"),
        ]
    );
    assert!(!feeder.actions.iter().any(|a| a == "power cycle"));
    assert!(!feeder.actions.iter().any(|a| a.ends_with(" feed")));
}

#[test]
fn tiers_take_on_more_of_the_suite() {
    let names = |tier: &str| -> Vec<&str> {
        select(&args(&["--tier", tier]))
            .unwrap()
            .iter()
            .map(|s| s.name)
            .collect()
    };

    assert_eq!(names("smoke"), ["ota-update", "requests", "request-auth"]);
    assert_eq!(names("standard").len(), SCENARIOS.len() - 2);
    assert!(!names("standard").contains(&"soak"));
    assert!(!names("standard").contains(&"sustained"));
    assert_eq!(names("soak").len(), SCENARIOS.len());
}

/// The sustained run alone, on the baseline already running.
fn sustained(feeder: &mut Feeder, cycles: &str) -> Report {
    let (reports, _) = suite(
        feeder,
        &[
            "--power",
            "--build",
            "--destructive",
            "--feeds",
            "--only",
            "sustained",
            "--sustained-cycles",
            cycles,
        ],
    );
    assert_eq!(reports[0].verdict, Verdict::Pass, "{:#?}", feeder.notes);
    reports[1].clone()
}

/// The run's halt reason, after checking relay 1 was cut.
fn halted(feeder: &Feeder, report: &Report) -> String {
    let Verdict::Fail(reason) = &report.verdict else {
        panic!("{report:?}\n{:#?}", feeder.notes);
    };
    assert!(feeder.killed, "relay 1 was not cut: {reason}");
    reason.clone()
}

#[test]
fn the_sustained_rotation_takes_every_fault_twice_in_sixteen_cycles() {
    use super::sustained::Step;
    assert_eq!(args(&[]).sustained_cycles, 16);
    for step in Step::ROTATION {
        let times = Step::ROTATION
            .iter()
            .cycle()
            .take(16)
            .filter(|s| **s == step)
            .count();
        assert_eq!(times, 2, "{step:?}");
    }
}

#[test]
fn a_sustained_run_feeds_through_every_fault_and_ends_on_the_baseline() {
    let mut feeder = Feeder::new(GOOD);
    let report = sustained(&mut feeder, "8");

    assert_eq!(report.verdict, Verdict::Pass, "{:#?}", feeder.notes);
    assert_eq!(feeder.running, GOOD);
    assert!(!feeder.killed);
    // Each variant went on and came off, and every feed asked at most 3.
    for variant in ["jam", "sensor", "stall"] {
        assert!(
            feeder
                .actions
                .iter()
                .any(|a| a.starts_with("stage run1-s0") && a.contains(variant)),
            "{variant}"
        );
    }
    // Both cuts landed in a feed, and every cycle passed its checks.
    let noted = |text: &str| feeder.notes.iter().filter(|n| n.contains(text)).count();
    assert_eq!(noted("Cutting the supply with feed"), 2);
    assert_eq!(noted("reserved, not yet driving"), 1);
    assert_eq!(noted("passed in"), 8, "{:#?}", feeder.notes);
    assert!(feeder.actions.iter().any(|a| a == "ask reboot"));
    // The faulted owner logged a refused submission under the id the next
    // feed took, and the journal check still passed.
    assert!(feeder.actions.iter().any(|a| a.starts_with("refused feed")));
    assert!(feeder.actions.iter().any(|a| a == "hotspot down"));
    // Half-hour cycles: eight of them take at least four hours.
    assert!(report.took >= Duration::from_secs(8 * 30 * 60));
}

#[test]
fn an_unplanned_restart_at_rest_halts_the_sustained_run_with_relay_1_cut() {
    let mut feeder = Feeder::new(GOOD);
    // Well into the first cycle's rest between feeds.
    feeder.at(900.0, Event::Restart(ResetCause::Watchdog));
    let report = sustained(&mut feeder, "2");

    let reason = halted(&feeder, &report);
    assert!(reason.contains("rest"), "{reason}");
}

#[test]
fn counters_that_miss_a_feed_halt_the_sustained_run() {
    let mut feeder = Feeder::new(GOOD);
    feeder.miscounts = true;
    let report = sustained(&mut feeder, "1");

    let reason = halted(&feeder, &report);
    assert!(reason.contains("counters read"), "{reason}");
}

#[test]
fn a_heap_that_keeps_falling_halts_the_sustained_run() {
    let mut feeder = Feeder::new(GOOD);
    feeder.leaks = true;
    let report = sustained(&mut feeder, "2");

    let reason = halted(&feeder, &report);
    assert!(reason.contains("Free heap fell"), "{reason}");
}

#[test]
fn a_clock_outside_its_stated_bound_halts_the_sustained_run() {
    let mut feeder = Feeder::new(GOOD);
    feeder.clock_skew_ms = 5_000;
    let report = sustained(&mut feeder, "1");

    let reason = halted(&feeder, &report);
    assert!(reason.contains("outside its 1000 ms bound"), "{reason}");
}

#[test]
fn a_scenario_that_leaves_the_feeder_changed_fails_and_is_put_right() {
    let mut feeder = Feeder::new(GOOD);
    feeder.volume_drifts = true;
    let (reports, _) = suite(
        &mut feeder,
        &[
            "--review",
            "private/verification/x-review",
            "--only",
            "network-bounce",
        ],
    );

    let Verdict::Fail(reason) = &reports[1].verdict else {
        panic!("{:?}", reports[1]);
    };
    assert!(
        reason.starts_with("left the feeder changed: settings"),
        "{reason}"
    );
    assert_eq!(feeder.volume, 40);
}

#[test]
fn a_refused_submission_is_no_feed() {
    let taken = |line: &str| super::log::events(line).find_map(|e| super::feeds::submitted(&e));

    assert_eq!(taken("PLOWN1 feed id=60 portions=1 submit=5"), None);
    assert_eq!(taken("PLOWN1 feed id=60 portions=1 submit=0"), Some(60));
}

#[test]
fn a_feed_handed_to_the_owner_again_after_a_cut_halts_the_sustained_run() {
    let mut feeder = Feeder::new(GOOD);
    feeder.reruns_interrupted = true;
    // Cycle 3 cuts the supply with a feed reserved.
    let report = sustained(&mut feeder, "3");

    let reason = halted(&feeder, &report);
    assert!(reason.contains("2 owner records and 1 results"), "{reason}");
}

#[test]
fn a_soak_fails_on_a_link_loss() {
    let mut feeder = Feeder::new(GOOD);
    feeder.at(200.0, Event::LinkLoss);
    let (reports, _) = suite(
        &mut feeder,
        &[
            "--review",
            "private/verification/x-review",
            "--only",
            "soak",
            "--soak-minutes",
            "10",
        ],
    );

    assert_eq!(
        verdicts(&reports),
        [("ota-update", "pass"), ("soak", "fail")]
    );
    let Verdict::Fail(reason) = &reports[1].verdict else {
        unreachable!()
    };
    assert!(reason.contains("link was lost"), "{reason}");
}

#[test]
fn a_feeder_just_powered_on_is_heard_before_offering_it_its_own_build() {
    let mut feeder = Feeder::new(GOOD);
    feeder.online = false;
    feeder.at(25.0, Event::Online(true));
    let (reports, _) = suite(&mut feeder, &["--build", "--only", "requests"]);

    assert_eq!(
        verdicts(&reports),
        [("ota-update", "pass"), ("requests", "pass")]
    );
    assert!(!feeder.actions.iter().any(|a| a.starts_with("offer")));
}

#[test]
fn a_running_review_is_not_reinstalled() {
    let mut feeder = Feeder::new(GOOD);
    let (reports, _) = suite(
        &mut feeder,
        &[
            "--review",
            "private/verification/x-review",
            "--only",
            "requests",
        ],
    );

    assert_eq!(
        verdicts(&reports),
        [("ota-update", "pass"), ("requests", "pass")]
    );
    assert!(
        !feeder
            .actions
            .iter()
            .any(|a| a.starts_with("stage") || a.starts_with("offer"))
    );
}

#[test]
fn only_selects_in_suite_order_and_refuses_unknown_names() {
    let names: Vec<_> = select(&args(&["--only", "network-bounce,requests"]))
        .unwrap()
        .iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["ota-update", "requests", "network-bounce"]);

    let error = select(&args(&["--only", "reboot"])).err().unwrap();
    assert!(
        matches!(&error, Error::UnknownScenario { name } if name == "reboot"),
        "{error:?}"
    );
    // The refusal lists what --only takes.
    assert!(
        error
            .to_string()
            .starts_with("Unknown scenario \"reboot\"; --list shows them (ota-update, requests,"),
        "{error}"
    );
}

#[test]
fn an_admitted_stranger_fails_and_stops_the_rest() {
    let mut feeder = Feeder::new(GOOD);
    feeder.admits_strangers = true;
    let (reports, _) = suite(&mut feeder, &["--review", "r"]);

    let v = verdicts(&reports);
    assert_eq!(v[2], ("request-auth", "fail"));
    assert!(
        v[3..].iter().all(|(_, verdict)| *verdict == "skip"),
        "{v:?}"
    );
    let Verdict::Fail(reason) = &reports[2].verdict else {
        unreachable!()
    };
    assert!(reason.contains("unenrolled key"), "{reason}");
    assert_eq!(exit_code(&reports, None), 1);
}

#[test]
fn keep_going_runs_past_a_failure() {
    let mut feeder = Feeder::new(GOOD);
    feeder.admits_strangers = true;
    let (reports, _) = suite(&mut feeder, &["--review", "r", "--keep-going"]);

    assert_eq!(verdicts(&reports)[5], ("network-bounce", "pass"));
}

#[test]
fn settings_lost_across_a_cold_boot_fail_and_are_still_restored() {
    let mut feeder = Feeder::new(GOOD);
    feeder.settings_lost_on_boot = true;
    let (reports, _) = suite(
        &mut feeder,
        &["--review", "r", "--power", "--only", "settings-persist"],
    );

    assert_eq!(verdicts(&reports)[1], ("settings-persist", "fail"));
    assert_eq!(
        (feeder.lights, feeder.sound, feeder.volume),
        (true, true, 40)
    );
}

#[test]
fn a_silent_feeder_stops_the_suite_with_uart_recovery() {
    let mut feeder = Feeder::new(OLD);
    feeder.silent = true;
    let (reports, silent) = suite(&mut feeder, &["--build"]);

    assert_eq!(verdicts(&reports)[0], ("ota-update", "fail"));
    assert!(silent.is_some());
    assert_eq!(exit_code(&reports, silent.as_ref()), 2);

    // An unpowered feeder (seen on the bench) is caught by the install
    // watch itself, not after its 15-minute deadline.
    assert!(
        reports[0].took < SILENCE + Duration::from_secs(10),
        "{}",
        reports[0].took.as_secs_f64()
    );

    let hint = recovery_hint("run1", None).join("\n");
    assert!(
        hint.contains("wellfed-bench flash-prepare run1-recover"),
        "{hint}"
    );
}

#[test]
fn the_log_joins_split_lines_and_counts_from_a_mark() {
    let mut log = Log::default();
    log.feed(
        b"PLAPI1 session slot=1 state=ended reas",
        Duration::from_secs(1),
    );
    assert_eq!(log.len(), 0);
    log.feed(
        b"on=noise\r\nboot noise\r\nPLAPI1 session slot=2 state=ended reason=noise\n",
        Duration::from_secs(2),
    );

    assert_eq!(log.len(), 3);
    let refused = |e: &wellfed_core::record::Event<'_>| ended_for(e, "noise");
    assert_eq!(log.count(0, refused), 2);
    assert_eq!(log.count(1, refused), 1);
    assert_eq!(log.find(1, refused), Some(2));
    assert_eq!(log.last_record_at(), Some(Duration::from_secs(2)));
}

#[test]
fn http_responses_parse_with_and_without_chunking() {
    let plain = b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\n{\"ok\":true}";
    assert_eq!(
        wellfed_stock_api::parse_response(plain).unwrap(),
        (200, json!({"ok": true}))
    );

    let chunked =
        b"HTTP/1.1 503 Service Unavailable\r\ntransfer-encoding: chunked\r\n\r\n6\r\n{\"a\":1\r\n1\r\n}\r\n0\r\n\r\n";
    assert_eq!(
        wellfed_stock_api::parse_response(chunked).unwrap(),
        (503, json!({"a": 1}))
    );
}

#[test]
fn the_record_lists_every_verdict_as_physical_evidence() {
    let reports = [
        Report {
            scenario: "ota-update",
            verdict: Verdict::Pass,
            took: Duration::from_millis(241_040),
        },
        Report {
            scenario: "ota-recovery",
            verdict: Verdict::Skip("needs --power".into()),
            took: Duration::ZERO,
        },
    ];
    let summary = serde_json::to_value(summary(
        "run1",
        &reports,
        None,
        &["private/captures/run1-uart-01".into()],
    ))
    .unwrap();

    assert_eq!(summary["evidence"], "physical");
    assert_eq!(summary["scenarios"][0]["seconds"], 241.0);
    assert_eq!(summary["scenarios"][1]["verdict"], "skip");
    assert_eq!(summary["captures"][0], "private/captures/run1-uart-01");
}

#[test]
fn the_scenarios_run_in_catalog_order() {
    let names: Vec<_> = SCENARIOS.iter().map(|s| s.name).collect();
    let catalog: Vec<_> = wellfed_formats::bench::catalog::SCENARIOS
        .iter()
        .map(|s| s.name)
        .collect();

    assert_eq!(names, catalog);
}

fn feed_writes(feeder: &mut Feeder) -> Report {
    let (reports, silent) = suite(
        feeder,
        &["--review", "r", "--feeds", "--only", "feed-writes"],
    );
    assert!(silent.is_none());
    assert_eq!(reports[0].verdict, Verdict::Pass);
    reports[1].clone()
}

#[test]
fn feed_writes_holds_a_session_across_the_feeds_and_puts_everything_back() {
    let mut feeder = Feeder::new(GOOD);
    let report = feed_writes(&mut feeder);

    assert_eq!(report.verdict, Verdict::Pass, "{:#?}", feeder.notes);
    // The held session opened before the first feed and carried every
    // write made during one.
    let at = |action: &str| feeder.actions.iter().position(|a| a == action).unwrap();
    assert!(at("hold") < at("ask feed"));
    for action in [
        "held put-settings",
        "held set-lock",
        "held time-sync",
        "held cancel-feed",
    ] {
        assert!(at(action) > at("ask feed"), "{action}");
    }
    assert!(at("release") > at("held cancel-feed"));
    assert_eq!(
        feeder.actions.iter().filter(|a| *a == "ask feed").count(),
        2
    );
    assert!(!feeder.actions.iter().any(|a| a == "write during feed"));
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("settings change during feed 12") && n.contains("busy")),
        "{:#?}",
        feeder.notes
    );
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("Feed 13 cancelled with 0/2"))
    );
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("writes_deferred unlogged then 2")),
        "{:#?}",
        feeder.notes
    );

    // The change went through after the feed, and everything came back.
    assert_eq!(
        (feeder.lights, feeder.locked, feeder.held),
        (true, false, false)
    );
    assert_eq!(feeder.generation, 2981);
}

#[test]
fn a_settings_save_during_a_feed_fails_feed_writes_and_is_put_back() {
    let mut feeder = Feeder::new(GOOD);
    feeder.saves_during_feed = true;
    let Verdict::Fail(reason) = feed_writes(&mut feeder).verdict else {
        panic!("{:#?}", feeder.notes);
    };

    assert!(
        reason.starts_with("Store writes during feed 12: [sdk] PLCFG1 saved"),
        "{reason}"
    );
    assert!(feeder.lights && !feeder.locked);
}

#[test]
fn a_cancel_that_leaves_the_feed_running_fails_feed_writes() {
    let mut feeder = Feeder::new(GOOD);
    feeder.ignores_cancel = true;
    let Verdict::Fail(reason) = feed_writes(&mut feeder).verdict else {
        panic!("{:#?}", feeder.notes);
    };

    assert!(reason.starts_with("Feed 13 was not cancelled"), "{reason}");
    assert!(!feeder.held);
}

#[test]
fn a_feeder_that_restarts_during_a_feed_fails_feed_writes() {
    let mut feeder = Feeder::new(GOOD);
    feeder.line(5.0, "PLFW1 start build=a1b2 bank=1 reset=0x8 mmu=1");
    let Verdict::Fail(reason) = feed_writes(&mut feeder).verdict else {
        panic!("{:#?}", feeder.notes);
    };

    assert!(reason.starts_with("The feeder restarted"), "{reason}");
}

#[test]
fn a_feed_whose_answer_is_lost_is_never_sent_again_and_is_waited_out() {
    let mut feeder = Feeder::new(GOOD);
    feeder.loses_feed_answer = true;
    let Verdict::Fail(reason) = feed_writes(&mut feeder).verdict else {
        panic!("{:#?}", feeder.notes);
    };

    assert!(reason.contains("the feed is uncertain"), "{reason}");
    assert_eq!(
        feeder.actions.iter().filter(|a| *a == "ask feed").count(),
        1
    );
    assert!(!feeder.actions.iter().any(|a| a == "write during feed"));
    // The owner's feed ended before the scenario put anything back.
    assert!(feeder.feeding().is_none());
    assert!(feeder.lights && !feeder.locked);
}

#[test]
fn a_held_session_lost_mid_feed_restores_only_after_the_feed() {
    let mut feeder = Feeder::new(GOOD);
    feeder.held_dies_mid_feed = true;
    let Verdict::Fail(reason) = feed_writes(&mut feeder).verdict else {
        panic!("{:#?}", feeder.notes);
    };

    assert!(reason.contains("no held session"), "{reason}");
    assert!(!feeder.actions.iter().any(|a| a == "write during feed"));
    assert_eq!(
        feeder.actions.iter().filter(|a| *a == "ask feed").count(),
        1
    );
    assert!(
        feeder
            .notes
            .iter()
            .any(|n| n.contains("Waiting for feed 12 to end"))
    );
}

#[test]
fn a_feed_that_never_ends_stops_the_suite_without_restoring() {
    let mut feeder = Feeder::new(GOOD);
    feeder.feed_never_ends = true;
    let (reports, _) = suite(
        &mut feeder,
        &[
            "--review",
            "r",
            "--feeds",
            "--keep-going",
            "--only",
            "feed-writes,soak",
            "--soak-minutes",
            "1",
        ],
    );

    assert_eq!(
        verdicts(&reports),
        [
            ("ota-update", "pass"),
            ("feed-writes", "fail"),
            ("soak", "skip")
        ]
    );
    let Verdict::Fail(reason) = &reports[1].verdict else {
        unreachable!()
    };
    assert!(reason.contains("feed 12 may still be running"), "{reason}");
    // Nothing went to the feeder after the feed but reads.
    let after = feeder.actions.iter().position(|a| a == "release").unwrap();
    assert!(
        feeder.actions[after + 1..]
            .iter()
            .all(|a| a == "ask status" || a == "ask get-settings"),
        "{:?}",
        &feeder.actions[after + 1..]
    );
}

#[test]
fn a_lost_feed_request_with_no_feed_record_stops_the_suite_without_restoring() {
    let mut feeder = Feeder::new(GOOD);
    feeder.drops_feed = true;
    let (reports, _) = suite(
        &mut feeder,
        &[
            "--review",
            "r",
            "--feeds",
            "--keep-going",
            "--only",
            "feed-writes,soak",
            "--soak-minutes",
            "1",
        ],
    );

    assert_eq!(
        verdicts(&reports),
        [
            ("ota-update", "pass"),
            ("feed-writes", "fail"),
            ("soak", "skip")
        ]
    );
    let Verdict::Fail(reason) = &reports[1].verdict else {
        unreachable!()
    };
    assert!(reason.contains("the feed is uncertain"), "{reason}");
    assert_eq!(
        feeder.actions.iter().filter(|a| *a == "ask feed").count(),
        1
    );
    let after = feeder.actions.iter().position(|a| a == "release").unwrap();
    assert!(
        feeder.actions[after + 1..]
            .iter()
            .all(|a| a == "ask status" || a == "ask get-settings"),
        "{:?}",
        &feeder.actions[after + 1..]
    );
}
