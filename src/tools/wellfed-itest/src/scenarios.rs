//! The scenarios, in suite order. Each one leaves the feeder as it found it:
//! the baseline build confirmed on bank 1, `serve` answering, the hotspot up,
//! settings and the Wi-Fi source as before. One that changes device state
//! restores it on failure too, as far as the feeder still answers.
//!
//! Updates are pushed over the device API; the failure drills spoil the
//! bench's own transfer and watch bank 0 stop it before activation.
//!
//! Every request goes over the device API as the owner's key; a write
//! names the store generation it read, so the feeder refuses it as stale if
//! anything changed in between.

use wellfed_model::{OpenRefusal, Refusal, Settings, SettingsPatch, WifiSource};

use std::ops::Deref;

use wellfed_formats::bench::catalog;
use wellfed_rig::commands::ota::Outcome;
use wellfed_rig::device::{Answer, Ask, Credentials, DeviceStatus, Intruder, Rebuffed};
use wellfed_rig::server::NotReady;
use wellfed_rig::{Error, Result, require};

use super::log::Record;
use super::{Baseline, Ctx, Fault, INSTALL_SECONDS, STATUS_SECONDS, compact};

/// One catalog entry and the code that runs it; the entry's fields read
/// through, as `scenario.name`.
pub(super) struct Scenario {
    info: catalog::Scenario,
    pub run: fn(&mut Ctx) -> Result<()>,
}

impl Deref for Scenario {
    type Target = catalog::Scenario;

    fn deref(&self) -> &catalog::Scenario {
        &self.info
    }
}

impl Scenario {
    /// The catalog's entry for `name`: a name the catalog lacks fails the
    /// build, so every scenario here is one the page and `--list` show.
    const fn new(name: &str, run: fn(&mut Ctx) -> Result<()>) -> Self {
        let mut i = 0;
        while i < catalog::SCENARIOS.len() {
            if same(catalog::SCENARIOS[i].name, name) {
                return Self {
                    info: catalog::SCENARIOS[i],
                    run,
                };
            }
            i += 1;
        }
        panic!("a scenario the catalog does not list");
    }
}

/// `a == b`, in a const context.
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The scenarios in catalog order; a test holds the two orders together.
pub(super) const SCENARIOS: [Scenario; 11] = [
    Scenario::new("ota-update", ota_update),
    Scenario::new("requests", requests),
    Scenario::new("request-auth", request_auth),
    Scenario::new("settings-persist", settings_persist),
    Scenario::new("wifi-fallback", wifi_fallback),
    Scenario::new("network-bounce", network_bounce),
    Scenario::new("feed-writes", super::feeds::feed_writes),
    Scenario::new("ota-recovery", ota_recovery),
    Scenario::new("ota-power-cuts", ota_power_cuts),
    Scenario::new("stock-roundtrip", super::stock::roundtrip),
    Scenario::new("soak", soak),
];

/// Stored by `wifi-fallback`: an SSID nothing broadcasts, so every join
/// fails whatever the passphrase. Neither is a real credential.
const ABSENT_SSID: &str = "wellfed-itest-absent";
const WRONG_PSK: &str = "itest-not-a-passphrase";

/// The NOR settings writer saves within a second or two of a change.
const SAVE_SECONDS: f64 = 30.0;
/// From power-on to the first startup record.
const BOOT_SECONDS: f64 = 60.0;
/// Five failed joins of the stored network, then the build's join.
const FALLBACK_SECONDS: f64 = 8.0 * 60.0;
/// From the offer to bank 1's give-way: its checks, then the answer's way out.
const OFFER_CHECK_SECONDS: f64 = 150.0;
/// How long a record may trail the answer it logs.
pub(super) const RECORD_SECONDS: f64 = 10.0;
/// The power cut lands once this many sectors are written: well inside the
/// image, past the first progress record.
const CUT_SECTORS: u32 = 48;
/// The updater logs progress every 16 sectors.
const PROGRESS_SECTORS: u32 = 16;
/// Into trial boot 1 before the cut; confirmation needs 30 s healthy.
const TRIAL_CUT_SECONDS: f64 = 10.0;
/// The Wi-Fi probe notices a lost access point within a beacon timeout or
/// two; the outage outlasts it.
const OUTAGE_SECONDS: f64 = 60.0;
/// The join retries back off to 30 s; joining takes a few more.
const REJOIN_SECONDS: f64 = 180.0;
/// The host and the feeder clocks agree this closely just after a sync.
const SKEW_MS: i64 = 5_000;
/// A full download (about 100 s) and the hash check, or a stall's retries.
const FAULT_SECONDS: f64 = 6.0 * 60.0;

