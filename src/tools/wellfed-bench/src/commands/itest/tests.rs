//! No device I/O: the scenarios against a scripted feeder whose records
//! follow the firmware's formats, on a virtual clock.

use std::path::Path;

use serde_json::{Value, json};

use super::log::{Log, Record};
use super::*;
use crate::commands::ota::{Firmware, OtaArgs, Variant};
use crate::server::{ForgedCase, LocalStatus, Request};

const GOOD: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const FAILING: &str = "f00df00df00df00df00df00df00df00df00df00df00df00df00df00df00df00d";
const SPEAKER: &str = "5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa5eaa";
const OLD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

enum Event {
    Line(String),
    Running(String),
    Online(bool),
    /// The booted image reports where its Wi-Fi credentials came from.
    Source(&'static str),
    /// The transport drops and reconnects on its own.
    Reconnect,
    /// Bank 0's recovery role asks for the offer: it installs it only if
    /// `serve` (not a fault server) answers then.
    Recovery,
}

/// A feeder that answers the way the hardware did in the FW10–FW25 drills.
struct Feeder {
    now: f64,
    pending: Vec<(f64, Event)>,
    uart: Vec<u8>,
    running: String,
    /// The build bank 1 last confirmed; reinstalling it skips the trial.
    confirmed: String,
    online: bool,
    connects: u64,
    /// Transport sessions so far; each one's health is a new `received_at`.
    sessions: u64,
    /// Until the booted transport is ready, requests are lost (QoS 0).
    ready_at: f64,
    requests: usize,
    lights: bool,
    sound: bool,
    volume: u64,
    generation: u64,
    stored: bool,
    source: &'static str,
    replies: Vec<Value>,
    version: u32,
    offered: Option<Staged>,
    /// When a failing trial gives way; a later offer installs after it.
    fallback_at: Option<f64>,
    /// The last install's start and build, so a power cut can land in it.
    installing: Option<(f64, String)>,
    server: String,
    actions: Vec<String>,
    notes: Vec<String>,

    // Faults to inject.
    silent: bool,
    replay_accepted: bool,
    settings_lost_on_boot: bool,
    /// A hotspot bounce changes the volume, which nothing in it checks.
    volume_drifts: bool,
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
            connects: 1,
            sessions: 1,
            ready_at: 0.0,
            requests: 0,
            lights: true,
            sound: true,
            volume: 40,
            generation: 2979,
            stored: false,
            source: "build",
            replies: Vec::new(),
            version: 19,
            offered: None,
            fallback_at: None,
            installing: None,
            server: "serve".into(),
            actions: Vec::new(),
            notes: Vec::new(),
            silent: false,
            replay_accepted: false,
            volume_drifts: false,
            settings_lost_on_boot: false,
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

