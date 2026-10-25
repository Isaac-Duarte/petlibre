//! Over-the-air iteration: optionally build and freeze a review with
//! `wellfed-build`, sign it with `wellfed-build`'s library (the bench
//! key, and only a frozen review), offer it with `wellfed-local` to the server
//! that is already running, watch receive-only UART captures until the new
//! build confirms, falls back, or time runs out, then check that the device
//! API reports the new build on bank 1. Each phase is timed for the
//! timing record.
//!
//! A review of the original stock image (`image_kind: stock`) installs the
//! same way, through the replacement's own signed update: its payload hash
//! stands in for a build ID, success is stock's `app_main:version:` banner
//! after activation, and there is no status to check, since stock serves no
//! device API. Stock has no trial, so a stock image that failed to
//! boot would need the UART recovery.
//!
//! The bench never starts the server or the hotspot, never switches a relay
//! here, and never writes to the feeder. [`Watch`] classifies the feeder's
//! `PL…` lines without I/O, so it is tested on real lines; the feeder's
//! status is read with [`DeviceStatus::matches`].

use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use wellfed_build::signing::{Frozen, KEY_PATH, Key, Motion};
use wellfed_core::record::{Record, tag};
use wellfed_formats::capture::SerialEvent;
use wellfed_formats::manifest::BuildManifest;

use super::Bench;
use crate::config::{CAPTURES, VERIFICATION};
use crate::device::DeviceStatus;
use crate::{Error, Result, require};

/// Each capture segment; the classifier reads it once it ends.
const SEGMENT_SECONDS: f64 = 60.0;

/// `wellfed-core::ota`'s signed manifest: magic, version (u32 LE at 8),
/// image length, bank, hashes, then the 64-byte signature. Only the header is
/// read here, to pick the next version; the firmware checks the signature.
const MANIFEST_MAGIC: &[u8; 8] = b"PLOTA1\0\0";
const MANIFEST_LEN: usize = 148;

/// The in-process signing step's status label.
pub(crate) const SIGN_STEP: &str = "sign";
/// Status steps after signing and the `wellfed-local` offer, in order.
pub(crate) const STAGES: [&str; 4] = ["give way", "download", "new build boot", "confirm"];
/// Status steps of `--build`, before signing.
const BUILD_STEPS: [&str; 2] = ["firmware build", "review image"];
/// The last status step: the device API's view of the new build.
pub(crate) const HEALTH_STEP: &str = "health";

/// The build's output, which the review freezes (root-relative).
const BUILD_OUTPUT: &str = "private/firmware-build/plaf103/firstboot";
/// The confirmed build serves the device API at once; allow for a slow join.
const HEALTH_SECONDS: f64 = 90.0;
const HEALTH_POLL_SECONDS: f64 = 5.0;

/// Where a run takes the image it offers from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A frozen review, `--review`.
    Review,
    /// `--build`: build this checkout and freeze its review first.
    Build,
}

/// Status steps for one `ota` run.
pub fn steps(source: Source) -> Vec<&'static str> {
    let built: &[&str] = match source {
        Source::Build => &BUILD_STEPS,
        Source::Review => &[],
    };
    built
        .iter()
        .chain(&[SIGN_STEP, "wellfed-local ota-offer"])
        .chain(&STAGES)
        .chain(&[HEALTH_STEP])
        .copied()
        .collect()
}

/// Why the bench never builds the motor firmware itself.
const MOTION_MANUAL: &str = "drives the motor; motion builds stay manual (build and review it yourself, then use --review with --attended-motion)";

/// A firmware `--build` may make (`src/firmware/wellfed/<name>`): none
/// moves anything at startup, and `feeder` drives the motor only for an
/// authenticated feed request, which the bench never sends. The motor
/// firmware is refused here: motion builds stay a manual, attended step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Firmware {
    /// The guided QA self-test; `wellfed-build`'s default.
    #[default]
    Qa,
    /// The optics probe.
    Optics,
    /// The panel buttons, read-only.
    Buttons,
    /// Read-only power-input samples.
    Adc,
    /// Tunes and the signed network audio stream.
    Speaker,
    /// Blinks the connection indicator.
    Blink,
    /// Trial images, one per `Variant`.
    Trial,
    /// The product firmware.
    Feeder,
}

impl Firmware {
    const ALL: [Self; 8] = [
        Self::Qa,
        Self::Optics,
        Self::Buttons,
        Self::Adc,
        Self::Speaker,
        Self::Blink,
        Self::Trial,
        Self::Feeder,
    ];

    /// The firmware's directory name, which `wellfed-build --firmware`
    /// selects.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Qa => "qa",
            Self::Optics => "optics",
            Self::Buttons => "buttons",
            Self::Adc => "adc",
            Self::Speaker => "speaker",
            Self::Blink => "blink",
            Self::Trial => "trial",
            Self::Feeder => "feeder",
        }
    }
}

/// `--firmware`'s parser, with a reason for the motor firmware.
pub(crate) fn parse_firmware(text: &str) -> std::result::Result<Firmware, String> {
    if text == "motor" {
        return Err(format!("motor {MOTION_MANUAL}"));
    }
    Firmware::ALL
        .into_iter()
        .find(|firmware| firmware.name() == text)
        .ok_or_else(|| {
            let known: Vec<&str> = Firmware::ALL.iter().map(|f| f.name()).collect();
            format!("unknown firmware {text:?}; use {}", known.join(", "))
        })
}

/// A variant the bench may build: the trial firmware's and the feeder's.
/// Motion variants are never among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Never confirms; the watchdog resets it, so KM0 falls back.
    Stall,
    /// Fails its startup diagnostic on purpose.
    Fail,
    /// The feeder streaming raw noise samples.
    Entropy,
    /// The feeder stalling its loop on purpose, so the feeding owner's beat
    /// lapses into a watchdog reset; no motion.
    OwnerStall,
}

impl Variant {
    /// The variant's name, which `wellfed-build --variant` selects.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Stall => "stall",
            Self::Fail => "fail",
            Self::Entropy => "entropy",
            Self::OwnerStall => "owner-stall",
        }
    }

    /// The firmware that declares it.
    pub(crate) fn firmware(self) -> Firmware {
        match self {
            Self::Stall | Self::Fail => Firmware::Trial,
            Self::Entropy | Self::OwnerStall => Firmware::Feeder,
        }
    }
}