fn ota_update(ctx: &mut Ctx) -> Result<()> {
    // A feeder the suite just powered on is not serving yet, and an offer of
    // its own build changes nothing (`same=1`); so hear from it first. One
    // that never answers is left to the install's own waits.
    let _ = ctx.reachable(STATUS_SECONDS);

    if let Some(review) = ctx.args.review.clone() {
        let build = ctx.rig.review_build(&review)?;
        if ctx.running(&build) {
            ctx.note(&format!(
                "  {} already runs on bank 1; nothing to install",
                &build[..12]
            ));
            ctx.baseline = Some(Baseline { build, review });
            return ctx.sync_clock();
        }
    }

    // Without a review, `ota_args` builds; only `--build` asks for that.
    require(
        ctx.args.build || ctx.args.review.is_some(),
        "Give --review DIR or --build",
    )?;
    let args = ctx.ota_args("good", ctx.args.review.clone());
    let staged = ctx
        .rig
        .stage(&args)?
        .ok_or_else(|| Error::Bench("Building, reviewing or signing the baseline failed".into()))?;

    if ctx.running(&staged.build) {
        ctx.note("  It already runs on bank 1; nothing to install");
    } else {
        let outcome = ctx.install(&staged)?;
        require(
            outcome == Outcome::Passed,
            format!("The baseline did not confirm: {outcome:?}"),
        )?;
        ctx.rig.offer(None, Fault::None)?;
    }

    ctx.baseline = Some(Baseline {
        build: staged.build,
        review: staged.review,
    });
    // No server sets the clock; the later scenarios want it valid.
    ctx.sync_clock()
}

fn requests(ctx: &mut Ctx) -> Result<()> {
    let build = ctx.baseline()?.build.clone();
    let since = ctx.mark()?;
    let status = ctx.device_status()?;
    ctx.note(&format!("  {}", status.matches(&build)?));
    ctx.expect("PLAPI1 request op=status", since, RECORD_SECONDS, |r| {
        r.tag == "PLAPI1" && r.first == "request" && r.field("op") == Some("status")
    })?;

    ctx.sync_clock()?;
    let status = ctx.device_status()?;
    ctx.note(&format!("  {}", clock_synced(&status, ctx.rig.unix_ms())?));

    let before = read_settings(ctx)?;
    let flipped = SettingsPatch {
        lights: Some(!before.lights),
        ..SettingsPatch::default()
    };
    set_settings(ctx, flipped)?;
    set_settings(ctx, changes_to(&before)).map(|_| ())
}

/// The feeder's clock is set and agrees with the host's `now_ms`.
fn clock_synced(status: &DeviceStatus, now_ms: i64) -> Result<String> {
    let clock = status.clock_ms.ok_or(NotReady::ClockUnset)?;
    let skew_ms = now_ms - i64::try_from(clock).unwrap_or(i64::MAX);
    if skew_ms.abs() > SKEW_MS {
        return Err(NotReady::Skew(skew_ms).into());
    }
    Ok(format!(
        "Clock: {skew_ms} ms behind this host after the sync"
    ))
}

