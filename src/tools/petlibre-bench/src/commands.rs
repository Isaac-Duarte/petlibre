//! Bench sequences. Each one either completes, or stops without retry and
//! says what state it left the feeder in.
//!
//! `main` dispatches each subcommand to a [`Bench`] method. Bench only
//! switches relays and orders steps; serial reads, inspections, plans and
//! writes go through [`Tools`] as separate `petlibre-flash` runs (and the
//! over-the-air loop's build and review as `petlibre-build` runs, its
//! signing, offer and health check as `petlibre-local` runs). Evidence
//! directories are never reused, and an execute refuses once its output
//! exists, so each prepared plan runs at most once.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use clap::{Args, ValueEnum};
use petlibre_formats::canonical::sha256;
use petlibre_formats::flash_plan::{DevelopmentPlan, GiveWayPlan, StockRecoveryReviewed};
use petlibre_formats::manifest::BuildManifest;
use petlibre_formats::transients::{self, TransientCycle, TransientsManifest};
use serde::Deserialize;

use crate::config::{CAPTURES, Config, FLASH_BACKUPS, VERIFICATION};
use crate::relay::{POWER, Relay, SPARES, STRAP};
use crate::status::{Status, relative};
use crate::tools::{Analyzer, Observation, Observer, Tools};
use crate::{Error, Result, now, require};

pub mod itest;
pub mod ota;
pub use itest::{ItestArgs, StockOtaArgs};
pub use ota::{Firmware, Image, OtaArgs, Variant};

const STRAP_SECONDS: f64 = 2.0;
const LOADER_SECONDS: f64 = 5.0;
/// A ROM loader answers the strap with a run of NAKs at least this long.
const LOADER_NAK_RUN: u64 = 8;

/// Observed per staged frame, framing included; for progress only.
const FRAME_TX_BYTES: u64 = 1056;
/// Logic analyzer (sigrok fx2lafw, 8 channels). Capture starts this long
/// before power-on so the unpowered state is recorded first; the first open
/// also uploads the analyzer's firmware, which takes about a second.
const ANALYZER_ARMING_SECONDS: f64 = 3.0;
const ANALYZER_DEFAULT_CHANNELS: [(&str, u8); 4] =
    [("ina", 0), ("inb", 1), ("outa", 2), ("outb", 3)];

const STOPPED_POWERED: &str =
    "Stopped without retry; the feeder stays powered. Inspect the evidence.";

#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    /// Session name for private evidence paths.
    pub name: String,
    /// Frozen review of the installed image (bank 0 may omit it to overwrite its baseline).
    #[arg(long)]
    pub source: Option<PathBuf>,
    /// Bank to update; 0 is the factory loader's fallback.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub bank: u8,
    /// Bank 0 without --source: a full flash read pinning bank 0's current bytes.
    #[arg(long)]
    pub original_read: Option<PathBuf>,
    /// Frozen review to install.
    #[arg(long)]
    pub target: PathBuf,

    /// Finish a stopped update from a fresh session.
    #[arg(long)]
    pub interrupted: bool,
    /// Restore bank 1 after a give-way (the source with its first marker byte cleared).
    #[arg(long)]
    pub restore_marker: bool,
    /// Allow a target whose manifest names a motion policy (owner present).
    #[arg(long)]
    pub attended_motion: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ExecuteArgs {
    #[command(flatten)]
    pub update: UpdateArgs,
    #[arg(long)]
    pub expected_plan_sha256: String,
    #[arg(long, default_value_t = 30.0)]
    pub boot_seconds: f64,
}

