//! No device I/O: the scenarios against a scripted feeder whose records
//! follow the firmware's formats, on a virtual clock.

use std::path::Path;

use wellfed_model::{Generation, OpenRefusal, Refusal, Settings, WifiSource};
use serde_json::json;

use super::log::{Log, Record};
use super::*;
use wellfed_bench::commands::ota::Variant;

const GOOD: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const FAILING: &str = "f00df00df00df00df00df00df00df00df00df00df00df00df00df00df00df00d";
const TWIN: &str = "7e157e157e157e157e157e157e157e157e157e157e157e157e157e157e157e15";
const OLD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

enum Event {
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

    // Faults to inject.
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
                Event::Line(text) => {
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
            self.at(delay + 106.0, Event::Running(build.into()));
            return;
        }
        self.line(
            delay + 105.0,
            &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
        );
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
            clock_ms: self
                .synced
                .then(|| u64::try_from(self.unix_ms() - 300).unwrap()),
            settings_generation: self.generation,
            wifi_generation: self.wifi_generation,
            locked: Some(self.locked),
        }
    }

    /// The feed whose outputs are claimed now.
    fn feeding(&self) -> Option<Feeding> {
        self.feeding.filter(|feed| self.now < feed.end)
    }

    /// The owner's records for feed `id`, from its reservation to Idle.
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
        self.line(
            2.0,
            "PLOWN1 state=4 fault=0 supervisor=0 stopped=0 optics_off=0",
        );
        let end = if self.feed_never_ends {
            f64::INFINITY
        } else {
            self.finish(2.0 + drive, id, portions, portions, 1, 1);
            self.now + 2.0 + drive + 0.02
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
                 reason={reason} durable=1 drive_ms=9000 rising={completed} falling={completed} \
                 first_edge_ms=7840 current_max=168 current_mean=59 samples=300"
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
            return Err(Error::Bench("the feeder did not answer".into()));
        }
        self.logged(ask.name());
        if matches!(ask, Ask::Feed { .. }) && self.drops_feed {
            return Err(Error::Bench("the feeder did not answer".into()));
        }
        if let Ask::Feed { portions } = ask
            && self.loses_feed_answer
            && self.feeding().is_none()
        {
            self.feed(*portions);
            return Err(Error::Bench("the feeder did not answer".into()));
        }
        // An event recorded during a feed waits for it, and the interlock
        // logs the writer's wait.
        if matches!(ask, Ask::SetLock { .. } | Ask::SyncClock) && self.feeding().is_some() {
            self.deferred += 1;
            let deferred = self.deferred;
            self.line(
                0.3,
                &format!(
                    "PLOWN1 interlock refused=0 deferred=0 forced=0 waited_max_ms=0 long_joins=0 \
                     writes_deferred={deferred} writes_forced=0 write_waited_max_ms=0 feeds_delayed=0"
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
            Ask::UpdateState | Ask::CancelUpdate => Answer::Unexpected,
        })
    }

    /// A request record, as the firmware logs one after answering.
    fn logged(&mut self, op: &str) {
        self.line(0.5, &format!("PLAPI1 request slot=0 op={op} result=ok"));
    }

    fn boot(&mut self) {
        self.line(
            3.0,
            &format!("PLFW1 start build={} bank=1 reset=0x0 mmu=1", self.running),
        );
        if self.settings_lost_on_boot {
            self.lights = true;
            self.sound = true;
            self.volume = 40;
        }
        self.boots += 1;
        self.synced = false;

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
        let build = match (failing, twin) {
            (true, _) => FAILING,
            (_, true) => TWIN,
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
            self.line(1.0, "PLWCH1 refused reason=same");
        } else {
            self.line(
                30.0,
                &format!("PLWCH1 give_way=1 version={} restart=1", staged.version),
            );
            self.at(38.0, Event::Recovery);
        }
        Ok(())
    }

    fn offered(&mut self) -> Option<std::result::Result<(), String>> {
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
            return Err(Error::Bench(
                "the feeder did not answer the handshake".into(),
            ));
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
            return Err(Error::Bench("no held session is open".into()));
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

    fn hotspot(&mut self, up: bool) -> Result<()> {
        self.actions
            .push(format!("hotspot {}", if up { "up" } else { "down" }));
        if up {
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

    fn uart(&mut self) -> Result<Vec<u8>> {
        self.deliver();
        Ok(std::mem::take(&mut self.uart))
    }

    fn wait(&mut self, seconds: f64) -> Result<()> {
        self.now += seconds;
        Ok(())
    }

    fn clock(&self) -> f64 {
        self.now
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
    assert_eq!(names("standard").len(), SCENARIOS.len() - 1);
    assert!(!names("standard").contains(&"soak"));
    assert_eq!(names("soak").len(), SCENARIOS.len());
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

    let error = select(&args(&["--only", "reboot"]))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("Unknown scenario \"reboot\""), "{error}");
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
        reports[0].seconds < SILENT_SECONDS + 10.0,
        "{}",
        reports[0].seconds
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
    log.feed(b"PLAPI1 session slot=1 state=ended reas", 1.0);
    assert_eq!(log.len(), 0);
    log.feed(
        b"on=noise\r\nboot noise\r\nPLAPI1 session slot=2 state=ended reason=noise\n",
        2.0,
    );

    assert_eq!(log.len(), 3);
    let refused = |r: &Record| r.is("PLAPI1", &[("reason", "noise")]);
    assert_eq!(log.count(0, refused), 2);
    assert_eq!(log.count(1, refused), 1);
    assert_eq!(log.find(1, refused), Some(2));
    assert_eq!(log.last_record_at(), Some(2.0));
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
            seconds: 241.04,
        },
        Report {
            scenario: "ota-recovery",
            verdict: Verdict::Skip("needs --power".into()),
            seconds: 0.0,
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