pub(super) fn read_settings(ctx: &mut Ctx) -> Result<Settings> {
    let settings = match ctx.ask(&Ask::GetSettings)? {
        Answer::Settings(settings) => settings,
        other => {
            return Err(Error::Bench(format!(
                "The settings read answered {}",
                compact(&other)
            )));
        }
    };
    require(
        settings.writable,
        format!("Settings are not writable: {}", compact(&settings)),
    )?;
    Ok(settings)
}

/// The change that sets every setting back to `settings`.
pub(super) fn changes_to(settings: &Settings) -> SettingsPatch {
    SettingsPatch {
        lights: Some(settings.lights),
        sound: Some(settings.sound),
        volume: Some(settings.volume),
    }
}

/// Whether `settings` hold every field `change` sets.
fn shows(settings: &Settings, change: SettingsPatch) -> bool {
    change.lights.is_none_or(|on| settings.lights == on)
        && change.sound.is_none_or(|on| settings.sound == on)
        && change.volume.is_none_or(|volume| settings.volume == volume)
}

/// Change settings against the generation just read, see the writer save a
/// newer one, and read the change back; the generation it was made from.
pub(super) fn set_settings(ctx: &mut Ctx, change: SettingsPatch) -> Result<u32> {
    let base = read_settings(ctx)?.generation.0;
    let since = ctx.mark()?;

    let answer = ctx.ask(&Ask::PutSettings {
        base,
        changes: change,
    })?;
    require(
        matches!(answer, Answer::Done { .. }),
        format!("The settings change answered {}", compact(&answer)),
    )?;
    ctx.expect("PLCFG1 saved", since, SAVE_SECONDS, |r| {
        r.tag == "PLCFG1"
            && r.first == "saved"
            && r.number::<u32>("generation").is_some_and(|g| g > base)
    })?;

    let after = read_settings(ctx)?;
    require(
        shows(&after, change),
        format!(
            "Settings read back {} after setting {}",
            compact(&after),
            compact(&change)
        ),
    )?;
    Ok(base)
}

fn request_auth(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let unenrolled = ctx.rig.intrude(Intruder::UnenrolledKey)?;
    ctx.note(&format!("  unenrolled key: {unenrolled:?}"));
    require(
        unenrolled == Rebuffed::Refused(OpenRefusal::Unknown),
        format!("An unenrolled key was not refused as unknown: {unenrolled:?}"),
    )?;
    ctx.expect("PLAPI1 reason=unknown-key", since, RECORD_SECONDS, |r| {
        r.tag == "PLAPI1" && r.field("reason") == Some("unknown-key")
    })?;

    let since = ctx.mark()?;
    let tampered = ctx.rig.intrude(Intruder::Tampered)?;
    ctx.note(&format!("  tampered handshake: {tampered:?}"));
    require(
        tampered == Rebuffed::Dropped,
        format!("A tampered handshake was not dropped: {tampered:?}"),
    )?;
    ctx.expect("PLAPI1 reason=noise", since, RECORD_SECONDS, |r| {
        r.tag == "PLAPI1" && r.field("reason") == Some("noise")
    })?;

    // Neither cost the owner its way in.
    let build = ctx.baseline()?.build.clone();
    let status = ctx.device_status()?;
    status.matches(&build).map(|_| ()).map_err(Into::into)
}

/// Cut and restore the supply, then see the baseline start on bank 1 and
/// answer the device API.
fn cold_boot(ctx: &mut Ctx) -> Result<usize> {
    let build = ctx.baseline()?.build.clone();
    let since = ctx.mark()?;
    ctx.rig.power_cycle()?;

    let since = ctx.expect(
        "the baseline starting on bank 1",
        since,
        BOOT_SECONDS,
        |r| r.tag == "PLFW1" && r.first == "start" && started(r, &build),
    )?;
    // A session opened after the start record is the new boot's: the old
    // one's sockets died with the power.
    let report = ctx.until_device("status from the new boot", STATUS_SECONDS, |status| {
        Ok(status.matches(&build)?)
    })?;
    ctx.note(&format!("  {report}"));
    Ok(since)
}