#[derive(Debug, Clone, Args)]
pub struct GiveWayArgs {
    /// Session name for private evidence paths.
    pub name: String,
    /// Frozen review of bank 0's image.
    #[arg(long)]
    pub fallback: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct GiveWayExecuteArgs {
    #[command(flatten)]
    pub give_way: GiveWayArgs,
    #[arg(long)]
    pub expected_plan_sha256: String,
    #[arg(long, default_value_t = 30.0)]
    pub boot_seconds: f64,
}

/// The original stock application back into bank 1 over UART.
#[derive(Debug, Clone, Args)]
pub struct StockArgs {
    /// Session name for private evidence paths.
    pub name: String,
    /// Frozen review of bank 0's image. While bank 1 holds a valid image,
    /// stock recovery refuses, so the session gives way to bank 0 first.
    #[arg(long)]
    pub fallback: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct StockExecuteArgs {
    #[command(flatten)]
    pub stock: StockArgs,
    #[arg(long)]
    pub expected_plan_sha256: String,
    /// Stock's banner arrives within seconds; its log is long.
    #[arg(long, default_value_t = 60.0)]
    pub boot_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TransientMode {
    Boot,
    Loader,
}

#[derive(Debug, Clone, Args)]
pub struct TransientArgs {
    /// Session name under private/captures/.
    pub name: String,
    #[arg(long, default_value_t = 10)]
    pub cycles: u32,
    #[arg(long, value_enum, default_value = "boot")]
    pub mode: TransientMode,
    /// Capture length per cycle, arming included.
    #[arg(long, default_value_t = 8.0)]
    pub seconds: f64,
    #[arg(long, default_value_t = 2_000_000)]
    pub samplerate: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Pattern {
    Chase,
    Blink,
    Heartbeat,
    Morse,
}

#[derive(Debug, Clone, Args)]
pub struct DemoArgs {
    /// Patterns to show; default all, in order.
    #[arg(value_enum)]
    pub patterns: Vec<Pattern>,
    /// Faster above 1 (rate-limited).
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,
    /// Morse message on relay 3.
    #[arg(long, default_value = "OK")]
    pub text: String,
}

/// Evidence locations for one session name.
pub(crate) struct Paths {
    pub loader: PathBuf,
    pub bounded: PathBuf,
    pub full: PathBuf,
    pub inspect: PathBuf,
    pub plan: PathBuf,
    pub execute: PathBuf,
    pub boot: PathBuf,
}

impl Paths {
    pub(crate) fn new(root: &Path, name: &str) -> Self {
        Self {
            loader: root.join(CAPTURES).join(format!("{name}-loader")),
            bounded: root.join(FLASH_BACKUPS).join(format!("{name}-bounded")),
            full: root.join(FLASH_BACKUPS).join(format!("{name}-full")),
            inspect: root.join(VERIFICATION).join(format!("{name}-inspect")),
            plan: root.join(VERIFICATION).join(format!("{name}-plan.json")),
            execute: root.join(VERIFICATION).join(format!("{name}-execute")),
            boot: root.join(CAPTURES).join(format!("{name}-boot")),
        }
    }
}

/// A session name is one plain path component.
pub fn check_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    require(
        matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none()
            && !name.contains(['/', '\\']),
        format!("Session name {name:?} must be a single plain name"),
    )
}

/// One command line: the subcommand words, then each group of flags.
fn argv(head: &[&str], parts: &[&[String]]) -> Vec<String> {
    head.iter()
        .map(|word| (*word).to_string())
        .chain(parts.iter().flat_map(|part| part.iter().cloned()))
        .collect()
}

pub struct Bench {
    pub root: PathBuf,
    pub config: Config,
    pub relay: Relay,
    pub status: Status,
    pub tools: Box<dyn Tools>,
    /// Interruptible in production; instant in tests.
    pub sleep: fn(f64) -> Result<()>,
    pub off_seconds: f64,
}

impl Bench {
    fn rel(&self, path: &Path) -> String {
        relative(&self.root, path)
    }

    pub(crate) fn set(&mut self, channel: u8, on: bool) -> Result<()> {
        self.relay.set(channel, on)?;
        self.status.relay(channel, on);
        Ok(())
    }

    pub(crate) fn get(&mut self, channel: u8) -> Result<bool> {
        let on = self.relay.get(channel)?;
        self.status.relay(channel, on);
        Ok(on)
    }

    /// Release the strap and cut power, with no wait: safe in cleanup paths.
    pub(crate) fn cut_power(&mut self) -> Result<()> {
        self.set(STRAP, false)?;
        self.set(POWER, false)
    }

    fn power_off(&mut self, seconds: f64) -> Result<()> {
        self.cut_power()?;
        (self.sleep)(seconds)
    }

    /// Captures never reuse a directory; refuse before any relay change.
    fn fresh(&self, output: &Path) -> Result<()> {
        require(
            !output.exists(),
            format!("Refusing to reuse {}", self.rel(output)),
        )
    }

    fn start_observer(&mut self, output: &Path, seconds: f64) -> Result<Box<dyn Observer>> {
        let observer = self
            .tools
            .observe(&self.config.uart_port, output, seconds)?;
        self.status.capture(Some(output), seconds);
        Ok(observer)
    }

    fn finish_observer(&mut self, observer: &mut dyn Observer) -> Result<Observation> {
        let result = observer.finish();
        self.status.capture(None, 0.0);
        result
    }

    /// Run one `petlibre-flash` command; its exit status. An interrupt during
    /// it stops the sequence even if the child finished, so no step follows.
    fn run(&mut self, args: &[String]) -> Result<i32> {
        say!("+ petlibre-flash {}", args.join(" "));
        self.status
            .step(&format!("petlibre-flash {} {}", args[0], args[1]));
        let code = self.tools.flash(args, &mut self.status)?;

        if crate::interrupted() {
            return Err(Error::Interrupted);
        }
        Ok(code)
    }

    fn run_all(&mut self, steps: &[Vec<String>]) -> Result<bool> {
        for step in steps {
            if self.run(step)? != 0 {
                say!("{STOPPED_POWERED}");
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn status_report(&mut self) -> Result<u8> {
        for channel in 1..=4 {
            let on = self.get(channel)?;
            say!("Relay {channel}: {}", if on { "on" } else { "off" });
        }

        for (key, port) in [
            ("relay_port", &self.config.relay_port),
            ("uart_port", &self.config.uart_port),
        ] {
            let present = Path::new(port).exists();
            say!("{key}: {}", if present { "present" } else { "missing" });
        }
        Ok(0)
    }

    pub fn on(&mut self) -> Result<u8> {
        self.set(STRAP, false)?;
        self.set(POWER, true)?;
        Ok(0)
    }

    pub fn off(&mut self) -> Result<u8> {
        self.cut_power()?;
        Ok(0)
    }

    /// Record the running feeder without touching power.
    pub fn capture(&mut self, name: &str, seconds: f64) -> Result<u8> {
        let output = self.root.join(CAPTURES).join(name);
        self.fresh(&output)?;
        let mut observer = self.start_observer(&output, seconds)?;
        let result = self.finish_observer(observer.as_mut())?;
        say!("Captured {} bytes in {}", result.bytes, self.rel(&output));
        Ok(0)
    }

    /// Cold boot into the installed application while recording it from the start.
    pub fn boot(&mut self, output: &Path, seconds: f64) -> Result<Observation> {
        self.status.step("boot capture");
        self.fresh(output)?;
        self.power_off(self.off_seconds)?;
        let mut observer = self.start_observer(output, seconds)?;
        if let Err(error) = self.set(POWER, true) {
            observer.kill();
            return Err(error);
        }

        let result = self.finish_observer(observer.as_mut())?;
        require(
            !result.download_banner_seen,
            "Boot reached the ROM loader, not the application",
        )?;

        say!(
            "Boot captured: {} bytes in {}; fault marker {}",
            result.bytes,
            self.rel(output),
            if result.fault_marker_seen {
                "seen"
            } else {
                "not seen"
            }
        );
        Ok(result)
    }

    /// Cold boot with the strap held; leave the feeder powered in the ROM loader.
    pub fn loader(&mut self, output: &Path) -> Result<Observation> {
        self.status.step("loader entry");
        self.fresh(output)?;
        self.power_off(self.off_seconds)?;
        let mut observer = self.start_observer(output, LOADER_SECONDS)?;

        // A failed entry kills the observer and tries to cut power rather
        // than leave the feeder strapped or half in the loader.
        let entered = (|| {
            self.set(STRAP, true)?;
            self.set(POWER, true)?;
            (self.sleep)(STRAP_SECONDS)?;
            self.set(STRAP, false)?;
            let result = self.finish_observer(observer.as_mut())?;
            require(
                result.download_banner_seen
                    && result.longest_nak_run >= LOADER_NAK_RUN
                    && !result.fault_marker_seen
                    && result.stock_startup_markers == 0,
                format!("Loader not confirmed; see {}", self.rel(output)),
            )?;
            Ok(result)
        })();

        match entered {
            Ok(result) => {
                say!(
                    "Loader confirmed: banner and {} NAKs in {}",
                    result.nak_bytes,
                    self.rel(output)
                );
                Ok(result)
            }

            Err(error) => {
                observer.kill();
                self.status.capture(None, 0.0);
                match self.cut_power() {
                    Ok(()) => eprintln!("Feeder powered off"),
                    Err(cut) => eprintln!("Could not power the feeder off: {cut}"),
                }
                Err(error)
            }
        }
    }

    /// Repeated captured power-ons (or loader entries) for the driver-input check.
    ///
    /// Each cycle: power off, start the analyzer, wait for it to arm, power on
    /// (holding the strap for a loader entry), let the capture finish, power off.
    /// Never opens the UART; never writes flash.
    pub fn transients(&mut self, args: &TransientArgs) -> Result<u8> {
        let out = self.root.join(CAPTURES).join(&args.name);
        self.fresh(&out)?;
        require(
            args.cycles >= 1 && args.seconds > ANALYZER_ARMING_SECONDS,
            "Need cycles >= 1 and a capture longer than arming",
        )?;
        require(
            self.tools.sigrok_available(),
            "sigrok-cli not found; install it before a transient session",
        )?;
        fs::create_dir_all(&out)?;

        let channels: BTreeMap<String, u8> =
            self.config.analyzer_channels.clone().unwrap_or_else(|| {
                ANALYZER_DEFAULT_CHANNELS
                    .iter()
                    .map(|(name, channel)| ((*name).to_string(), *channel))
                    .collect()
            });
        let mut manifest = TransientsManifest {
            schema_version: 1,
            mode: match args.mode {
                TransientMode::Boot => transients::TransientMode::Boot,
                TransientMode::Loader => transients::TransientMode::Loader,
            },
            samplerate: args.samplerate,
            seconds: args.seconds,
            arming_seconds: ANALYZER_ARMING_SECONDS,
            channels: channels.into_iter().map(|(k, v)| (k, v.into())).collect(),
            cycles: Vec::new(),
        };

        // Power off whether or not the cycles succeeded, then report the
        // cycles' failure ahead of the power-off's.
        let session = self.transient_cycles(args, &out, &mut manifest);
        let cut = self.cut_power();
        session?;
        cut?;

        say!(
            "Transient captures in {}; audit them with petlibre-audit transients",
            self.rel(&out)
        );
        Ok(0)
    }

    fn transient_cycles(
        &mut self,
        args: &TransientArgs,
        out: &Path,
        manifest: &mut TransientsManifest,
    ) -> Result<()> {
        for n in 1..=args.cycles {
            self.status.step(&format!("cycle {n}/{}", args.cycles));
            self.power_off(self.off_seconds)?;
            let name = format!("cycle-{n:03}.bin");
            let started = now();

            let mut analyzer =
                self.tools
                    .analyzer(&out.join(&name), args.seconds, args.samplerate)?;
            let cycle = self.transient_cycle(args, analyzer.as_mut());
            if cycle.is_err() {
                analyzer.kill();
            }
            let powered = cycle?;

            // Rewritten after every cycle, so a stopped session keeps the
            // manifest of the cycles it finished.
            let bytes = fs::metadata(out.join(&name)).map_or(0, |m| m.len());
            manifest.cycles.push(TransientCycle {
                file: name,
                bytes,
                power_on_after_launch_s: ((powered - started) * 1000.0).round() / 1000.0,
            });
            fs::write(
                out.join("manifest.json"),
                serde_json::to_string_pretty(manifest)?,
            )?;
            say!("Cycle {n}/{}: {bytes} bytes", args.cycles);
        }
        Ok(())
    }

    /// One analyzer capture across a power-on; when power came on.
    fn transient_cycle(
        &mut self,
        args: &TransientArgs,
        analyzer: &mut dyn Analyzer,
    ) -> Result<f64> {
        (self.sleep)(ANALYZER_ARMING_SECONDS)?;
        if !analyzer.running() {
            return Err(Error::Bench(format!(
                "Analyzer exited before power-on: {:?}",
                analyzer.stderr()
            )));
        }

        let powered = now();
        if args.mode == TransientMode::Loader {
            self.set(STRAP, true)?;
            self.set(POWER, true)?;
            (self.sleep)(STRAP_SECONDS)?;
            self.set(STRAP, false)?;
        } else {
            self.set(POWER, true)?;
        }

        let code = analyzer.wait(Duration::from_secs_f64(args.seconds + 30.0))?;
        let Some(code) = code else {
            return Err(Error::Bench(
                "Analyzer did not finish its bounded capture".into(),
            ));
        };
        self.power_off(0.0)?;
        require(
            code == 0,
            format!("Analyzer failed: {:?}", analyzer.stderr()),
        )?;
        Ok(powered)
    }

    /// Loader, bounded read, then a full 8 MiB read in the same helper session.
    pub fn dump(&mut self, name: &str) -> Result<u8> {
        let p = Paths::new(&self.root, name);
        for path in [&p.bounded, &p.full] {
            self.fresh(path)?;
        }
        self.loader(&p.loader)?;

        let read = self.read_args();
        let steps = [
            argv(&["read", "small"], &[&read, &self.out(&p.bounded)]),
            argv(
                &["read", "full"],
                &[
                    &read,
                    &self.out(&p.full),
                    &["--small-report".into(), self.report(&p.bounded)],
                ],
            ),
        ];
        if !self.run_all(&steps)? {
            return Ok(1);
        }

        say!(
            "Full read in {}; compare it before relying on it",
            self.rel(&p.full)
        );
        Ok(0)
    }

    fn path_arg(&self, path: &Path) -> String {
        path.display().to_string()
    }

    fn out(&self, path: &Path) -> Vec<String> {
        vec!["--output".into(), self.path_arg(path)]
    }

    fn report(&self, directory: &Path) -> String {
        self.path_arg(&directory.join("report.json"))
    }

    fn read_args(&self) -> Vec<String> {
        vec![
            "--port".into(),
            self.config.uart_port.clone(),
            "--rom-report".into(),
            self.path_arg(&self.root.join(&self.config.rom_report)),
        ]
    }

    fn evidence_args(&self, p: &Paths) -> Vec<String> {
        vec![
            "--baseline".into(),
            self.path_arg(&self.root.join(&self.config.baseline)),
            "--rom-report".into(),
            self.path_arg(&self.root.join(&self.config.rom_report)),
            "--small-report".into(),
            self.report(&p.bounded),
            "--port".into(),
            self.config.uart_port.clone(),
        ]
    }

    fn update_args(&self, args: &UpdateArgs, p: &Paths) -> Result<Vec<String>> {
        require(
            args.source.is_some() || args.bank == 0,
            "--source is required except when overwriting bank 0",
        )?;

        let mut out = self.evidence_args(p);
        out.extend([
            "--target".into(),
            self.path_arg(&args.target),
            "--bank".into(),
            args.bank.to_string(),
        ]);
        if let Some(source) = &args.source {
            out.extend(["--source".into(), self.path_arg(source)]);
        } else if let Some(read) = &args.original_read {
            out.extend(["--original-read".into(), self.path_arg(read)]);
        }
        if args.restore_marker {
            out.push("--restore-marker".into());
        }
        if args.interrupted {
            out.push("--interrupted".into());
        }
        Ok(out)
    }

    fn give_way_args(&self, args: &GiveWayArgs, p: &Paths) -> Vec<String> {
        let mut out = self.evidence_args(p);
        out.extend(["--fallback".into(), self.path_arg(&args.fallback)]);
        out
    }

    /// A target whose manifest names a motion policy drives the motor after
    /// boot. A target without a manifest (stock, diagnostics) has no policy;
    /// one whose manifest cannot be read is refused rather than assumed safe.
    fn require_attended_motion(&self, args: &UpdateArgs) -> Result<()> {
        let path = args.target.join("build-manifest.json");
        if !path.exists() {
            return Ok(());
        }

        let manifest: BuildManifest = petlibre_formats::load(&path).map_err(|error| {
            Error::Bench(format!(
                "Cannot read {} to check its motion policy: {error}",
                self.rel(&path)
            ))
        })?;
        self.attended_motion(&manifest, args.attended_motion)
    }

    /// Refuse a build manifest naming a motion policy unless `allowed`.
    fn attended_motion(&self, manifest: &BuildManifest, allowed: bool) -> Result<()> {
        let Some(policy) = manifest
            .startup_motion_policy
            .as_deref()
            .filter(|p| !p.is_empty())
        else {
            return Ok(());
        };

        require(
            allowed,
            format!(
                "Target drives the motor ({policy}); rerun with --attended-motion only with the owner watching"
            ),
        )?;
        say!("Attended motion image ({policy}): the motor will move after boot");
        Ok(())
    }

    fn prepared(&self, p: &Paths) -> Result<(Vec<u8>, String)> {
        let raw = fs::read(&p.plan)?;
        let sha = sha256(&raw);
        Ok((raw, sha))
    }

    /// Loader, helper bounded read, inspection and plan; stop for plan review.
    pub fn flash_prepare(&mut self, args: &UpdateArgs) -> Result<u8> {
        let p = Paths::new(&self.root, &args.name);
        self.require_attended_motion(args)?;
        let update = self.update_args(args, &p)?;
        for path in [&p.loader, &p.bounded, &p.inspect, &p.plan] {
            self.fresh(path)?;
        }
        self.loader(&p.loader)?;

        let steps = [
            argv(
                &["read", "small"],
                &[&self.read_args(), &self.out(&p.bounded)],
            ),
            argv(&["update", "inspect"], &[&update, &self.out(&p.inspect)]),
            argv(
                &["update", "prepare"],
                &[
                    &update,
                    &["--inspection".into(), self.report(&p.inspect)],
                    &self.out(&p.plan),
                ],
            ),
        ];
        if !self.run_all(&steps)? {
            return Ok(1);
        }

        let (raw, sha) = self.prepared(&p)?;
        let Prepared {
            operation_plan: plan,
        } = serde_json::from_slice::<Prepared<DevelopmentPlan>>(&raw)?;
        say!(
            "Plan {}: bank {}, {} sectors staged, {} erases, target {}",
            self.rel(&p.plan),
            plan.bank,
            plan.ordered_sectors.len(),
            plan.erase_sectors,
            plan.target_ota_sha256,
        );

        let next = execute_line(args, &sha);
        say!("Review it, then: {next}");
        self.status.next(next);
        Ok(0)
    }

    /// Execute a reviewed plan in the still-powered helper session, then boot.
    pub fn flash_execute(&mut self, args: &ExecuteArgs) -> Result<u8> {
        let p = Paths::new(&self.root, &args.update.name);
        self.require_attended_motion(&args.update)?;
        let update = self.update_args(&args.update, &p)?;
        require(
            p.plan.is_file() && !p.execute.exists(),
            "Prepared plan required; execution is single-use",
        )?;
        self.require_unbroken_power()?;

        // Progress estimate only; the updater reads and checks the plan itself.
        if let Some(plan) = fs::read(&p.plan)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Prepared<DevelopmentPlan>>(&raw).ok())
        {
            let (tx, rx) = update_traffic(&plan.operation_plan);
            self.status.transfer(Some(&p.execute), tx, rx);
        }

        let command = argv(
            &["update", "execute"],
            &[
                &update,
                &[
                    "--inspection".into(),
                    self.report(&p.inspect),
                    "--plan".into(),
                    self.path_arg(&p.plan),
                    "--expected-plan-sha256".into(),
                    args.expected_plan_sha256.clone(),
                ],
                &self.out(&p.execute),
            ],
        );
        if self.run(&command)? != 0 {
            say!("Update stopped; the feeder stays powered and is not booted. No retry.");
            return Ok(1);
        }

        self.status.transfer(None, 0, 0);
        self.boot(&p.boot, args.boot_seconds)?;
        Ok(0)
    }

    /// Relay state shows the supply was not switched off; it cannot rule out a brownout.
    fn require_unbroken_power(&mut self) -> Result<()> {
        let powered = self.get(POWER)?;
        let strap = self.get(STRAP)?;
        require(
            powered && !strap,
            "Feeder power was cycled since prepare; start over",
        )
    }

    /// Loader, bounded read, give-way inspection and plan; stop for review.
    pub fn give_way_prepare(&mut self, args: &GiveWayArgs) -> Result<u8> {
        let Some(sha) = self.give_way_plan(args)? else {
            return Ok(1);
        };

        let next = format!(
            "petlibre-bench give-way-execute {} --fallback {} --expected-plan-sha256 {sha}",
            args.name,
            args.fallback.display()
        );
        say!("Review it, then: {next}");
        self.status.next(next);
        Ok(0)
    }

    /// The give-way plan's SHA-256 once prepared; `None` if a step failed.
    fn give_way_plan(&mut self, args: &GiveWayArgs) -> Result<Option<String>> {
        let p = Paths::new(&self.root, &args.name);
        for path in [&p.loader, &p.bounded, &p.inspect, &p.plan] {
            self.fresh(path)?;
        }
        self.loader(&p.loader)?;

        let give_way = self.give_way_args(args, &p);
        let steps = [
            argv(
                &["read", "small"],
                &[&self.read_args(), &self.out(&p.bounded)],
            ),
            argv(
                &["give-way", "inspect"],
                &[&give_way, &self.out(&p.inspect)],
            ),
            argv(
                &["give-way", "prepare"],
                &[
                    &give_way,
                    &["--inspection".into(), self.report(&p.inspect)],
                    &self.out(&p.plan),
                ],
            ),
        ];
        if !self.run_all(&steps)? {
            return Ok(None);
        }

        let (raw, sha) = self.prepared(&p)?;
        let Prepared {
            operation_plan: plan,
        } = serde_json::from_slice::<Prepared<GiveWayPlan>>(&raw)?;
        say!(
            "Give-way plan {}: clears bank 1's first marker byte; bank 0 is {}",
            self.rel(&p.plan),
            plan.fallback_ota_sha256,
        );
        Ok(Some(sha))
    }

    /// Execute a reviewed give-way in the still-powered helper session, then boot.
    pub fn give_way_execute(&mut self, args: &GiveWayExecuteArgs) -> Result<u8> {
        let p = Paths::new(&self.root, &args.give_way.name);
        require(
            p.plan.is_file() && !p.execute.exists(),
            "Prepared plan required; execution is single-use",
        )?;
        self.require_unbroken_power()?;

        let command = argv(
            &["give-way", "execute"],
            &[
                &self.give_way_args(&args.give_way, &p),
                &[
                    "--inspection".into(),
                    self.report(&p.inspect),
                    "--plan".into(),
                    self.path_arg(&p.plan),
                    "--expected-plan-sha256".into(),
                    args.expected_plan_sha256.clone(),
                ],
                &self.out(&p.execute),
            ],
        );
        if self.run(&command)? != 0 {
            say!("Give-way stopped; the feeder stays powered and is not booted. No retry.");
            return Ok(1);
        }

        self.boot(&p.boot, args.boot_seconds)?;
        Ok(0)
    }

    /// The pinned original stock back into bank 1 over UART (the restore
    /// procedure), stopping for review at each plan. With `--fallback`, bank
    /// 1 still holds a valid image, so this prepares the give-way to that
    /// bank-0 review first; stock recovery refuses a valid target marker.
    /// Otherwise: loader, bounded and full reads, then the recovery plan.
    pub fn stock_prepare(&mut self, args: &StockArgs) -> Result<u8> {
        if let Some(fallback) = &args.fallback {
            let give_way = GiveWayArgs {
                name: give_way_name(&args.name),
                fallback: fallback.clone(),
            };
            let Some(sha) = self.give_way_plan(&give_way)? else {
                return Ok(1);
            };
            let next = format!(
                "petlibre-bench stock-execute {} --fallback {} --expected-plan-sha256 {sha}",
                args.name,
                fallback.display()
            );
            say!("Review it, then: {next}");
            self.status.next(next);
            return Ok(0);
        }

        let p = Paths::new(&self.root, &args.name);
        for path in [&p.loader, &p.bounded, &p.full, &p.plan] {
            self.fresh(path)?;
        }
        self.loader(&p.loader)?;

        let read = self.read_args();
        let reads = [
            argv(&["read", "small"], &[&read, &self.out(&p.bounded)]),
            argv(
                &["read", "full"],
                &[
                    &read,
                    &self.out(&p.full),
                    &["--small-report".into(), self.report(&p.bounded)],
                ],
            ),
        ];
        if !self.run_all(&reads)? {
            return Ok(1);
        }

        let current = p.full.join("flash-read.bin");
        require(
            !bank1_marker_valid(&current)?,
            format!(
                "Bank 1 holds a valid image, which stock recovery refuses; give way first: \
                 petlibre-bench stock-prepare {} --fallback <bank-0 review>",
                args.name
            ),
        )?;

        let recover = argv(
            &["stock-recovery", "prepare"],
            &[
                &self.evidence_args(&p),
                &["--current".into(), self.path_arg(&current)],
                &self.out(&p.plan),
            ],
        );
        if self.run(&recover)? != 0 {
            return Ok(1);
        }

        let (raw, sha) = self.prepared(&p)?;
        let reviewed: StockRecoveryReviewed = serde_json::from_slice(&raw)?;
        let plan = reviewed.recovery_plan;
        say!(
            "Stock plan {}: the pinned original into bank {}, {} sectors changed",
            self.rel(&p.plan),
            plan.target_bank,
            plan.changed_sector_count,
        );

        let next = format!(
            "petlibre-bench stock-execute {} --expected-plan-sha256 {sha}",
            args.name
        );
        say!("Review it, then: {next} (about an hour: three 8 MiB reads)");
        self.status.next(next);
        Ok(0)
    }

    /// Execute what `stock-prepare` planned. With `--fallback`, the give-way,
    /// then boot bank 0 and point at the recovery plan. Otherwise the stock
    /// recovery, single-use in the still-powered helper session, then a boot
    /// that must show stock's banner. The capture holds the unit's secrets.
    pub fn stock_execute(&mut self, args: &StockExecuteArgs) -> Result<u8> {
        let name = &args.stock.name;
        if let Some(fallback) = &args.stock.fallback {
            let give_way = GiveWayExecuteArgs {
                give_way: GiveWayArgs {
                    name: give_way_name(name),
                    fallback: fallback.clone(),
                },
                expected_plan_sha256: args.expected_plan_sha256.clone(),
                boot_seconds: args.boot_seconds,
            };
            let code = self.give_way_execute(&give_way)?;
            if code == 0 {
                let next = format!("petlibre-bench stock-prepare {name}");
                say!("Bank 0 runs; bank 1 can take stock now: {next}");
                self.status.next(next);
            }
            return Ok(code);
        }

        let p = Paths::new(&self.root, name);
        require(
            p.plan.is_file() && !p.execute.exists(),
            "Prepared plan required; execution is single-use",
        )?;
        self.require_unbroken_power()?;

        let command = argv(
            &["stock-recovery", "execute"],
            &[
                &self.evidence_args(&p),
                &[
                    "--current".into(),
                    self.path_arg(&p.full.join("flash-read.bin")),
                    "--plan".into(),
                    self.path_arg(&p.plan),
                    "--expected-plan-sha256".into(),
                    args.expected_plan_sha256.clone(),
                ],
                &self.out(&p.execute),
            ],
        );
        if self.run(&command)? != 0 {
            say!("Stock recovery stopped; the feeder stays powered and is not booted. No retry.");
            return Ok(1);
        }

        let seen = self.boot(&p.boot, args.boot_seconds)?;
        require(
            seen.stock_startup_markers > 0,
            "The boot capture shows no stock version banner",
        )?;
        say!(
            "Stock boots from bank 1. {} holds the unit's serial and cloud keys: keep it private.",
            self.rel(&p.boot)
        );
        Ok(0)
    }

    /// Click the spare relays; power and the strap are read, never set.
    pub fn demo(&mut self, args: &DemoArgs) -> Result<u8> {
        let power = self.get(POWER)?;
        let strap = self.get(STRAP)?;
        say!(
            "Feeder power {}, strap {}: left as is",
            if power { "on" } else { "off" },
            if strap { "held" } else { "released" }
        );

        let patterns = demo_patterns(args);
        let mut show = Show {
            bench: self,
            step: (0.25 / args.speed.max(f64::MIN_POSITIVE)).max(MIN_STEP),
            toggles: 0,
            on: [false; 2],
        };

        let shown = (|| {
            for pattern in &patterns {
                let label = pattern_name(*pattern);
                show.bench.status.step(label);
                say!("{label}...");
                match pattern {
                    Pattern::Chase => show.chase(4)?,
                    Pattern::Blink => show.blink(3)?,
                    Pattern::Heartbeat => show.heartbeat(3)?,
                    Pattern::Morse => show.morse(&args.text)?,
                }
            }
            Ok(())
        })();
        let toggles = show.toggles;

        // Both spares end off whatever happened; try each even if one fails.
        let mut failure = shown.err();
        for channel in SPARES {
            if let Err(error) = self.set(channel, false) {
                failure = Some(Error::Bench(format!(
                    "Spare {channel} may still be on: {error}"
                )));
            }
        }

        match failure {
            Some(error) => Err(error),
            None => {
                say!("Done: {toggles} toggles");
                Ok(0)
            }
        }
    }
}

/// The execute command matching a prepare: same evidence flags, plus the hash.
fn execute_line(args: &UpdateArgs, sha: &str) -> String {
    let mut line = format!(
        "petlibre-bench flash-execute {} --bank {}",
        args.name, args.bank
    );
    if let Some(source) = &args.source {
        line += &format!(" --source {}", source.display());
    } else if let Some(read) = &args.original_read {
        line += &format!(" --original-read {}", read.display());
    }

    line += &format!(" --target {}", args.target.display());
    for (set, flag) in [
        (args.restore_marker, " --restore-marker"),
        (args.interrupted, " --interrupted"),
        (args.attended_motion, " --attended-motion"),
    ] {
        if set {
            line += flag;
        }
    }
    line + &format!(" --expected-plan-sha256 {sha}")
}

/// The give-way session a stock return runs first, beside its own evidence.
fn give_way_name(name: &str) -> String {
    format!("{name}-giveway")
}

/// Whether a full read's bank 1 starts with the loader marker. Stock
/// recovery refuses a valid one, and only a full read shows it.
fn bank1_marker_valid(read: &Path) -> Result<bool> {
    let bytes = fs::read(read)?;
    let start = usize::try_from(petlibre_core::ota::BANK1_OFFSET)
        .map_err(|_| Error::Bench("Bank 1 offset out of range".into()))?;
    let marker = bytes
        .get(start..start + petlibre_core::ota::MARKER.len())
        .ok_or_else(|| Error::Bench(format!("{} is not a full read", read.display())))?;
    Ok(marker == petlibre_core::ota::MARKER)
}

/// Expected serial bytes (out, back) of a development update, for the
/// estimated bar. It mirrors the read sequence in petlibre-flash's
/// `development::run`, so a change there must change this too.
///
/// Each acquisition reads the factory region, the other bank's guard sector
/// and the application region. A plan with sectors to stage (or an interrupted
/// one) does three acquisitions, reads every staged sector three times
/// (preimage, erased, programmed) and the marker sector twice; otherwise only
/// the preimage acquisition runs.
fn update_traffic(plan: &DevelopmentPlan) -> (u64, u64) {
    const SECTOR: u64 = 0x1000;
    const ACQUIRED_GUARDS: u64 = 0x6000 + SECTOR;

    let packets = plan.staging_packets + plan.commit_packets;
    let sectors = plan.ordered_sectors.len() as u64;
    let acquisition = ACQUIRED_GUARDS + plan.region.bytes;

    let read = if sectors > 0 || plan.interrupted {
        3 * acquisition + (3 * sectors + 2) * SECTOR
    } else {
        acquisition
    };

    // Read packets carry 1024 payload bytes in 1028 on the wire.
    (packets * FRAME_TX_BYTES, read / 1024 * 1028)
}

/// The operation a prepared plan describes; the updater checks the rest.
#[derive(Deserialize)]
struct Prepared<P> {
    operation_plan: P,
}

/// Labels for the overall-progress steps of each command.
pub fn steps(command: &str) -> &'static [&'static str] {
    const READ_SMALL: &str = "petlibre-flash read small";
    match command {
        "boot" => &["boot capture"],
        "loader" => &["loader entry"],
        "dump" => &["loader entry", READ_SMALL, "petlibre-flash read full"],
        "flash-prepare" => &[
            "loader entry",
            READ_SMALL,
            "petlibre-flash update inspect",
            "petlibre-flash update prepare",
        ],
        "flash-execute" => &["petlibre-flash update execute", "boot capture"],
        "give-way-prepare" => &[
            "loader entry",
            READ_SMALL,
            "petlibre-flash give-way inspect",
            "petlibre-flash give-way prepare",
        ],
        "give-way-execute" => &["petlibre-flash give-way execute", "boot capture"],
        // With --fallback, the stock commands run the give-way steps instead.
        "stock-prepare" => &[
            "loader entry",
            READ_SMALL,
            "petlibre-flash read full",
            "petlibre-flash stock-recovery prepare",
        ],
        "stock-execute" => &["petlibre-flash stock-recovery execute", "boot capture"],
        "ota" => &[
            "petlibre-local ota-sign",
            "petlibre-local ota-offer",
            ota::STAGES[0],
            ota::STAGES[1],
            ota::STAGES[2],
            ota::STAGES[3],
            ota::HEALTH_STEP,
        ],
        _ => &[],
    }
}

/// Seconds between toggles; the board clicks audibly.
const MIN_STEP: f64 = 0.12;
/// Per run, to bound contact wear.
const MAX_TOGGLES: u32 = 120;

pub fn pattern_name(pattern: Pattern) -> &'static str {
    match pattern {
        Pattern::Chase => "chase",
        Pattern::Blink => "blink",
        Pattern::Heartbeat => "heartbeat",
        Pattern::Morse => "morse",
    }
}

pub fn demo_patterns(args: &DemoArgs) -> Vec<Pattern> {
    if args.patterns.is_empty() {
        vec![
            Pattern::Chase,
            Pattern::Blink,
            Pattern::Heartbeat,
            Pattern::Morse,
        ]
    } else {
        args.patterns.clone()
    }
}

fn morse(letter: char) -> &'static str {
    match letter {
        'A' => ".-",
        'B' => "-...",
        'C' => "-.-.",
        'D' => "-..",
        'E' => ".",
        'F' => "..-.",
        'G' => "--.",
        'H' => "....",
        'I' => "..",
        'J' => ".---",
        'K' => "-.-",
        'L' => ".-..",
        'M' => "--",
        'N' => "-.",
        'O' => "---",
        'P' => ".--.",
        'Q' => "--.-",
        'R' => ".-.",
        'S' => "...",
        'T' => "-",
        'U' => "..-",
        'V' => "...-",
        'W' => ".--",
        'X' => "-..-",
        'Y' => "-.--",
        'Z' => "--..",
        ' ' => " ",
        _ => "",
    }
}

struct Show<'a> {
    bench: &'a mut Bench,
    step: f64,
    toggles: u32,
    on: [bool; 2],
}

/// Which spare relays a step of a pattern holds on.
#[derive(Clone, Copy)]
enum Lit {
    Neither,
    Third,
    Fourth,
    Both,
}

impl Lit {
    /// Relay 3's state, then relay 4's.
    fn states(self) -> (bool, bool) {
        match self {
            Self::Neither => (false, false),
            Self::Third => (true, false),
            Self::Fourth => (false, true),
            Self::Both => (true, true),
        }
    }
}

impl Show<'_> {
    fn set(&mut self, channel: u8, on: bool, hold: f64) -> Result<()> {
        let index = SPARES
            .iter()
            .position(|spare| *spare == channel)
            .ok_or_else(|| {
                Error::Bench(format!(
                    "Relay {channel} is not a spare; the demo never switches it"
                ))
            })?;

        // Only actual changes count toward the cap; a repeat is just a hold.
        if self.on[index] != on {
            require(
                self.toggles < MAX_TOGGLES,
                format!("Toggle cap ({MAX_TOGGLES}) reached"),
            )?;
            self.bench.set(channel, on)?;
            self.on[index] = on;
            self.toggles += 1;
        }
        (self.bench.sleep)(self.step * hold)
    }