/// `--variant`'s parser, with a reason for the motion variant.
pub(crate) fn parse_variant(text: &str) -> std::result::Result<Variant, String> {
    match text {
        "stall" => Ok(Variant::Stall),
        "fail" => Ok(Variant::Fail),
        "entropy" => Ok(Variant::Entropy),
        "owner-stall" => Ok(Variant::OwnerStall),
        "pulse" => Err(format!("pulse {MOTION_MANUAL}")),
        other => Err(format!(
            "unknown variant {other:?}; the trial firmware has stall and fail, the feeder entropy \
             and owner-stall"
        )),
    }
}

/// What `--build` makes: a firmware and, for the trial firmware, a variant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Image {
    pub firmware: Firmware,
    pub variant: Option<Variant>,
}

impl Image {
    pub(crate) const fn of(firmware: Firmware) -> Self {
        Self {
            firmware,
            variant: None,
        }
    }

    pub(crate) const fn trial(variant: Variant) -> Self {
        Self {
            firmware: Firmware::Trial,
            variant: Some(variant),
        }
    }

    /// `--firmware <name> [--variant <name>]` for `wellfed-build`.
    fn args(self) -> Vec<String> {
        let mut args = vec!["--firmware".to_string(), self.firmware.name().to_string()];
        if let Some(variant) = self.variant {
            args.extend(["--variant".to_string(), variant.name().to_string()]);
        }
        args
    }
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.firmware.name())?;
        match self.variant {
            Some(variant) => write!(f, " ({})", variant.name()),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct OtaArgs {
    /// Session name: private/verification/NAME-vN.manifest, private/captures/NAME-01, -02, …
    pub name: String,
    /// Frozen review to offer (its image.bin and build-manifest.json).
    #[arg(long, required_unless_present = "build", conflicts_with = "build")]
    pub review: Option<PathBuf>,
    /// Build the PLAF103 firstboot profile and freeze it as private/verification/NAME-review
    /// first, unless a review built from the same sources is already there.
    #[arg(long)]
    pub build: bool,
    /// With --build: build even when a review of the same sources exists.
    #[arg(long, requires = "build")]
    pub rebuild: bool,
    /// Firmware without startup motion for --build: qa (the default), optics, buttons, adc,
    /// speaker, blink, trial or feeder.
    #[arg(long, value_parser = parse_firmware)]
    pub firmware: Option<Firmware>,
    /// Variant for --build: stall or fail with --firmware trial, entropy or owner-stall with
    /// --firmware feeder.
    #[arg(long, value_parser = parse_variant)]
    pub variant: Option<Variant>,
    /// wellfed-local config (root-relative): its hotspot interface and
    /// feeder MAC find the feeder for the final status check when mDNS does not.
    #[arg(long, default_value = "private/server.json")]
    pub config: PathBuf,
    /// The owner key file the final status check opens its session with.
    #[arg(long, default_value = crate::device::OWNER_KEYS)]
    pub keys: PathBuf,
    /// Manifest version (default: one more than the highest signed in private/verification).
    #[arg(long)]
    pub version: Option<u32>,
    /// How long to watch the UART for an outcome.
    #[arg(long, default_value_t = 20.0)]
    pub minutes: f64,
    /// Allow a review whose manifest names a motion policy (owner present).
    #[arg(long)]
    pub attended_motion: bool,
}

impl OtaArgs {
    pub fn source(&self) -> Source {
        if self.build {
            Source::Build
        } else {
            Source::Review
        }
    }

    /// What `--build` makes.
    pub(crate) fn image(&self) -> Image {
        Image {
            firmware: self.firmware.unwrap_or_default(),
            variant: self.variant,
        }
    }
}

/// The version in a manifest file's header, if it has one.
fn manifest_version(bytes: &[u8]) -> Option<u32> {
    (bytes.len() == MANIFEST_LEN && bytes[..8] == MANIFEST_MAGIC[..])
        .then(|| u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]))
}

/// The highest version among `*.manifest` files in `directory`, with its file.
fn highest_signed(directory: &Path) -> Option<(u32, PathBuf)> {
    fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "manifest"))
        .filter_map(|path| Some((manifest_version(&fs::read(&path).ok()?)?, path)))
        .max_by_key(|(version, _)| *version)
}

/// One thing the feeder reported, in the order it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// The running build saw an offer; `same` when it is its own build.
    Offer {
        version: Option<u32>,
        same: bool,
    },
    /// The running build cleared its marker for the offer and restarts.
    GiveWay {
        version: Option<u32>,
    },
    NoOffer,
    Manifest {
        version: Option<u32>,
        bytes: Option<u64>,
    },
    Progress {
        done: u64,
        total: u64,
    },
    Activated {
        version: Option<u32>,
    },
    /// A build started; `new` when it is the offered build on bank 1.
    Started {
        build: String,
        bank: Option<u32>,
        new: bool,
    },
    Trial {
        boot: u32,
        limit: Option<u32>,
    },
    /// KM0 gave way before the offered build was activated.
    EarlyGiveWay,
    Confirmed {
        detail: String,
    },
    /// Stock's application banner after activation: a stock install passed.
    StockStarted {
        version: String,
    },
    FellBack {
        boot: Option<u32>,
    },
    Failed {
        stage: String,
    },
    /// The recovery role found its attempt marker set and did not arm.
    Skipped,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let or_q = |v: &Option<u32>| v.map_or_else(|| "?".into(), |v| v.to_string());
        match self {
            Self::Offer { same: true, .. } => {
                write!(
                    f,
                    "Running build reports the offer as its own build (same=1)"
                )
            }
            Self::Offer { version, .. } => {
                write!(f, "Running build saw offer version {}", or_q(version))
            }
            Self::GiveWay { version } => write!(
                f,
                "Running build gave way for version {}; restarting into bank 0",
                or_q(version)
            ),
            Self::NoOffer => write!(f, "Recovery role found no offer"),
            Self::Manifest { version, bytes } => write!(
                f,
                "Manifest version {} accepted ({} bytes); downloading",
                or_q(version),
                bytes.map_or_else(|| "?".into(), |b| b.to_string())
            ),
            // `Written sectors: N/M` is a progress count the page draws as a bar.
            Self::Progress { done, total } => write!(f, "Written sectors: {done}/{total}"),
            Self::Activated { version } => write!(
                f,
                "Activated version {}; resetting into bank 1",
                or_q(version)
            ),
            Self::Started { new: true, .. } => write!(f, "New build started on bank 1 (trial)"),
            Self::Started { build, bank, .. } => write!(
                f,
                "Build {} started on bank {}{}",
                build.get(..12).unwrap_or(build),
                or_q(bank),
                if *bank == Some(0) {
                    " (recovery role)"
                } else {
                    ""
                }
            ),
            Self::Trial { boot, limit } => {
                write!(f, "Trial boot {boot} of {}", or_q(limit))
            }
            Self::EarlyGiveWay => write!(f, "KM0 gave way to bank 0 before the new build ran"),
            Self::Confirmed { detail } => write!(f, "New build confirmed ({detail})"),
            Self::StockStarted { version } => write!(f, "Stock {version} started on bank 1"),
            Self::FellBack { boot } => write!(
                f,
                "KM0 rejected the trial after boot {}; bank 0 runs again",
                or_q(boot)
            ),
            Self::Failed { stage } => write!(f, "Update error at stage {stage}"),
            Self::Skipped => write!(
                f,
                "Recovery role skipped the update: an earlier attempt this power cycle reset"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The offered build started on bank 1, then confirmed itself.
    Passed,
    /// KM0 rejected the trial; `boot` is the last trial count seen.
    FellBack { boot: Option<u32> },
    /// The recovery role stopped the update before activation.
    Failed { stage: String },
    /// An attempt reset (a fault or hang) and left the attempt marker set,
    /// so the recovery role will not try again until a cold boot.
    Skipped,
}

/// What an offered image is, from its review's `image_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// A replacement build: it must start on bank 1 and confirm.
    Replacement,
    /// The original stock image: it must print its banner after activation.
    Stock,
}