fn started(record: &Record, build: &str) -> bool {
    record
        .field("build")
        .is_some_and(|b| b.eq_ignore_ascii_case(build))
        && record.field("bank") == Some("1")
}

fn settings_persist(ctx: &mut Ctx) -> Result<()> {
    let before = read_settings(ctx)?;
    let changed = SettingsPatch {
        lights: Some(!before.lights),
        sound: Some(!before.sound),
        volume: Some(if before.volume == 35 { 36 } else { 35 }),
    };

    let result = set_settings(ctx, changed).and_then(|base| {
        stale_write(ctx, base, changed)?;
        cold_boot(ctx)?;
        let after = read_settings(ctx)?;
        require(
            shows(&after, changed),
            format!("After a cold boot the settings read {}", compact(&after)),
        )
    });
    let restored = set_settings(ctx, changes_to(&before)).map(|_| ());
    result.and(restored)
}

/// A write against `base`, a generation the store has moved past, is
/// refused as stale and changes nothing.
fn stale_write(ctx: &mut Ctx, base: u32, change: SettingsPatch) -> Result<()> {
    let current = read_settings(ctx)?;
    let answer = ctx.ask(&Ask::PutSettings {
        base,
        changes: change,
    })?;
    ctx.note(&format!(
        "  A write from generation {base}: {}",
        compact(&answer)
    ));
    require(
        answer
            == Answer::Refused(Refusal::Stale {
                current: current.generation,
            }),
        format!(
            "A write from stale generation {base} was not refused as stale at {}: {}",
            current.generation,
            compact(&answer)
        ),
    )?;
    let after = read_settings(ctx)?;
    require(
        after == current,
        format!("The stale write changed the settings: {}", compact(&after)),
    )
}

fn wifi_source(status: &DeviceStatus, source: WifiSource) -> Result<String> {
    if status.wifi == source {
        Ok(format!("Wi-Fi source {source:?}"))
    } else {
        Err(NotReady::Source(status.wifi).into())
    }
}

/// Store `wifi` (or with `None` clear the store) against the generation the
/// status reports.
fn put_wifi(ctx: &mut Ctx, wifi: Option<Credentials>) -> Result<()> {
    let base = ctx.device_status()?.wifi_generation;
    let answer = ctx.ask(&Ask::PutWifi { base, wifi })?;
    require(
        matches!(answer, Answer::Done { .. }),
        format!("The Wi-Fi change answered {}", compact(&answer)),
    )
}

fn wifi_fallback(ctx: &mut Ctx) -> Result<()> {
    let status = ctx.device_status()?;
    require(
        status.wifi == WifiSource::Build,
        format!(
            "The feeder joined with {:?} credentials, not the build's; clear them first",
            status.wifi
        ),
    )?;

    let result = wrong_credentials(ctx);
    let cleared = clear_credentials(ctx);
    result.and(cleared)
}

fn wrong_credentials(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    put_wifi(
        ctx,
        Some(Credentials {
            ssid: ABSENT_SSID.into(),
            psk: WRONG_PSK.into(),
        }),
    )?;
    ctx.expect("PLCFG1 net saved stored=1", since, SAVE_SECONDS, |r| {
        r.is("PLCFG1", &[("stored", "1")]) && r.body_starts("net saved")
    })?;

    // Only the new boot can report the fallback: the stored network applies
    // from the next join.
    ctx.rig.power_cycle()?;
    let report = ctx.offline(|ctx| {
        ctx.until_device(
            "the build's credentials after fallback",
            FALLBACK_SECONDS,
            |s| wifi_source(s, WifiSource::Fallback),
        )
    })?;
    ctx.note(&format!("  {report}"));
    Ok(())
}