    fn release(&mut self) {
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
                Event::Online(online) => {
                    self.sessions += u64::from(online && !self.online);
                    self.online = online;
                }
                Event::Reconnect => {
                    self.connects += 1;
                    self.uart.extend_from_slice(b"PLMQT1 ready ms=8\r\n");
                }
                Event::Recovery => {
                    if let Some(offer) = self.offered.clone().filter(|_| self.server == "serve") {
                        self.install(self.now, offer.version, &offer.build);
                    }
                }
            }
        }
    }

    /// Download from `start`, activate, trial-boot `build` on bank 1, then
    /// confirm (or, for the failing build, give way three boots later).
    fn install(&mut self, start: f64, version: u32, build: &str) {
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
            self.at(delay + 107.0, Event::Running(build.into()));
            return;
        }
        self.line(
            delay + 105.0,
            &format!("PLFW1 start build={build} bank=1 reset=0x0 mmu=1"),
        );
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
            self.at(delay + 141.0, Event::Running(build.into()));
        }
    }

    fn reply(&mut self, reply: Value) {
        self.replies.push(reply);
    }

    fn settings_reply(&self, id: &str) -> Value {
        json!({"id": id, "op": "settings", "ok": true, "pending": false,
               "lights": u8::from(self.lights), "sound": u8::from(self.sound),
               "volume": self.volume, "generation": self.generation, "writable": true})
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

        // As on the bench: the old session still looks online and fresh
        // until the broker's keepalive lapses, unless the new one is quicker.
        let ready = if self.stored { 200.0 } else { 20.0 };
        self.ready_at = self.now + ready;
        self.at(ready.min(30.0) - 0.1, Event::Online(false));
        self.line(ready - 0.1, "PLMQT1 ready ms=9");
        let source = if self.stored { "fallback" } else { "build" };
        self.at(ready, Event::Source(source));
        self.at(ready, Event::Online(true));
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
        let failing = args.variant == Some(Variant::Fw11Fail)
            || review.to_string_lossy().ends_with("-fail-review");
        let speaker = args.firmware == Some(Firmware::Speaker)
            || review.to_string_lossy().contains("speaker");
        let build = match (failing, speaker) {
            (true, _) => FAILING,
            (_, true) => SPEAKER,
            _ => GOOD,
        };
        self.actions
            .push(format!("stage {} v{}", args.name, self.version));
        Ok(Some(Staged {
            build: build.into(),
            target: crate::commands::ota::Target::Replacement,
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
        self.line(1.0, "PLMQT1 closed reason=eof");
        self.line(6.0, "PLMQT1 connect n=2 ok=1 ms=40");
        self.line(7.0, "PLMQT1 ready ms=12");
        // Offers live in serve's memory only.
        self.offered = None;
        self.server = match &server {
            Server::Serve | Server::StockTrial => "serve".into(),
            Server::Broker => "broker".into(),
            Server::Faulty { fault, staged } => {
                let stage = match fault {
                    Fault::Corrupt => Some("hash"),
                    Fault::Range => Some("range"),
                    Fault::Stall => Some("timeout"),
                    Fault::None => None,
                };
                // A failing trial still under way gives way first; bank 0's
                // recovery role then meets this server.
                let start = self.fallback_at.take().unwrap_or(self.now);
                match stage {
                    Some(stage) => self.line(
                        start - self.now + 90.0,
                        &format!("PLOTA1 error stage={stage}"),
                    ),
                    None => {
                        let (version, build) = (staged.version, staged.build.clone());
                        self.install(start + 10.0, version, &build);
                    }
                }
                format!("fault {}", fault.arg())
            }
        };
        Ok(())
    }

    fn forge(&mut self) -> Result<Vec<ForgedCase>> {
        self.actions.push("forge".into());
        let refused = json!({"ok": false, "error": "auth"});
        let replay = if self.replay_accepted {
            json!({"ok": true, "op": "settings"})
        } else {
            refused.clone()
        };
        let refusals = if self.replay_accepted { 2 } else { 3 };
        // As on the bench: each record is logged after its reply is
        // published, so the UART chunk lands after the forger has its answers.
        for _ in 0..refusals {
            self.line(1.5, "PLMQT1 request op=unauthenticated ok=0");
        }
        let lines = json!([
            {"case": "unsigned", "reply": refused},
            {"case": "wrong_key", "reply": refused},
            {"case": "valid", "reply": {"ok": true, "op": "settings"}},
            {"case": "replay", "reply": replay},
        ]);
        Ok(serde_json::from_value(lines)?)
    }

    fn offer(&mut self, staged: Option<&Staged>) -> Result<()> {
        self.actions.push(match staged {
            Some(s) => format!("offer v{}", s.version),
            None => "offer clear".into(),
        });
        self.offered = staged.cloned();
        let Some(staged) = staged else { return Ok(()) };
        if let Some(fallback) = self.fallback_at.take() {
            // Bank 0's recovery role installs whatever is offered then.
            self.install(fallback, staged.version, &staged.build.clone());
        } else if staged.build == self.running {
            self.line(
                30.0,
                &format!("PLWCH1 offer=1 same=1 version={}", staged.version),
            );
        } else {
            self.line(
                30.0,
                &format!("PLWCH1 give_way=1 version={} restart=1", staged.version),
            );
            self.at(38.0, Event::Recovery);
        }
        Ok(())
    }

    fn request(&mut self, request: &Request) -> Result<String> {
        // Judged by its JSON, as the server forwards it.
        let body = serde_json::to_value(request)?;
        let id = format!("00000000-0000-4000-8000-{:012}", self.requests);
        self.requests += 1;
        self.actions.push(format!("request {}", body["op"]));
        if self.silent || self.now < self.ready_at {
            return Ok(id);
        }
        match body["op"].as_str().unwrap() {
            "ping" => {
                self.line(0.5, "PLMQT1 request op=ping ok=1");
                self.reply(json!({"id": id, "op": "ping", "ok": true, "uptime_ms": 1234}));
            }
            "health" => self.reply(json!({"id": id, "op": "health", "ok": true})),
            "settings" => {
                let change = ["lights", "sound", "volume"]
                    .iter()
                    .any(|k| !body[k].is_null());
                if change {
                    if let Some(on) = body["lights"].as_bool() {
                        self.lights = on;
                    }
                    if let Some(on) = body["sound"].as_bool() {
                        self.sound = on;
                    }
                    if let Some(volume) = body["volume"].as_u64() {
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
                }
                let reply = self.settings_reply(&id);
                self.reply(reply);
            }
            "wifi" => {
                if body["clear"] == true {
                    self.stored = false;
                    self.line(1.0, "PLCFG1 net saved generation=5 stored=0");
                } else if body["ssid"].is_string() {
                    self.stored = true;
                    self.line(1.0, "PLCFG1 net saved generation=4 stored=1");
                }
                let stored = if self.stored {
                    json!("1a2b3c4d")
                } else {
                    Value::Null
                };
                self.reply(json!({"id": id, "op": "wifi", "ok": true, "pending": false,
                                  "stored": stored, "generation": 4, "writable": true,
                                  "source": self.source}));
            }
            other => panic!("unexpected op {other}"),
        }
        Ok(id)
    }

    fn status(&mut self) -> Result<LocalStatus> {
        self.release();
        let stale = !self.online || self.silent;
        let status = json!({"replacement": {
            "health": {
                "build": self.running, "bank": 1, "seq": 7, "outputs": "stopped", "faults": [],
                "clock": {"syncs": 2, "unix_ms": 1_792_670_399_800i64},
                "wifi": {"source": self.source},
                "transport": {"connects": self.connects},
            },
            "received_at": format!("2026-10-22T12:00:{:02}Z", self.sessions % 60),
            "age_ms": 1000,
            "online": self.online,
            "stale": stale,
            "replies": self.replies.iter().map(|m| json!({"message": m})).collect::<Vec<_>>(),
        }});
        Ok(serde_json::from_value(status)?)
    }

    fn hotspot(&mut self, up: bool) -> Result<()> {
        self.actions
            .push(format!("hotspot {}", if up { "up" } else { "down" }));
        if up {
            if self.volume_drifts {
                self.volume = 50;
            }
            self.line(15.0, "PLMQT1 connect n=3 ok=1 ms=50");
            self.line(16.0, "PLMQT1 ready ms=10");
            self.connects += 1;
            self.at(16.0, Event::Online(true));
        } else {
            self.line(40.0, "PLMQT1 closed reason=timeout");
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
                if let Some(offer) = self.offered.clone() {
                    self.install(self.now + 10.0, offer.version, &offer.build);
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
                self.at(40.0, Event::Running(build));
            }
            _ => self.boot(),
        }
        Ok(())
    }

    fn uart(&mut self) -> Result<Vec<u8>> {
        self.release();
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
    assert_eq!(feeder.server, "serve");
    assert!(feeder.offered.is_none());
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
            ("ota-recovery", "skip"),
            ("ota-power-cuts", "skip"),
            ("stock-roundtrip", "skip"),
        ]
    );
    assert!(!feeder.actions.iter().any(|a| a == "power cycle"));
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
fn a_soak_fails_on_a_reconnect() {
    let mut feeder = Feeder::new(GOOD);
    feeder.at(200.0, Event::Reconnect);
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
    assert!(reason.contains("reconnected"), "{reason}");
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
fn an_accepted_replay_fails_and_stops_the_rest() {
    let mut feeder = Feeder::new(GOOD);
    feeder.replay_accepted = true;
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
    assert!(reason.contains("replay"), "{reason}");
    assert_eq!(exit_code(&reports, None), 1);
    // The bare broker was replaced by serve again.
    assert_eq!(feeder.server, "serve");
}

#[test]
fn keep_going_runs_past_a_failure() {
    let mut feeder = Feeder::new(GOOD);
    feeder.replay_accepted = true;
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
    log.feed(b"PLMQT1 request op=unauth", 1.0);
    assert_eq!(log.len(), 0);
    log.feed(
        b"enticated ok=0\r\nboot noise\r\nPLMQT1 request op=unauthenticated ok=0\n",
        2.0,
    );

    assert_eq!(log.len(), 3);
    let refused = |r: &Record| r.is("PLMQT1", &[("ok", "0")]);
    assert_eq!(log.count(0, refused), 2);
    assert_eq!(log.count(1, refused), 1);
    assert_eq!(log.find(1, refused), Some(2));
    assert_eq!(log.last_record_at(), Some(2.0));
}

#[test]
fn http_responses_parse_with_and_without_chunking() {
    let plain = b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\n{\"ok\":true}";
    assert_eq!(
        crate::local_api::parse_response(plain).unwrap(),
        (200, json!({"ok": true}))
    );

    let chunked =
        b"HTTP/1.1 503 Service Unavailable\r\ntransfer-encoding: chunked\r\n\r\n6\r\n{\"a\":1\r\n1\r\n}\r\n0\r\n\r\n";
    assert_eq!(
        crate::local_api::parse_response(chunked).unwrap(),
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