/// Stock's application banner, followed by its version (`1.1.15`).
const STOCK_BANNER: &str = "app_main:version:";

/// Classifies UART text, fed in pieces, against one offered build.
#[derive(Debug)]
pub(crate) struct Watch {
    build: String,
    target: Target,
    pending: Vec<u8>,
    /// Stream bytes before `pending`.
    consumed: u64,
    started: bool,
    activated: bool,
    trial_boot: Option<u32>,
    outcome: Option<Outcome>,
}

/// Records the bench reads; SDK output between them is ignored.
const TAGS: [&str; 5] = [
    tag::WATCH,
    tag::UPDATE,
    tag::START,
    tag::TRIAL,
    tag::CONFIRM,
];
/// A longer run without a line break is noise, not a record.
const MAX_LINE: usize = 4096;

impl Watch {
    pub(crate) fn of(build: &str, target: Target) -> Self {
        Self {
            build: build.to_ascii_lowercase(),
            target,
            pending: Vec::new(),
            consumed: 0,
            started: false,
            activated: false,
            trial_boot: None,
            outcome: None,
        }
    }

    pub(crate) fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// Take more UART bytes; the events in the complete lines among them. A
    /// partial last line waits for the next piece. Nothing counts after an outcome.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.feed_at(bytes)
            .into_iter()
            .map(|(_, event)| event)
            .collect()
    }

    /// As [`Self::feed`], with each event's stream offset: where its line
    /// ended, counted from the first byte ever fed.
    pub(crate) fn feed_at(&mut self, bytes: &[u8]) -> Vec<(u64, Event)> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();

        while let Some(end) = self.pending.iter().position(|b| matches!(b, b'\n' | b'\r')) {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let at = self.consumed + end as u64;
            self.consumed += line.len() as u64;
            if self.outcome.is_none()
                && let Some(event) = self.line(&String::from_utf8_lossy(&line))
            {
                events.push((at, event));
            }
        }
        if self.pending.len() > MAX_LINE {
            self.consumed += self.pending.len() as u64;
            self.pending.clear();
        }
        events
    }

    fn line(&mut self, line: &str) -> Option<Event> {
        if self.target == Target::Stock
            && self.activated
            && let Some(at) = line.find(STOCK_BANNER)
        {
            let version = line[at + STOCK_BANNER.len()..]
                .split_whitespace()
                .next()
                .unwrap_or("?")
                .to_string();
            self.outcome = Some(Outcome::Passed);
            return Some(Event::StockStarted { version });
        }

        let record = Record::all(line).find(|record| TAGS.contains(&record.tag))?;
        let first = record.first;

        match record.tag {
            tag::WATCH if record.field("give_way") == Some("1") => Some(Event::GiveWay {
                version: record.number("version"),
            }),
            tag::WATCH if record.field("offer") == Some("1") => Some(Event::Offer {
                version: record.number("version"),
                same: record.field("same") == Some("1"),
            }),

            tag::UPDATE => match first {
                "manifest" => Some(Event::Manifest {
                    version: record.number("version"),
                    bytes: record.number("bytes"),
                }),
                "progress" => {
                    let (done, total) = record.field("sectors")?.split_once('/')?;
                    Some(Event::Progress {
                        done: done.parse().ok()?,
                        total: total.parse().ok()?,
                    })
                }
                "activated" => {
                    self.activated = true;
                    self.trial_boot = None;
                    Some(Event::Activated {
                        version: record.number("version"),
                    })
                }
                "error" => {
                    let stage = record.field("stage").unwrap_or("?").to_string();
                    self.outcome = Some(Outcome::Failed {
                        stage: stage.clone(),
                    });
                    Some(Event::Failed { stage })
                }
                "no_offer" => Some(Event::NoOffer),
                "skip" => {
                    self.outcome = Some(Outcome::Skipped);
                    Some(Event::Skipped)
                }
                _ => None,
            },

            tag::START if first == "start" => {
                let build = record.field("build")?.to_ascii_lowercase();
                let bank = record.number("bank");
                let new = build == self.build && bank == Some(1);
                self.started |= new;
                Some(Event::Started { build, bank, new })
            }

            tag::TRIAL if record.field("give_way") == Some("1") => {
                if self.started || self.activated {
                    self.outcome = Some(Outcome::FellBack {
                        boot: self.trial_boot,
                    });
                    Some(Event::FellBack {
                        boot: self.trial_boot,
                    })
                } else {
                    Some(Event::EarlyGiveWay)
                }
            }
            tag::TRIAL if record.field("confirmed") == Some("0") => {
                let boot = record.number("boot")?;
                self.trial_boot = Some(boot);
                Some(Event::Trial {
                    boot,
                    limit: record.number("limit"),
                })
            }

            // A confirmation counts only from the offered build.
            tag::CONFIRM if self.started && record.field("confirmed") == Some("1") => {
                self.outcome = Some(Outcome::Passed);
                let detail = match record.field("healthy_ms") {
                    Some(ms) => format!("healthy_ms={ms}"),
                    None => "already confirmed".into(),
                };
                Some(Event::Confirmed { detail })
            }
            _ => None,
        }
    }
}