    /// Set both spares to `lit`, then wait `hold` steps.
    fn hold(&mut self, lit: Lit, hold: f64) -> Result<()> {
        let (third, fourth) = lit.states();
        self.set(3, third, 0.0)?;
        self.set(4, fourth, hold)
    }

    fn chase(&mut self, rounds: u32) -> Result<()> {
        for _ in 0..rounds {
            self.hold(Lit::Third, 1.0)?;
            self.hold(Lit::Fourth, 1.0)?;
        }
        self.hold(Lit::Neither, 1.0)
    }

    fn blink(&mut self, times: u32) -> Result<()> {
        for _ in 0..times {
            self.hold(Lit::Both, 2.0)?;
            self.hold(Lit::Neither, 2.0)?;
        }
        Ok(())
    }

    fn heartbeat(&mut self, beats: u32) -> Result<()> {
        for _ in 0..beats {
            self.hold(Lit::Both, 0.6)?;
            self.hold(Lit::Neither, 0.6)?;
            self.hold(Lit::Both, 0.6)?;
            self.hold(Lit::Neither, 4.0)?;
        }
        Ok(())
    }

    fn morse(&mut self, text: &str) -> Result<()> {
        for letter in text.to_uppercase().chars() {
            for symbol in morse(letter).chars() {
                if symbol == ' ' {
                    (self.bench.sleep)(self.step * 4.0)?;
                    continue;
                }
                self.set(3, true, if symbol == '.' { 1.0 } else { 3.0 })?;
                self.set(3, false, 1.0)?;
            }
            (self.bench.sleep)(self.step * 2.0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
