//! The scenarios, in suite order. Each one leaves the feeder as it found it:
//! the baseline build confirmed on bank 1, `serve` answering, the hotspot up,
//! settings and stored Wi-Fi credentials as before. One that changes device
//! state restores it on failure too, as far as the feeder still answers.

use super::log::Record;
use super::{
    Baseline, Ctx, Fault, HEALTH_SECONDS, INSTALL_SECONDS, Need, Server, Tier, compact, reply_ok,
};
use crate::commands::ota::Outcome;
use crate::server::{LocalStatus, NotReady, Reply, Request, SettingsChange, WifiChange};
use crate::{Error, Result, require};

pub(super) struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    /// The cheapest tier that runs it (`--tier`).
    pub tier: Tier,
    /// Session flags it cannot run without; it is skipped, never passed.
    pub needs: &'static [Need],
    /// Installs the build under test; every other scenario needs it.
    pub baseline: bool,
    pub run: fn(&mut Ctx) -> Result<()>,
}

pub(super) const SCENARIOS: [Scenario; 9] = [
    Scenario {
        name: "ota-update",
        about: "install the build under test over the air and see it confirm",
        tier: Tier::Smoke,
        needs: &[],
        baseline: true,
        run: ota_update,
    },
    Scenario {
        name: "requests",
        about: "ping, health, clock sync, and a settings change read back and restored",
        tier: Tier::Smoke,
        needs: &[],
        baseline: false,
        run: requests,
    },
    Scenario {
        name: "request-auth",
        about: "unsigned, wrongly signed and replayed requests are refused",
        tier: Tier::Smoke,
        needs: &[],
        baseline: false,
        run: request_auth,
    },
    Scenario {
        name: "settings-persist",
        about: "changed settings survive a cold boot, then are restored",
        tier: Tier::Standard,
        needs: &[Need::Power],
        baseline: false,
        run: settings_persist,
    },
    Scenario {
        name: "wifi-fallback",
        about: "wrong stored Wi-Fi credentials fall back to the build's; a signed clear recovers",
        tier: Tier::Standard,
        needs: &[Need::Power],
        baseline: false,
        run: wifi_fallback,
    },
    Scenario {
        name: "network-bounce",
        about: "the hotspot goes down and up; the transport reconnects and health resumes",
        tier: Tier::Standard,
        needs: &[],
        baseline: false,
        run: network_bounce,
    },
    Scenario {
        name: "ota-recovery",
        about: "a failed trial gives way to bank 0, which stops corrupt, truncated and stalled downloads, then restores the baseline",
        tier: Tier::Standard,
        needs: &[Need::Power, Need::Build],
        baseline: false,
        run: ota_recovery,
    },
    Scenario {
        name: "ota-power-cuts",
        about: "power cut mid-download (bank 0 downloads again) and early in a trial (the count restarts)",
        tier: Tier::Standard,
        needs: &[Need::Power, Need::Build, Need::Destructive],
        baseline: false,
        run: ota_power_cuts,
    },
    Scenario {
        name: "soak",
        about: "pings for --soak-minutes with no restart, reconnect or missed reply",
        tier: Tier::Soak,
        needs: &[],
        baseline: false,
        run: soak,
    },
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
/// The confirmed build checks for an offer once a minute.
const OFFER_CHECK_SECONDS: f64 = 150.0;
/// How long a record may trail the reply it logs.
const RECORD_SECONDS: f64 = 10.0;
/// The power cut lands once this many sectors are written: well inside the
/// image, past the first progress record.
const CUT_SECTORS: u32 = 48;
/// The updater logs progress every 16 sectors.
const PROGRESS_SECTORS: u32 = 16;
/// Into trial boot 1 before the cut; confirmation needs 30 s healthy.
const TRIAL_CUT_SECONDS: f64 = 10.0;
/// The transport notices a dead link within a keepalive or two.
const LOSS_SECONDS: f64 = 120.0;
const OUTAGE_SECONDS: f64 = 20.0;
/// Reconnect backoff tops out at 30 s; joining takes a few more.
const RECONNECT_SECONDS: f64 = 180.0;
/// A full download (about 100 s) and the hash check, or a stall's retries.
const FAULT_SECONDS: f64 = 6.0 * 60.0;

fn ota_update(ctx: &mut Ctx) -> Result<()> {
    // A feeder the suite just powered on has not reported yet, and an offer
    // of its own build changes nothing (`same=1`); so hear from it first.
    // One that never reports is left to the install's own waits.
    let _ = ctx.health(HEALTH_SECONDS);

    if let Some(review) = ctx.args.review.clone() {
        let build = ctx.rig.review_build(&review)?;
        if ctx.running(&build) {
            ctx.note(&format!(
                "  {} already runs on bank 1; nothing to install",
                &build[..12]
            ));
            ctx.baseline = Some(Baseline { build, review });
            return Ok(());
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
        ctx.rig.offer(None)?;
    }

    ctx.baseline = Some(Baseline {
        build: staged.build,
        review: staged.review,
    });
    Ok(())
}

fn requests(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let ping = ctx.ask(&Request::Ping)?;
    reply_ok(&ping)?;
    require(
        ping.uptime_ms.is_some(),
        format!("Ping reply has no uptime_ms: {}", compact(&ping)),
    )?;
    ctx.expect("PLMQT1 request op=ping ok=1", since, 15.0, |r| {
        r.is("PLMQT1", &[("op", "ping"), ("ok", "1")])
    })?;

    reply_ok(&ctx.ask(&Request::Health)?)?;

    ctx.until_status("a synced firmware clock", HEALTH_SECONDS, clock_synced)
        .map(|report| ctx.note(&format!("  {report}")))?;

    let before = read_settings(ctx)?;
    let flipped = SettingsChange {
        lights: Some(before.lights != Some(1)),
        ..SettingsChange::default()
    };
    set_settings(ctx, &flipped)?;
    set_settings(ctx, &before.settings())
}

/// Health shows a synced clock that agrees with the server's receive time.
fn clock_synced(status: &LocalStatus) -> Result<String> {
    let health = status.fresh_health()?;
    let clock = health.clock.as_ref().ok_or(NotReady::NoClock)?;
    if clock.syncs < 1 {
        return Err(NotReady::Unsynced(compact(clock)).into());
    }
    let unix_ms = clock
        .unix_ms
        .ok_or_else(|| NotReady::ClockUnset(compact(clock)))?;

    let received = status
        .received_at()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .ok_or(NotReady::NoReceivedAt)?;
    let skew_ms = received.timestamp_millis() - unix_ms;
    if skew_ms.abs() > 5_000 {
        return Err(NotReady::Skew(skew_ms).into());
    }
    Ok(format!(
        "Clock: {} syncs, {skew_ms} ms behind the server at receipt",
        clock.syncs
    ))
}

fn read_settings(ctx: &mut Ctx) -> Result<Reply> {
    let settings = ctx.ask(&Request::Settings(SettingsChange::default()))?;
    reply_ok(&settings)?;
    require(
        settings.writable == Some(true),
        format!("Settings are not writable: {}", compact(&settings)),
    )?;
    Ok(settings)
}

/// Change settings, see the writer save a newer generation, and read the
/// change back.
fn set_settings(ctx: &mut Ctx, change: &SettingsChange) -> Result<()> {
    let generation = read_settings(ctx)?.generation.unwrap_or(0);
    let since = ctx.mark()?;

    reply_ok(&ctx.ask(&Request::Settings(change.clone()))?)?;
    ctx.expect("PLCFG1 saved", since, SAVE_SECONDS, |r| {
        r.tag == "PLCFG1"
            && r.first == "saved"
            && r.number::<u64>("generation")
                .is_some_and(|g| g > generation)
    })?;

    let after = read_settings(ctx)?;
    require(
        after.shows(change),
        format!(
            "Settings read back {} after setting {}",
            compact(&after),
            compact(change)
        ),
    )
}

fn request_auth(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let result = forged(ctx, since);
    restore_serve(ctx)?;
    result
}

fn forged(ctx: &mut Ctx, since: usize) -> Result<()> {
    ctx.rig.server(Server::Broker)?;
    // A new session on the bare broker carries a fresh nonce.
    let since = ctx.expect(
        "a reconnect to the bare broker",
        since,
        RECONNECT_SECONDS,
        |r| r.tag == "PLMQT1" && r.first == "ready",
    )?;

    let lines = ctx.rig.forge()?;
    let reply = |case: &str| {
        lines
            .iter()
            .find(|line| line.case == case)
            .and_then(|line| line.reply.clone())
    };
    for case in ["unsigned", "wrong_key", "replay"] {
        let answer = reply(case);
        ctx.note(&format!("  {case}: {}", compact(&answer)));
        require(
            answer
                .as_ref()
                .is_some_and(|a| !a.ok && a.error.as_deref() == Some("auth")),
            format!(
                "The {case} request was not refused for auth: {}",
                compact(&answer)
            ),
        )?;
    }
    let valid = reply("valid");
    ctx.note(&format!("  valid: {}", compact(&valid)));
    require(
        valid
            .as_ref()
            .is_some_and(|v| v.ok && v.op.as_deref() == Some("settings")),
        format!(
            "The validly signed request was not answered: {}",
            compact(&valid)
        ),
    )?;

    let refused = ctx.expect_count(since, 3, RECORD_SECONDS, |r| {
        r.is("PLMQT1", &[("op", "unauthenticated"), ("ok", "0")])
    })?;
    require(
        refused >= 3,
        format!("UART shows {refused} unauthenticated refusals, not 3"),
    )
}

fn restore_serve(ctx: &mut Ctx) -> Result<()> {
    ctx.rig.server(Server::Serve)?;
    ctx.health(HEALTH_SECONDS).map(|_| ())
}

/// Cut and restore the supply, then see the baseline start on bank 1 and
/// report health.
fn cold_boot(ctx: &mut Ctx) -> Result<usize> {
    let build = ctx.baseline()?.build.clone();
    // The server keeps reporting the old session's health as fresh until the
    // broker's keepalive lapses, and a request sent then is lost (seen on the
    // bench), so only health received after the new session counts.
    let before = ctx.rig.status()?.received_at().map(str::to_string);
    let since = ctx.mark()?;
    ctx.rig.power_cycle()?;

    let since = ctx.expect(
        "the baseline starting on bank 1",
        since,
        BOOT_SECONDS,
        |r| r.tag == "PLFW1" && r.first == "start" && started(r, &build),
    )?;
    ctx.expect(
        "the transport ready after boot",
        since,
        RECONNECT_SECONDS,
        |r| r.tag == "PLMQT1" && r.first == "ready",
    )?;
    let report = ctx.until_status("health from the new session", HEALTH_SECONDS, |status| {
        if status.received_at() == before.as_deref() {
            return Err(NotReady::Predates.into());
        }
        Ok(status.health_matches(&build)?)
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
    let changed = SettingsChange {
        lights: Some(before.lights != Some(1)),
        sound: Some(before.sound != Some(1)),
        volume: Some(if before.volume == Some(35) { 36 } else { 35 }),
    };

    let result = set_settings(ctx, &changed).and_then(|()| {
        cold_boot(ctx)?;
        let after = read_settings(ctx)?;
        require(
            after.shows(&changed),
            format!("After a cold boot the settings read {}", compact(&after)),
        )
    });
    let restored = set_settings(ctx, &before.settings());
    result.and(restored)
}

fn wifi_source(status: &LocalStatus, source: &str) -> Result<String> {
    let found = &status.fresh_health()?.wifi.source;
    if found.as_deref() == Some(source) {
        Ok(format!("Wi-Fi source {source}"))
    } else {
        Err(NotReady::WifiSource(found.clone()).into())
    }
}

/// A read of the Wi-Fi store.
fn wifi_state(ctx: &mut Ctx) -> Result<Reply> {
    ctx.ask(&Request::Wifi(WifiChange::default()))
}

fn clear_wifi() -> Request {
    Request::Wifi(WifiChange {
        clear: true,
        ..WifiChange::default()
    })
}

fn wifi_fallback(ctx: &mut Ctx) -> Result<()> {
    let state = wifi_state(ctx)?;
    reply_ok(&state)?;
    require(
        state.stored.is_none() && state.source.as_deref() == Some("build"),
        format!(
            "Stored credentials are already set; clear them first: {}",
            compact(&state)
        ),
    )?;

    let result = wrong_credentials(ctx);
    let cleared = clear_credentials(ctx);
    result.and(cleared)
}

fn wrong_credentials(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    reply_ok(&ctx.ask(&Request::Wifi(WifiChange {
        ssid: Some(ABSENT_SSID.into()),
        psk: Some(WRONG_PSK.into()),
        clear: false,
    }))?)?;
    ctx.expect("PLCFG1 net saved stored=1", since, SAVE_SECONDS, |r| {
        r.is("PLCFG1", &[("stored", "1")]) && r.body_starts("net saved")
    })?;

    let before = ctx.rig.status()?.received_at().map(str::to_string);
    ctx.rig.power_cycle()?;
    let report = ctx.offline(|ctx| {
        ctx.until_status(
            "the build's credentials after fallback",
            FALLBACK_SECONDS,
            |s| {
                if s.received_at() == before.as_deref() {
                    return Err(NotReady::Predates.into());
                }
                wifi_source(s, "fallback")
            },
        )
    })?;
    ctx.note(&format!("  {report}"));
    Ok(())
}

fn clear_credentials(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let state = wifi_state(ctx)?;
    if state.stored.is_none() {
        return Ok(());
    }

    reply_ok(&ctx.ask(&clear_wifi())?)?;
    ctx.expect("PLCFG1 net saved stored=0", since, SAVE_SECONDS, |r| {
        r.is("PLCFG1", &[("stored", "0")]) && r.body_starts("net saved")
    })?;
    cold_boot(ctx)?;
    let report = ctx.until_status("the build's credentials", HEALTH_SECONDS, |s| {
        wifi_source(s, "build")
    })?;
    ctx.note(&format!("  {report}"));
    Ok(())
}

fn network_bounce(ctx: &mut Ctx) -> Result<()> {
    let before = ctx.health(HEALTH_SECONDS)?.transport.connects;
    let since = ctx.mark()?;

    ctx.rig.hotspot(false)?;
    let lost = ctx.expect("PLMQT1 closed", since, LOSS_SECONDS, |r| {
        r.tag == "PLMQT1" && r.first == "closed"
    });
    let outage = ctx.rig.wait(OUTAGE_SECONDS);
    let restored = ctx.rig.hotspot(true);
    let since = lost?;
    outage?;
    restored?;

    ctx.expect("a reconnect", since, RECONNECT_SECONDS, |r| {
        r.tag == "PLMQT1" && r.first == "ready"
    })?;
    let after = ctx.health(HEALTH_SECONDS)?.transport.connects;
    require(
        after > before,
        format!("Health reports {after} transport connects, {before} before the outage"),
    )
}

/// One failing trial, then the fault drills from the recovery role it falls
/// back to, so the fallback needs no restore install of its own: the clean
/// download at the end is the restore.
fn ota_recovery(ctx: &mut Ctx) -> Result<()> {
    let failing = ctx.failing("fail")?;
    // Signed after the variant, so bank 0 accepts it as newer.
    let staged = ctx.resign("restore")?;

    let result = recovery(ctx, &failing, &staged);
    if result.is_err() {
        ctx.note(
            "  The feeder may be left on bank 0's recovery role; serving the baseline and cold-booting it",
        );
        let _ = ctx.rig.server(Server::Faulty {
            fault: Fault::None,
            staged: staged.clone(),
        });
        let _ = ctx.rig.power_cycle();
        let since = ctx.log.len();
        let _ = ctx.watch(&staged.build, since, INSTALL_SECONDS);
    }
    restore_serve(ctx)?;
    // A restarted serve holds no offer; clearing says so in the record.
    ctx.rig.offer(None)?;
    ctx.confirm(&staged.build)?;
    result
}

fn recovery(ctx: &mut Ctx, failing: &super::Staged, staged: &super::Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(failing))?;
    ctx.expect("PLOTA1 activated", since, INSTALL_SECONDS, |r| {
        r.tag == "PLOTA1" && r.first == "activated"
    })?;

    // The fault server takes over while the variant fails its trial, so
    // bank 0's recovery role meets it on its first boot. Each later case
    // needs a cold boot: a stopped update hands over to the transport until
    // the next one.
    let cases = [
        (Fault::Corrupt, "hash"),
        (Fault::Range, "range"),
        (Fault::Stall, "timeout"),
    ];
    for (i, (fault, stage)) in cases.into_iter().enumerate() {
        let start = ctx.mark()?;
        ctx.rig.server(Server::Faulty {
            fault,
            staged: staged.clone(),
        })?;
        if i == 0 {
            let outcome = ctx.watch(&failing.build, since, INSTALL_SECONDS)?;
            require(
                matches!(outcome, Outcome::FellBack { .. }),
                format!("The failing build was not rejected: {outcome:?}"),
            )?;
        } else {
            ctx.rig.power_cycle()?;
        }

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
    ctx.rig.server(Server::Faulty {
        fault: Fault::None,
        staged: staged.clone(),
    })?;
    ctx.rig.power_cycle()?;
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
    // cuts happen while it installs the speaker firmware (no motion), which
    // it has not; the baseline goes back on afterwards.
    let speaker = ctx.image(
        "cut-speaker",
        crate::commands::Image::of(crate::commands::Firmware::Speaker),
    )?;
    let restore = ctx.resign("cut-restore")?;

    let result = power_cuts(ctx, &speaker);
    let since = ctx.mark()?;
    ctx.rig.offer(Some(&restore))?;
    if result.is_err() {
        // A stopped update hands over to the transport until a cold boot.
        ctx.note("  Offering the baseline and cold-booting, whatever stage it stopped in");
        ctx.rig.power_cycle()?;
    }
    let outcome = ctx.watch(&build, since, INSTALL_SECONDS)?;
    ctx.rig.offer(None)?;
    ctx.confirm(&build)?;
    result?;
    require(
        outcome == Outcome::Passed,
        format!("The baseline did not come back: {outcome:?}"),
    )
}

fn power_cuts(ctx: &mut Ctx, staged: &super::Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(staged))?;
    let since = ctx.expect("PLWCH1 give_way=1", since, OFFER_CHECK_SECONDS, |r| {
        r.is("PLWCH1", &[("give_way", "1")])
    })?;

    // A cut mid-download: bank 1's marker is still clear, so the loader
    // must boot bank 0, which downloads again from the start.
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
    ctx.expect("PLCNF1 confirmed=1", since, HEALTH_SECONDS, confirmed)?;
    ctx.confirm(&staged.build)
}

/// One ping per period; the firmware's health period, so a soak sees every
/// report between them.
const SOAK_PERIOD_SECONDS: f64 = 30.0;

fn soak(ctx: &mut Ctx) -> Result<()> {
    let since = ctx.mark()?;
    let before = ctx.health(HEALTH_SECONDS)?.transport.connects;
    let end = ctx.rig.clock() + ctx.args.soak_minutes * 60.0;

    let (mut pings, mut slowest) = (0u32, 0.0f64);
    while ctx.rig.clock() < end {
        let asked = ctx.rig.clock();
        reply_ok(&ctx.ask(&Request::Ping)?)?;
        slowest = slowest.max(ctx.rig.clock() - asked);
        pings += 1;
        ctx.idle(SOAK_PERIOD_SECONDS)?;
    }

    let restarts = ctx
        .log
        .count(since, |r| r.tag == "PLFW1" && r.first == "start");
    let after = ctx.health(HEALTH_SECONDS)?.transport.connects;
    ctx.note(&format!(
        "  {pings} pings, slowest reply {slowest:.1} s, {restarts} restarts, {} reconnects",
        after.saturating_sub(before)
    ));
    require(
        restarts == 0,
        format!("The feeder restarted {restarts} times"),
    )?;
    require(
        after == before,
        format!("The transport reconnected ({before} connects, then {after})"),
    )
}

/// What every scenario must leave as it found it: the stored settings and
/// Wi-Fi credentials, and the baseline running on bank 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeviceState {
    settings: SettingsChange,
    wifi_stored: bool,
    build: String,
    bank: u64,
}

pub(super) fn device_state(ctx: &mut Ctx) -> Result<DeviceState> {
    let settings = read_settings(ctx)?.settings();
    let wifi = wifi_state(ctx)?;
    reply_ok(&wifi)?;
    let health = ctx.health(HEALTH_SECONDS)?;

    Ok(DeviceState {
        settings,
        wifi_stored: wifi.stored.is_some(),
        build: health.build.to_ascii_lowercase(),
        bank: health.bank.unwrap_or(0),
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
        if let Err(error) = set_settings(ctx, &want.settings) {
            changed.push(format!("restoring them failed: {error}"));
        }
    }
    if now.wifi_stored != want.wifi_stored {
        changed.push(format!(
            "stored Wi-Fi credentials {}",
            if now.wifi_stored {
                "left set"
            } else {
                "cleared"
            }
        ));
        if !want.wifi_stored
            && let Err(error) = ctx.ask(&clear_wifi()).and_then(|reply| reply_ok(&reply))
        {
            changed.push(format!("clearing them failed: {error}"));
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