/// Clear the store whatever it holds (the device API cannot tell), then see
/// the build's credentials join again.
fn clear_credentials(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    put_wifi(ctx, None)?;
    ctx.expect("PLCFG1 net saved stored=0", since, SAVE_SECONDS, |r| {
        r.is("PLCFG1", &[("stored", "0")]) && r.body_starts("net saved")
    })?;
    cold_boot(ctx)?;
    let report = ctx.until_device("the build's credentials", STATUS_SECONDS, |s| {
        wifi_source(s, WifiSource::Build)
    })?;
    ctx.note(&format!("  {report}"));
    Ok(())
}

fn network_bounce(ctx: &mut Ctx) -> Result<()> {
    let before = ctx.reachable(STATUS_SECONDS)?;

    ctx.rig.hotspot(false)?;
    let outage = ctx.offline(|ctx| ctx.rig.wait(OUTAGE_SECONDS));
    let restored = ctx.rig.hotspot(true);
    outage?;
    restored?;

    let after = ctx.offline(|ctx| {
        ctx.until_device("the device API after the outage", REJOIN_SECONDS, |_| {
            Ok(String::new())
        })?;
        ctx.device_status()
    })?;
    require(
        after.boot_count == before.boot_count,
        format!(
            "The feeder restarted during the outage (boot {} then {})",
            before.boot_count, after.boot_count
        ),
    )?;
    require(
        after.losses > before.losses,
        format!(
            "Status reports {} link losses, {} before the outage",
            after.losses, before.losses
        ),
    )
}

/// One failing trial, then the fault drills on bank 0, which it falls back
/// to, so the fallback needs no restore install of its own: the clean
/// download at the end is the restore. Bank 0 keeps answering after each
/// stop, so the drills need no cold boot between them.
fn ota_recovery(ctx: &mut Ctx) -> Result<()> {
    let failing = ctx.failing("fail")?;
    // Signed after the variant, so it is above the floor.
    let staged = ctx.resign("restore")?;

    let result = recovery(ctx, &failing, &staged);
    if result.is_err() {
        ctx.note("  The feeder may be left on bank 0; pushing the baseline to it");
        let since = ctx.log.len();
        let _ = ctx.rig.offer(Some(&staged), Fault::None);
        let _ = ctx.watch(&staged.build, since, INSTALL_SECONDS);
    }
    ctx.rig.offer(None, Fault::None)?;
    ctx.confirm(&staged.build)?;
    result
}

fn recovery(ctx: &mut Ctx, failing: &super::Staged, staged: &super::Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(failing), Fault::None)?;
    ctx.expect("PLOTA1 activated", since, INSTALL_SECONDS, |r| {
        r.tag == "PLOTA1" && r.first == "activated"
    })?;
    let outcome = ctx.watch(&failing.build, since, INSTALL_SECONDS)?;
    require(
        matches!(outcome, Outcome::FellBack { .. }),
        format!("The failing build was not rejected: {outcome:?}"),
    )?;

    // Bank 0 runs now. Each drill pushes the restore with one fault, and
    // bank 0 must stop it before the marker.
    for fault in [Fault::Corrupt, Fault::Unwanted, Fault::Stall] {
        let Some(stage) = fault.stage() else { continue };
        let start = ctx.mark()?;
        ctx.rig.offer(Some(staged), fault)?;
        let what = format!("PLOTA1 error stage={stage}");
        ctx.expect(&what, start, FAULT_SECONDS, |r| {
            r.tag == "PLOTA1" && r.first == "error" && r.field("stage") == Some(stage)
        })?;
        let activated = ctx
            .log
            .count(start, |r| r.tag == "PLOTA1" && r.first == "activated");
        require(
            activated == 0,
            format!("The {stage} fault still activated bank 1"),
        )?;
    }

    let since = ctx.mark()?;
    ctx.rig.offer(Some(staged), Fault::None)?;
    let outcome = ctx.watch(&staged.build, since, INSTALL_SECONDS)?;
    require(
        outcome == Outcome::Passed,
        format!("The clean download did not confirm: {outcome:?}"),
    )
}

/// Sectors written so far, from `PLOTA1 progress sectors=N/TOTAL`.
fn sectors(record: &Record) -> Option<u32> {
    record.field("sectors")?.split('/').next()?.parse().ok()
}