/// When the receive byte at `offset` arrived, in Unix seconds, from a
/// capture's `serial-events.jsonl` (one timed record per received chunk).
pub(crate) fn arrival(events: &str, offset: u64) -> Option<f64> {
    let event = SerialEvent::parse_lines(events).find(|event| event.received(offset))?;
    let utc = chrono::DateTime::parse_from_rfc3339(&event.utc).ok()?;
    Some(utc.timestamp_micros() as f64 / 1e6)
}

/// When a feeder event was seen. `exact` when the capture's chunk timing gave
/// it; otherwise it is the end of the segment it arrived in (up to 60 s late).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stamp {
    pub at: f64,
    pub exact: bool,
}

/// The `ota` timing record: how long each phase of one run took, in seconds.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Timing {
    /// Some child ran, so there is something to report.
    pub ran: bool,
    /// `--build` found a review of the same sources and skipped the build.
    pub reused: bool,
    pub build: Option<f64>,
    pub review: Option<f64>,
    pub sign_offer: Option<f64>,
    pub give_way: Option<Stamp>,
    pub activated: Option<Stamp>,
    pub confirmed: Option<Stamp>,
    pub health: Option<f64>,
    pub total: f64,
}

fn between(from: Option<Stamp>, to: Option<Stamp>) -> Option<(f64, bool)> {
    let (from, to) = (from?, to?);
    Some(((to.at - from.at).max(0.0), from.exact && to.exact))
}

/// Seconds as `8.2 s` or `4 min 07 s`.
pub(crate) fn duration(seconds: f64) -> String {
    if seconds < 60.0 {
        return format!("{seconds:.1} s");
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a run's length in whole seconds"
    )]
    let whole = seconds.round() as u64;
    format!("{} min {:02} s", whole / 60, whole % 60)
}

impl Timing {
    /// The summary lines, one per phase; a phase not reached shows `-`.
    pub(crate) fn summary(&self, source: Source) -> Vec<String> {
        let built = source == Source::Build;
        let skipped = || if built { "-" } else { "skipped (--review)" }.to_string();
        let reused = || "reused (same sources)".to_string();
        let plain = |value: Option<f64>| value.map_or_else(|| "-".into(), duration);
        let span = |value: Option<(f64, bool)>| match value {
            None => "-".to_string(),
            Some((seconds, true)) => duration(seconds),
            Some((seconds, false)) => {
                format!("{} (to segment ends, up to 60 s off)", duration(seconds))
            }
        };

        let rows = [
            (
                "Build",
                match (built, self.reused) {
                    (true, true) => reused(),
                    (true, false) => plain(self.build),
                    (false, _) => skipped(),
                },
            ),
            (
                "Review",
                match (built, self.reused) {
                    (true, true) => reused(),
                    (true, false) => plain(self.review),
                    (false, _) => skipped(),
                },
            ),
            ("Sign + offer", plain(self.sign_offer)),
            (
                "Give-way to activation (download)",
                span(between(self.give_way, self.activated)),
            ),
            (
                "Activation to confirmation",
                span(between(self.activated, self.confirmed)),
            ),
            ("Health check", plain(self.health)),
            ("Total", duration(self.total)),
        ];
        std::iter::once("Timing:".to_string())
            .chain(
                rows.iter()
                    .map(|(label, value)| format!("  {label:<34} {value}")),
            )
            .collect()
    }
}

/// A signed update ready to offer: root-relative paths under private/, as
/// the server's offer takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Staged {
    pub build: String,
    pub target: Target,
    pub version: u32,
    pub review: PathBuf,
    pub manifest: String,
    pub image: String,
}

/// What a run will offer, once every refusal has passed.
struct OtaPlan {
    /// Root-relative review: the one given, found, or about to be built.
    review: PathBuf,
    /// No review of these sources exists yet, so `--build` builds one.
    build: bool,
    version: u32,
    /// Where the signed manifest goes.
    signed: PathBuf,
}

/// A frozen review's startup identity, or a refusal.
pub(crate) fn build_id(manifest: &BuildManifest) -> Result<String> {
    let id = manifest.startup_identity.as_deref().unwrap_or_default();
    require(
        is_build_id(id),
        "Review build-manifest.json has no 64-hex startup_identity",
    )?;
    Ok(id.to_ascii_lowercase())
}

fn is_build_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Bench {
    /// Run one `wellfed-local` command; its exit status. An interrupt
    /// during it stops the sequence.
    fn local(&mut self, args: &[String]) -> Result<i32> {
        say!("+ wellfed-local {}", args.join(" "));
        self.status.step(&format!("wellfed-local {}", args[0]));
        let code = self.tools.local(args, &mut self.status)?;

        if crate::interrupted() {
            return Err(Error::Interrupted);
        }
        Ok(code)
    }

    /// A root-relative path under private/, as `wellfed-local ota-offer` requires.
    fn private_arg(&self, path: &Path) -> Result<String> {
        let Some(relative) = path
            .strip_prefix(&self.root)
            .ok()
            .filter(|relative| relative.starts_with("private"))
        else {
            return Err(Error::Bench(format!(
                "{} must be under private/",
                path.display()
            )));
        };
        Ok(relative.display().to_string())
    }

    fn ota_version(&self, given: Option<u32>) -> Result<u32> {
        let highest = highest_signed(&self.root.join(VERIFICATION));
        match (given, highest) {
            (Some(version), Some((top, file))) if version <= top => {
                say!(
                    "Version {version} (--version); {} already signs {top}, so a feeder that installed it refuses this one",
                    self.rel(&file)
                );
                Ok(version)
            }
            (Some(version), _) => {
                say!("Version {version} (--version)");
                Ok(version)
            }
            (None, Some((top, file))) => {
                let version = top
                    .checked_add(1)
                    .ok_or_else(|| Error::Bench("No version above u32::MAX".into()))?;
                say!(
                    "Version {version}: one more than {top} in {}",
                    self.rel(&file)
                );
                Ok(version)
            }
            (None, None) => {
                say!("Version 1: no signed manifest in {VERIFICATION}");
                Ok(1)
            }
        }
    }

    fn segment(&self, name: &str, n: u32) -> PathBuf {
        self.root.join(CAPTURES).join(format!("{name}-{n:02}"))
    }

    /// Refuse a name that already has a numbered capture segment.
    fn unused_segments(&self, name: &str) -> Result<()> {
        let prefix = format!("{name}-");
        let used = fs::read_dir(self.root.join(CAPTURES))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|file| {
                file.strip_prefix(&prefix)
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
        match used {
            Some(file) => Err(Error::Bench(format!(
                "Refusing to reuse {CAPTURES}/{file}; pick a new name"
            ))),
            None => Ok(()),
        }
    }

    /// One line on stdout and in the progress log.
    fn note(&mut self, text: &str) {
        say!("{text}");
        self.status.line(text);
    }

    /// Run one `wellfed-build` step from the root; its exit status and
    /// duration.
    fn run_step(&mut self, step: &str, args: &[String]) -> Result<(i32, f64)> {
        say!("+ wellfed-build {}", args.join(" "));
        self.status.step(step);
        let started = Instant::now();
        let code = self.tools.build(args, &mut self.status)?;

        if crate::interrupted() {
            return Err(Error::Interrupted);
        }
        Ok((code, started.elapsed().as_secs_f64()))
    }

    /// Optionally build and review, sign and offer, watch the UART for the
    /// outcome, then check the server's health; a timing summary follows
    /// whatever ran.
    pub fn ota(&mut self, args: &OtaArgs) -> Result<u8> {
        let started = Instant::now();
        let mut timing = Timing::default();
        let result = self.ota_run(args, &mut timing);

        if timing.ran {
            timing.total = started.elapsed().as_secs_f64();
            for line in timing.summary(args.source()) {
                self.note(&line);
            }
        }
        result
    }

    fn ota_run(&mut self, args: &OtaArgs, timing: &mut Timing) -> Result<u8> {
        let Some(staged) = self.ota_stage(args, timing)? else {
            return Ok(1);
        };

        let offering = Instant::now();
        let offer = [
            "ota-offer",
            "--manifest",
            &staged.manifest,
            "--image",
            &staged.image,
        ];
        let offered = self.local(&offer.map(String::from))?;
        timing.sign_offer =
            Some(timing.sign_offer.unwrap_or(0.0) + offering.elapsed().as_secs_f64());
        if offered != 0 {
            say!(
                "Offer failed; nothing captured. wellfed-local serve and the hotspot must already be up: the bench never starts them."
            );
            return Ok(1);
        }

        say!(
            "Offered version {}; watching the UART for up to {} min",
            staged.version,
            args.minutes
        );
        self.ota_watch(args, &staged, timing)?;
        if staged.target == Target::Stock {
            say!("Stock serves no device API; no status to check");
            return Ok(0);
        }
        // Only the product firmware reports anything after its
        // trial: the bring-up firmwares open no MQTT session and serve no
        // API, so their UART confirmation is the whole check.
        let described = self.root.join(&staged.review).join("build-manifest.json");
        let manifest: wellfed_formats::manifest::BuildManifest =
            wellfed_formats::load(&described).map_err(|error| {
                Error::Bench(format!("{} is unreadable: {error}", self.rel(&described)))
            })?;
        if manifest.firmware.as_deref() != Some(Firmware::Feeder.name()) {
            self.note(&format!(
                "{} serves no device API; the UART confirmation is the check",
                manifest.firmware.as_deref().unwrap_or("this firmware")
            ));
            return Ok(0);
        }

        let checking = Instant::now();
        let health = self.ota_health(args, &staged.build);
        timing.health = Some(checking.elapsed().as_secs_f64());
        health?;
        Ok(0)
    }

    /// Everything before the offer: refusals, the optional build and review,
    /// then signing the next version. `None` when the build or review child
    /// failed (it said why); a review the signer refuses is an error. The
    /// integration suite offers the result through its own server.
    pub(crate) fn ota_stage(
        &mut self,
        args: &OtaArgs,
        timing: &mut Timing,
    ) -> Result<Option<Staged>> {
        let plan = self.ota_plan(args, timing)?;

        if plan.build && !self.ota_build(args, &plan.review, timing)? {
            return Ok(None);
        }

        // Only a frozen review signs: its review.json binds the image, and
        // for a build the manifest that names its identity and motion.
        let review = self.root.join(&plan.review);
        let frozen = Frozen::open(&review).map_err(|source| Error::Signing {
            review: self.rel(&review).into(),
            source,
        })?;
        let build = frozen.build_id();
        let (target, motion) = match frozen.manifest() {
            None => {
                say!("Stock image, payload {}", &build[..12]);
                (Target::Stock, Motion::Refused)
            }
            Some(manifest) => {
                self.attended_motion(manifest, args.attended_motion)?;
                if let Some(variant) = &manifest.variant {
                    say!("Variant {variant}");
                }
                say!("Build {build}");
                let motion = match frozen.motion_policy() {
                    Some(policy) if args.attended_motion => Motion::Attended(policy),
                    _ => Motion::Refused,
                };
                (Target::Replacement, motion)
            }
        };

        self.ota_sign(plan, &frozen, motion, target, timing)
            .map(Some)
    }

    /// Every refusal `ota_stage` can make, before anything is built: the
    /// review to offer and the version to sign.
    fn ota_plan(&mut self, args: &OtaArgs, timing: &mut Timing) -> Result<OtaPlan> {
        require(args.minutes > 0.0, "--minutes must be positive")?;
        // clap counts a flag as always present, so this pairing is checked here.
        require(
            args.build || (args.firmware.is_none() && args.variant.is_none()),
            "--firmware and --variant apply to --build only",
        )?;
        require(
            args.variant
                .is_none_or(|variant| args.firmware == Some(variant.firmware())),
            "--variant needs the firmware that declares it (trial or feeder)",
        )?;
        let mut review = match (&args.review, args.build) {
            (_, true) => PathBuf::from(VERIFICATION).join(format!("{}-review", args.name)),
            (Some(review), false) => review.clone(),
            (None, false) => return Err(Error::Bench("Give --review DIR or --build".into())),
        };

        let mut build = false;
        if args.build {
            match self.cached_review(args)? {
                Some(found) => {
                    self.note(&format!(
                        "Reusing {}: built from these sources (--rebuild to build anyway)",
                        found.display()
                    ));
                    review = found;
                    timing.reused = true;
                }
                None => {
                    self.fresh(&self.root.join(&review))?;
                    build = true;
                }
            }
        }

        self.unused_segments(&args.name)?;
        let version = self.ota_version(args.version)?;
        let signed = self
            .root
            .join(VERIFICATION)
            .join(format!("{}-v{version}.manifest", args.name));
        self.fresh(&signed)?;

        Ok(OtaPlan {
            review,
            build,
            version,
            signed,
        })
    }

    /// Sign `frozen` as `plan`'s version with the bench key, in process.
    fn ota_sign(
        &mut self,
        plan: OtaPlan,
        frozen: &Frozen,
        motion: Motion,
        target: Target,
        timing: &mut Timing,
    ) -> Result<Staged> {
        let image_arg = self.private_arg(&self.root.join(&plan.review).join("image.bin"))?;
        let signed_arg = self.private_arg(&plan.signed)?;
        fs::create_dir_all(self.root.join(VERIFICATION))?;

        say!("Signing version {} with {KEY_PATH}", plan.version);
        self.status.step(SIGN_STEP);
        timing.ran = true;
        let signing = Instant::now();
        let refused = |source| Error::Signing {
            review: plan.review.clone(),
            source,
        };
        let key = Key::load(&self.root.join(KEY_PATH)).map_err(refused)?;
        let manifest = frozen.sign(&key, plan.version, motion).map_err(refused)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&plan.signed)?
            .write_all(&manifest)?;
        timing.sign_offer = Some(signing.elapsed().as_secs_f64());

        Ok(Staged {
            build: frozen.build_id(),
            target,
            version: plan.version,
            review: plan.review,
            manifest: signed_arg,
            image: image_arg,
        })
    }

    /// The newest complete review in private/verification whose identity is
    /// the one a build of these sources would embed (sources, variant and
    /// update key all feed it), root-relative; `None` means build.
    fn cached_review(&mut self, args: &OtaArgs) -> Result<Option<PathBuf>> {
        if args.rebuild {
            return Ok(None);
        }
        let mut query = vec!["source-id".to_string(), "--image".into()];
        query.extend(args.image().args());
        let answer = self.tools.build_query(&query)?;
        let identity = answer.stdout.trim();
        if answer.code != 0 || !is_build_id(identity) {
            say!("Could not compute the source identity; building");
            return Ok(None);
        }

        let newest = fs::read_dir(self.root.join(VERIFICATION))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| {
                dir.file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with("-review"))
                    && ["image.bin", "review.json"]
                        .iter()
                        .all(|file| dir.join(file).is_file())
            })
            .filter(|dir| {
                wellfed_formats::load::<BuildManifest>(&dir.join("build-manifest.json"))
                    .is_ok_and(|manifest| manifest.startup_identity.as_deref() == Some(identity))
            })
            .max_by_key(|dir| dir.metadata().and_then(|m| m.modified()).ok());
        Ok(newest.map(|dir| PathBuf::from(VERIFICATION).join(dir.file_name().unwrap_or_default())))
    }

    /// Build the firmware and freeze its review; false if either failed.
    fn ota_build(&mut self, args: &OtaArgs, review: &Path, timing: &mut Timing) -> Result<bool> {
        let mut build = ["build", "--profile", "firstboot"]
            .map(String::from)
            .to_vec();
        build.extend(args.image().args());
        timing.ran = true;
        let (code, seconds) = self.run_step(BUILD_STEPS[0], &build)?;
        timing.build = Some(seconds);
        self.note(&format!("Build took {}", duration(seconds)));
        if code != 0 {
            say!("Build failed (exit {code}); nothing was reviewed or offered.");
            return Ok(false);
        }

        let freeze = [
            "review".to_string(),
            "build".into(),
            "--build".into(),
            BUILD_OUTPUT.into(),
            "--output".into(),
            review.display().to_string(),
        ];
        let (code, seconds) = self.run_step(BUILD_STEPS[1], &freeze)?;
        timing.review = Some(seconds);
        self.note(&format!("Review took {}", duration(seconds)));
        if code != 0 {
            say!("Review failed (exit {code}); nothing was offered.");
            return Ok(false);
        }
        Ok(true)
    }

    /// Receive-only capture segments, classified as each one ends; Ok once
    /// the new build confirmed.
    fn ota_watch(&mut self, args: &OtaArgs, staged: &Staged, timing: &mut Timing) -> Result<()> {
        let (build, version) = (staged.build.as_str(), staged.version);
        let mut watch = Watch::of(build, staged.target);
        let total = args.minutes * 60.0;
        let mut elapsed = 0.0;
        let mut stage = 0;
        // Stream bytes before the current segment.
        let mut base = 0u64;
        self.status.step(STAGES[0]);

        let mut n = 0;
        while elapsed < total {
            n += 1;
            let output = self.segment(&args.name, n);
            let seconds = (total - elapsed).min(SEGMENT_SECONDS);
            self.fresh(&output)?;

            let mut observer = self.start_observer(&output, seconds)?;
            let finished = self.finish_observer(observer.as_mut());
            if crate::interrupted() {
                return Err(Error::Interrupted);
            }
            finished?;
            elapsed += seconds;
            let ended = crate::now();

            let rx = fs::read(output.join("serial-rx.bin")).unwrap_or_default();
            let chunks = fs::read_to_string(output.join("serial-events.jsonl")).unwrap_or_default();
            for (offset, event) in watch.feed_at(&rx) {
                let stamp = match arrival(&chunks, offset.saturating_sub(base)) {
                    Some(at) => Stamp { at, exact: true },
                    None => Stamp {
                        at: ended,
                        exact: false,
                    },
                };
                match event {
                    Event::GiveWay { .. } => _ = timing.give_way.get_or_insert(stamp),
                    Event::Activated { .. } => _ = timing.activated.get_or_insert(stamp),
                    Event::Confirmed { .. } | Event::StockStarted { .. } => {
                        timing.confirmed = Some(stamp)
                    }
                    _ => {}
                }
                self.ota_event(&event, &mut stage);
            }
            base += rx.len() as u64;

            match watch.outcome() {
                None => {}
                Some(Outcome::Passed) => {
                    let what = match staged.target {
                        Target::Replacement => "confirmed",
                        Target::Stock => "stock, running",
                    };
                    say!(
                        "Passed: version {version} ({}) {what} on bank 1; captures {}-01…{n:02}",
                        &build[..12],
                        args.name
                    );
                    return Ok(());
                }
                Some(Outcome::FellBack { boot }) => {
                    return Err(Error::Bench(format!(
                        "Fell back: KM0 rejected the trial after boot {}; bank 0's recovery role runs. Captures {}-01…{n:02}",
                        boot.map_or_else(|| "?".into(), |b| b.to_string()),
                        args.name
                    )));
                }
                Some(Outcome::Failed { stage }) => {
                    return Err(Error::Bench(format!(
                        "Update failed at stage {stage}; bank 1 was not activated. Captures {}-01…{n:02}",
                        args.name
                    )));
                }
                Some(Outcome::Skipped) => {
                    return Err(Error::Bench(format!(
                        "An update attempt reset and the recovery role will not retry until a cold boot; look for a fault before the skip. Captures {}-01…{n:02}",
                        args.name
                    )));
                }
            }
        }

        Err(Error::Bench(format!(
            "No outcome within {} min; the feeder is untouched. Captures {}-01…{n:02}",
            args.minutes, args.name
        )))
    }

    /// Poll the feeder's device API status until it shows the new build on
    /// bank 1, for up to [`HEALTH_SECONDS`].
    fn ota_health(&mut self, args: &OtaArgs, build: &str) -> Result<()> {
        self.status.step(HEALTH_STEP);
        say!(
            "+ device API status as {} (every {HEALTH_POLL_SECONDS} s, up to {HEALTH_SECONDS} s)",
            args.keys.display()
        );

        let mut waited = 0.0;
        let mut last = String::new();
        loop {
            let verdict = self
                .tools
                .device_status(&args.keys, &args.config)
                .and_then(|status: DeviceStatus| Ok(status.matches(build)?));
            if crate::interrupted() {
                return Err(Error::Interrupted);
            }

            match verdict {
                Ok(report) => {
                    self.note(&report);
                    return Ok(());
                }
                // Only a changed reason is shown, so polling stays quiet.
                Err(reason) => {
                    let reason = reason.to_string();
                    if reason != last {
                        self.note(&format!("Waiting for status: {reason}"));
                        last = reason;
                    }
                }
            }

            if waited >= HEALTH_SECONDS {
                return Err(Error::Bench(format!(
                    "Confirmed on UART but the device API did not report the build within {HEALTH_SECONDS} s ({last})"
                )));
            }
            (self.sleep)(HEALTH_POLL_SECONDS)?;
            waited += HEALTH_POLL_SECONDS;
        }
    }

    /// Show one event and move the status steps forward, never back.
    fn ota_event(&mut self, event: &Event, stage: &mut usize) {
        let text = event.to_string();
        say!("{text}");
        self.status.line(&text);

        let next = match event {
            Event::Manifest { .. } | Event::Progress { .. } => 1,
            Event::Activated { .. } => 2,
            Event::Started { new: true, .. } | Event::StockStarted { .. } => 3,
            _ => return,
        };
        if next > *stage {
            *stage = next;
            self.status.step(STAGES[next]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";
    const OLD: &str = "5b8433a4f795cb2178c7a38a45087a6727e7a2621fc557d146cff607556f991e";

    fn start(build: &str, bank: u8) -> String {
        format!(
            "PLFW1 start build={build} bank={bank} reset=00100000 mmu=00000003,0c000000,03e7a000\r\n"
        )
    }

    /// From an October 22, 2026 hardware run, with SDK noise around it.
    fn passing_run() -> String {
        [
            "PLWCH1 offer=1 version=10\r\n".to_string(),
            "PLWCH1 give_way=1 version=10 restart=1\r\n".into(),
            "ROM:[V1.1]\r\n== Rtl8721D IMG2 ==\r\n".into(),
            "PLTRL1 bank=0\r\n".into(),
            start(OLD, 0),
            "[MQTT] connect ok\r\nPLOTA1 manifest version=10 bytes=671744\r\n".into(),
            "PLOTA1 progress sectors=16/164 leases=255\r\n".into(),
            "PLOTA1 activated version=10 bytes=671744 sectors=164\r\n".into(),
            "PLTRL1 confirmed=0 boot=1 limit=3\r\n".into(),
            "PLSUP1 open watchdog=1 trial=1\r\n".into(),
            start(NEW, 1),
            "PLCNF1 confirmed=1 healthy_ms=30000\r\n".into(),
            "PLWCH1 offer=1 same=1\r\n".into(),
        ]
        .concat()
    }

    #[test]
    fn a_confirmed_new_build_passes() {
        let mut watch = Watch::of(&NEW.to_uppercase(), Target::Replacement);
        let events = watch.feed(passing_run().as_bytes());

        assert_eq!(watch.outcome(), Some(&Outcome::Passed));
        assert_eq!(
            events,
            [
                Event::Offer {
                    version: Some(10),
                    same: false
                },
                Event::GiveWay { version: Some(10) },
                Event::Started {
                    build: OLD.into(),
                    bank: Some(0),
                    new: false
                },
                Event::Manifest {
                    version: Some(10),
                    bytes: Some(671_744)
                },
                Event::Progress {
                    done: 16,
                    total: 164
                },
                Event::Activated { version: Some(10) },
                Event::Trial {
                    boot: 1,
                    limit: Some(3)
                },
                Event::Started {
                    build: NEW.into(),
                    bank: Some(1),
                    new: true
                },
                Event::Confirmed {
                    detail: "healthy_ms=30000".into()
                },
            ]
        );
    }

    #[test]
    fn a_stock_install_passes_on_its_banner_after_activation() {
        let run = [
            "PLWCH1 offer=1 version=73\r\n",
            "PLWCH1 give_way=1 version=73 restart=1\r\n",
            &start(OLD, 0),
            "PLOTA1 manifest version=73 bytes=1376256\r\n",
            "PLOTA1 activated version=73 bytes=1376256 sectors=336\r\n",
            "ROM:[V1.1]\r\n[0]app_main:version:1.1.15 DATE:Jul 16 2026 TIME:10:00:00\r\n",
        ]
        .concat();
        let mut watch = Watch::of(NEW, Target::Stock);
        let events = watch.feed(run.as_bytes());

        assert_eq!(watch.outcome(), Some(&Outcome::Passed));
        assert_eq!(
            events.last(),
            Some(&Event::StockStarted {
                version: "1.1.15".into()
            })
        );
    }

    #[test]
    fn a_stock_banner_before_activation_is_not_a_pass() {
        let mut watch = Watch::of(NEW, Target::Stock);
        watch.feed(b"[0]app_main:version:1.1.15 DATE:Jul 16 2026\r\n");
        assert_eq!(watch.outcome(), None);

        // A replacement watch ignores the banner entirely.
        let mut replacement = Watch::of(NEW, Target::Replacement);
        replacement.feed(b"PLOTA1 activated version=1\r\n[0]app_main:version:1.1.15\r\n");
        assert_eq!(replacement.outcome(), None);
    }

    #[test]
    fn lines_split_across_segments_still_classify() {
        let run = passing_run();
        let mut watch = Watch::of(NEW, Target::Replacement);
        let mut events = Vec::new();
        for piece in run.as_bytes().chunks(7) {
            events.extend(watch.feed(piece));
        }

        assert_eq!(watch.outcome(), Some(&Outcome::Passed));
        assert!(events.contains(&Event::Progress {
            done: 16,
            total: 164
        }));
    }

    #[test]
    fn a_confirmation_before_the_new_build_starts_does_not_pass() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        watch.feed(format!("{}PLCNF1 confirmed=1 already=1\n", start(OLD, 1)).as_bytes());
        assert_eq!(watch.outcome(), None);

        // The new build on bank 0 is not the trial either.
        watch.feed(format!("{}PLCNF1 confirmed=1 already=1\n", start(NEW, 0)).as_bytes());
        assert_eq!(watch.outcome(), None);

        let events =
            watch.feed(format!("{}PLCNF1 confirmed=1 already=1\n", start(NEW, 1)).as_bytes());
        assert_eq!(watch.outcome(), Some(&Outcome::Passed));
        assert_eq!(
            events.last(),
            Some(&Event::Confirmed {
                detail: "already confirmed".into()
            })
        );
    }

    #[test]
    fn a_rejected_trial_falls_back_with_its_boot_count() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        let log = [
            "PLOTA1 activated version=11 bytes=671744 sectors=164\r\n".to_string(),
            "PLTRL1 confirmed=0 boot=1 limit=3\r\n".into(),
            start(NEW, 1),
            "PLTRL1 confirmed=0 boot=3 limit=3\r\n".into(),
            start(NEW, 1),
            "PLFW1 result diagnostic=fail backend=unopened\r\n".into(),
            "PLSUP1 stop startup=2 trial=1 confirmed=0 alive=1 outputs=1 restart=0 ms=253\r\n"
                .into(),
            "PLTRL1 give_way=1 marker=00 reset=1\r\n".into(),
            // After the outcome, nothing more counts.
            "PLOTA1 error stage=hash\r\n".into(),
        ]
        .concat();

        let events = watch.feed(log.as_bytes());
        assert_eq!(watch.outcome(), Some(&Outcome::FellBack { boot: Some(3) }));
        assert_eq!(events.last(), Some(&Event::FellBack { boot: Some(3) }));
    }

    #[test]
    fn a_give_way_before_activation_is_not_a_fallback() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        let events =
            watch.feed(b"PLTRL1 confirmed=0 boot=3 limit=3\nPLTRL1 give_way=1 marker=00 reset=1\n");

        assert_eq!(watch.outcome(), None);
        assert_eq!(events.last(), Some(&Event::EarlyGiveWay));
    }

    #[test]
    fn a_skipped_attempt_ends_the_watch() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        let events = watch.feed(b"PLOTA1 ready reply=0 inactive=1\r\nPLOTA1 skip marker=0x100\r\n");

        assert_eq!(watch.outcome(), Some(&Outcome::Skipped));
        assert_eq!(events, [Event::Skipped]);
    }

    #[test]
    fn an_update_error_reports_its_stage() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        let events = watch.feed(b"noise PLOTA1 error stage=hash\r\nPLOTA1 no_offer\r\n");

        assert_eq!(
            watch.outcome(),
            Some(&Outcome::Failed {
                stage: "hash".into()
            })
        );
        assert_eq!(
            events,
            [Event::Failed {
                stage: "hash".into()
            }]
        );
    }

    #[test]
    fn events_carry_the_stream_offset_of_their_line_end() {
        let mut watch = Watch::of(NEW, Target::Replacement);
        let first = watch.feed_at(b"noise\nPLWCH1 give_way=1 version=2\nPLOTA1 act");
        assert_eq!(first, [(33, Event::GiveWay { version: Some(2) })]);

        let second = watch.feed_at(b"ivated version=2\n");
        assert_eq!(second, [(60, Event::Activated { version: Some(2) })]);
    }

    #[test]
    fn arrival_reads_the_chunk_holding_an_offset() {
        let chunks = concat!(
            r#"{"utc":"2026-10-22T12:00:00+00:00","direction":"rx","offset":0,"bytes":10}"#,
            "\n",
            r#"{"utc":"2026-10-22T12:00:01.5+00:00","direction":"tx","offset":0,"bytes":99}"#,
            "\n",
            r#"{"utc":"2026-10-22T12:00:02.250000+00:00","direction":"rx","offset":10,"bytes":5}"#,
        );
        let noon = arrival(chunks, 0).unwrap();
        assert_eq!(arrival(chunks, 9), Some(noon));
        assert_eq!(arrival(chunks, 14), Some(noon + 2.25));
        assert_eq!(arrival(chunks, 15), None);
        assert_eq!(arrival("", 0), None);
    }

    #[test]
    fn durations_read_in_seconds_or_minutes() {
        assert_eq!(duration(8.24), "8.2 s");
        assert_eq!(duration(247.0), "4 min 07 s");
    }

    #[test]
    fn the_summary_marks_segment_timed_phases_and_unreached_ones() {
        let stamp = |at, exact| Some(Stamp { at, exact });
        let timing = Timing {
            ran: true,
            sign_offer: Some(1.0),
            give_way: stamp(100.0, true),
            activated: stamp(160.0, false),
            total: 200.0,
            ..Timing::default()
        };

        let lines = timing.summary(Source::Review);
        assert_eq!(lines[0], "Timing:");
        assert!(lines[1].ends_with("skipped (--review)"));
        assert!(lines[4].ends_with("1 min 00 s (to segment ends, up to 60 s off)"));
        assert!(lines[5].ends_with(" -") && lines[6].ends_with(" -"));
        assert!(lines[7].ends_with("3 min 20 s"));
    }

    #[test]
    fn manifest_headers_give_their_version() {
        let mut bytes = vec![0u8; MANIFEST_LEN];
        bytes[..8].copy_from_slice(MANIFEST_MAGIC);
        bytes[8..12].copy_from_slice(&13u32.to_le_bytes());
        assert_eq!(manifest_version(&bytes), Some(13));

        assert_eq!(manifest_version(&bytes[..100]), None);
        bytes[0] = b'X';
        assert_eq!(manifest_version(&bytes), None);
    }
}