/// The record a mark just passed (an [`Ctx::expect`] result).
fn record_before(ctx: &Ctx, mark: usize, field: &str) -> Option<String> {
    let line = &ctx.log.line(mark.checked_sub(1)?).text;
    Record::parse(line)?.field(field).map(str::to_string)
}

fn ota_power_cuts(ctx: &mut Ctx) -> Result<()> {
    let build = ctx.baseline()?.build.clone();
    // The feeder skips the trial for a build it has confirmed before, so the
    // cuts happen while it installs the feeder's twin (the same firmware
    // under another identity, which still serves the device API), which it
    // has not; the baseline goes back on afterwards, pushed to the twin.
    let twin = ctx.image(
        "cut-twin",
        wellfed_rig::commands::Image {
            firmware: wellfed_rig::commands::Firmware::Feeder,
            variant: Some(wellfed_rig::commands::Variant::Twin),
        },
    )?;
    let restore = ctx.resign("cut-restore")?;

    let result = power_cuts(ctx, &twin);
    let since = ctx.mark()?;
    if result.is_err() {
        ctx.note("  Cold-booting, whatever stage it stopped in, then pushing the baseline");
        ctx.rig.power_cycle()?;
        // A twin left recorded as pending would make bank 0 refuse the
        // baseline: forget it first. Best effort: a feeder still in the
        // twin's trial answers Busy, and its fallback clears nothing.
        let cancelled = ctx
            .reachable(BOOT_SECONDS)
            .and_then(|_| ctx.ask(&Ask::CancelUpdate));
        ctx.note(&format!("  Cancel before the restore: {cancelled:?}"));
    }
    ctx.rig.offer(Some(&restore), Fault::None)?;
    let outcome = ctx.watch(&build, since, INSTALL_SECONDS)?;
    ctx.rig.offer(None, Fault::None)?;
    ctx.confirm(&build)?;
    result?;
    require(
        outcome == Outcome::Passed,
        format!("The baseline did not come back: {outcome:?}"),
    )
}

fn power_cuts(ctx: &mut Ctx, staged: &super::Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(staged), Fault::None)?;
    let since = ctx.expect("PLWCH1 give_way=1", since, OFFER_CHECK_SECONDS, |r| {
        r.is("PLWCH1", &[("give_way", "1")])
    })?;

    // A cut mid-download: bank 1's marker is still clear, so the loader
    // must boot bank 0, which takes the offer again from the push (still
    // running) and downloads from the start.
    let since = ctx.expect("a download under way", since, INSTALL_SECONDS, |r| {
        r.tag == "PLOTA1" && r.first == "progress" && sectors(r).is_some_and(|n| n >= CUT_SECTORS)
    })?;
    ctx.rig.power_cycle()?;
    let since = ctx.expect("a boot after the cut", since, BOOT_SECONDS, |r| {
        r.tag == "PLFW1" && r.first == "start"
    })?;
    let bank = record_before(ctx, since, "bank");
    require(
        bank.as_deref() == Some("0"),
        format!("After a cut mid-download the loader booted bank {bank:?}, not 0"),
    )?;
    let since = ctx.expect("the download again", since, INSTALL_SECONDS, |r| {
        r.tag == "PLOTA1" && r.first == "progress"
    })?;
    let first = Record::parse(&ctx.log.line(since - 1).text).and_then(|r| sectors(&r));
    require(
        first.is_some_and(|n| n <= PROGRESS_SECTORS),
        format!("The download resumed at {first:?} sectors instead of starting again"),
    )?;

    // A cut early in trial boot 1: backup registers clear on power loss, so
    // KM0 counts boot 1 again, and the build still confirms.
    let since = ctx.expect("trial boot 1", since, INSTALL_SECONDS, |r| {
        r.is("PLTRL1", &[("confirmed", "0"), ("boot", "1")])
    })?;
    ctx.idle(TRIAL_CUT_SECONDS)?;
    let confirmed = |r: &Record| r.is("PLCNF1", &[("confirmed", "1")]);
    require(
        ctx.log.count(since, confirmed) == 0,
        "The build confirmed before the trial cut",
    )?;
    ctx.rig.power_cycle()?;
    let since = ctx.expect("the trial count after the cut", since, BOOT_SECONDS, |r| {
        r.tag == "PLTRL1" && r.field("boot").is_some()
    })?;
    let boot = record_before(ctx, since, "boot");
    require(
        boot.as_deref() == Some("1"),
        format!("After a cut in the trial KM0 counted boot {boot:?}, not 1 again"),
    )?;
    ctx.expect("PLCNF1 confirmed=1", since, STATUS_SECONDS, confirmed)?;
    ctx.confirm(&staged.build)
}

/// One status request per period.
const SOAK_PERIOD_SECONDS: f64 = 30.0;

fn soak(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let before = ctx.reachable(STATUS_SECONDS)?;
    let end = ctx.rig.clock() + ctx.args.soak_minutes * 60.0;

    let (mut asked, mut slowest) = (0u32, 0.0f64);
    while ctx.rig.clock() < end {
        let at = ctx.rig.clock();
        ctx.device_status()?;
        slowest = slowest.max(ctx.rig.clock() - at);
        asked += 1;
        ctx.idle(SOAK_PERIOD_SECONDS)?;
    }

    let restarts = ctx
        .log
        .count(since, |r| r.tag == "PLFW1" && r.first == "start");
    let after = ctx.device_status()?;
    let losses = after.losses.saturating_sub(before.losses);
    ctx.note(&format!(
        "  {asked} status requests, slowest answer {slowest:.1} s, {restarts} restarts, {losses} link losses"
    ));
    require(
        restarts == 0 && after.boot_count == before.boot_count,
        format!("The feeder restarted {restarts} times"),
    )?;
    require(
        losses == 0,
        format!(
            "The Wi-Fi link was lost ({} losses, then {})",
            before.losses, after.losses
        ),
    )
}

/// What every scenario must leave as it found it: the settings, the Wi-Fi
/// source, and the baseline running on bank 1. The device API does not say
/// whether credentials are stored until a boot uses them, so a store left
/// set shows only once it takes effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeviceState {
    settings: SettingsPatch,
    wifi: WifiSource,
    build: String,
    bank: u8,
}

pub(super) fn device_state(ctx: &mut Ctx) -> Result<DeviceState> {
    let settings = changes_to(&read_settings(ctx)?);
    let status = ctx.reachable(STATUS_SECONDS)?;

    Ok(DeviceState {
        settings,
        wifi: status.wifi,
        build: status.build,
        bank: status.bank,
    })
}

/// How `now` differs from `want`, after putting back what a request can:
/// settings at once, stored Wi-Fi credentials from the next boot.
pub(super) fn restore_state(ctx: &mut Ctx, want: &DeviceState, now: &DeviceState) -> Vec<String> {
    let mut changed = Vec::new();
    if now.settings != want.settings {
        changed.push(format!(
            "settings {} (were {})",
            compact(&now.settings),
            compact(&want.settings)
        ));
        if let Err(error) = set_settings(ctx, want.settings) {
            changed.push(format!("restoring them failed: {error}"));
        }
    }
    if now.wifi != want.wifi {
        changed.push(format!("Wi-Fi source {:?} (was {:?})", now.wifi, want.wifi));
        if want.wifi == WifiSource::Build
            && let Err(error) = put_wifi(ctx, None)
        {
            changed.push(format!("clearing the stored credentials failed: {error}"));
        }
    }
    if now.build != want.build || now.bank != want.bank {
        changed.push(format!(
            "build {} on bank {} (was {} on bank {})",
            short(&now.build),
            now.bank,
            short(&want.build),
            want.bank
        ));
    }
    changed
}

fn short(build: &str) -> &str {
    &build[..build.len().min(12)]
}
