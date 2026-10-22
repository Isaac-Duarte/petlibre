//! The scenarios, in suite order. Each one leaves the feeder as it found it:
//! the baseline build confirmed on bank 1, `serve` answering, the hotspot up,
//! settings and stored Wi-Fi credentials as before. One that changes device
//! state restores it on failure too, as far as the feeder still answers.

use serde_json::{Value, json};

use super::log::Record;
use super::{
    Baseline, Ctx, Fault, HEALTH_SECONDS, INSTALL_SECONDS, Server, compact, fresh_health, reply_ok,
};
use crate::commands::ota::{Outcome, health_matches};
use crate::{Error, Result, require};

pub struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    /// Cold-boots the feeder through the supply relay (`--power`).
    pub power: bool,
    /// Builds a firmware variant (`--build`).
    pub build: bool,
    /// Installs the build under test; every other scenario needs it.
    pub baseline: bool,
    pub run: fn(&mut Ctx) -> Result<()>,
}

pub const SCENARIOS: [Scenario; 8] = [
    Scenario {
        name: "ota-update",
        about: "install the build under test over the air and see it confirm",
        power: false,
        build: false,
        baseline: true,
        run: ota_update,
    },
    Scenario {
        name: "requests",
        about: "ping, health, clock sync, and a settings change read back and restored",
        power: false,
        build: false,
        baseline: false,
        run: requests,
    },
    Scenario {
        name: "request-auth",
        about: "unsigned, wrongly signed and replayed requests are refused",
        power: false,
        build: false,
        baseline: false,
        run: request_auth,
    },
    Scenario {
        name: "settings-persist",
        about: "changed settings survive a cold boot, then are restored",
        power: true,
        build: false,
        baseline: false,
        run: settings_persist,
    },
    Scenario {
        name: "wifi-fallback",
        about: "wrong stored Wi-Fi credentials fall back to the build's; a signed clear recovers",
        power: true,
        build: false,
        baseline: false,
        run: wifi_fallback,
    },
    Scenario {
        name: "network-bounce",
        about: "the hotspot goes down and up; the transport reconnects and health resumes",
        power: false,
        build: false,
        baseline: false,
        run: network_bounce,
    },
    Scenario {
        name: "trial-fallback",
        about: "a build that fails its trial gives way to bank 0, which restores the baseline",
        power: false,
        build: true,
        baseline: false,
        run: trial_fallback,
    },
    Scenario {
        name: "ota-faults",
        about: "corrupt, truncated and stalled downloads stop before activation; then a clean one",
        power: true,
        build: true,
        baseline: false,
        run: ota_faults,
    },
];

/// Stored by `wifi-fallback`: an SSID nothing broadcasts, so every join
/// fails whatever the passphrase. Neither is a real credential.
const ABSENT_SSID: &str = "petlibre-itest-absent";
const WRONG_PSK: &str = "itest-not-a-passphrase";

/// The NOR settings writer saves within a second or two of a change.
const SAVE_SECONDS: f64 = 30.0;
/// From power-on to the first startup record.
const BOOT_SECONDS: f64 = 60.0;
/// Five failed joins of the stored network, then the build's join.
const FALLBACK_SECONDS: f64 = 8.0 * 60.0;
/// The confirmed build checks for an offer once a minute.
const OFFER_CHECK_SECONDS: f64 = 150.0;
/// The transport notices a dead link within a keepalive or two.
const LOSS_SECONDS: f64 = 120.0;
const OUTAGE_SECONDS: f64 = 20.0;
/// Reconnect backoff tops out at 30 s; joining takes a few more.
const RECONNECT_SECONDS: f64 = 180.0;
/// A full download (about 100 s) and the hash check, or a stall's retries.
const FAULT_SECONDS: f64 = 6.0 * 60.0;

fn ota_update(ctx: &mut Ctx) -> Result<()> {
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

    let args = ctx.ota_args("good", ctx.args.review.clone(), ctx.args.build);
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
    let ping = ctx.ask(json!({"op": "ping"}))?;
    reply_ok(&ping)?;
    require(
        ping["uptime_ms"].is_u64(),
        format!("Ping reply has no uptime_ms: {}", compact(&ping)),
    )?;
    ctx.expect("PLMQT1 request op=ping ok=1", since, 15.0, |r| {
        r.is("PLMQT1", &[("op", "ping"), ("ok", "1")])
    })?;

    reply_ok(&ctx.ask(json!({"op": "health"}))?)?;

    ctx.until_status("a synced firmware clock", HEALTH_SECONDS, clock_synced)
        .map(|report| ctx.note(&format!("  {report}")))?;

    let before = read_settings(ctx)?;
    let flipped = json!({"lights": before["lights"] != 1});
    set_settings(ctx, &flipped)?;
    set_settings(ctx, &settings_fields(&before))
}

/// Health shows a synced clock that agrees with the server's receive time.
fn clock_synced(status: &Value) -> std::result::Result<String, String> {
    let health = fresh_health(status)?;
    let clock = &health["clock"];
    if clock.is_null() {
        return Err("health has no clock (a pre-FW21 build)".into());
    }
    if !clock["syncs"].as_u64().is_some_and(|n| n >= 1) {
        return Err(format!("the clock never synced: {}", compact(clock)));
    }
    let unix_ms = clock["unix_ms"]
        .as_i64()
        .ok_or_else(|| format!("the clock is not set: {}", compact(clock)))?;

    let received = status["replacement"]["received_at"]
        .as_str()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .ok_or("status has no received_at")?;
    let skew_ms = received.timestamp_millis() - unix_ms;
    if skew_ms.abs() > 5_000 {
        return Err(format!(
            "the firmware clock is {skew_ms} ms behind the server"
        ));
    }
    Ok(format!(
        "Clock: {} syncs, {skew_ms} ms behind the server at receipt",
        clock["syncs"]
    ))
}

fn read_settings(ctx: &mut Ctx) -> Result<Value> {
    let settings = ctx.ask(json!({"op": "settings"}))?;
    reply_ok(&settings)?;
    require(
        settings["writable"] == true,
        format!("Settings are not writable: {}", compact(&settings)),
    )?;
    Ok(settings)
}

/// The request fields that would set a reply's values again.
fn settings_fields(reply: &Value) -> Value {
    json!({
        "lights": reply["lights"] == 1,
        "sound": reply["sound"] == 1,
        "volume": reply["volume"],
    })
}

/// Whether a settings reply holds every field of a settings request.
fn settings_match(reply: &Value, fields: &Value) -> bool {
    fields
        .as_object()
        .into_iter()
        .flatten()
        .all(|(key, want)| match want {
            Value::Bool(on) => reply[key] == u8::from(*on),
            other => &reply[key] == other,
        })
}

/// Change settings, see the writer save a newer generation, and read the
/// change back.
fn set_settings(ctx: &mut Ctx, fields: &Value) -> Result<()> {
    let generation = read_settings(ctx)?["generation"].as_u64().unwrap_or(0);
    let since = ctx.mark()?;

    let mut request = json!({"op": "settings"});
    for (key, value) in fields.as_object().into_iter().flatten() {
        request[key] = value.clone();
    }
    reply_ok(&ctx.ask(request)?)?;
    ctx.expect("PLCFG1 saved", since, SAVE_SECONDS, |r| {
        r.tag == "PLCFG1"
            && r.first == "saved"
            && r.number::<u64>("generation")
                .is_some_and(|g| g > generation)
    })?;

    let after = read_settings(ctx)?;
    require(
        settings_match(&after, fields),
        format!(
            "Settings read back {} after setting {}",
            compact(&after),
            compact(fields)
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
            .find(|line| line["case"] == case)
            .map_or(Value::Null, |line| line["reply"].clone())
    };
    for case in ["unsigned", "wrong_key", "replay"] {
        let answer = reply(case);
        ctx.note(&format!("  {case}: {}", compact(&answer)));
        require(
            answer["ok"] == false && answer["error"] == "auth",
            format!(
                "The {case} request was not refused for auth: {}",
                compact(&answer)
            ),
        )?;
    }
    let valid = reply("valid");
    ctx.note(&format!("  valid: {}", compact(&valid)));
    require(
        valid["ok"] == true && valid["op"] == "settings",
        format!(
            "The validly signed request was not answered: {}",
            compact(&valid)
        ),
    )?;

    ctx.mark()?;
    let refused = ctx.log.count(since, |r| {
        r.is("PLMQT1", &[("op", "unauthenticated"), ("ok", "0")])
    });
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
    let before = ctx.rig.status()?["replacement"]["received_at"].clone();
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
        if status["replacement"]["received_at"] == before {
            return Err("health predates the boot".into());
        }
        health_matches(status, &build)
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
    let volume = before["volume"].as_u64().unwrap_or(0);
    let changed = json!({
        "lights": before["lights"] != 1,
        "sound": before["sound"] != 1,
        "volume": if volume == 35 { 36 } else { 35 },
    });

    let result = set_settings(ctx, &changed).and_then(|()| {
        cold_boot(ctx)?;
        let after = read_settings(ctx)?;
        require(
            settings_match(&after, &changed),
            format!("After a cold boot the settings read {}", compact(&after)),
        )
    });
    let restored = set_settings(ctx, &settings_fields(&before));
    result.and(restored)
}

fn wifi_source(status: &Value, source: &str) -> std::result::Result<String, String> {
    let health = fresh_health(status)?;
    match health["wifi"]["source"].as_str() {
        Some(found) if found == source => Ok(format!("Wi-Fi source {source}")),
        found => Err(format!("Wi-Fi source {found:?}")),
    }
}

fn wifi_fallback(ctx: &mut Ctx) -> Result<()> {
    let state = ctx.ask(json!({"op": "wifi"}))?;
    reply_ok(&state)?;
    require(
        state["stored"].is_null() && state["source"] == "build",
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
    reply_ok(&ctx.ask(json!({"op": "wifi", "ssid": ABSENT_SSID, "psk": WRONG_PSK}))?)?;
    ctx.expect("PLCFG1 net saved stored=1", since, SAVE_SECONDS, |r| {
        r.is("PLCFG1", &[("stored", "1")]) && r.body_starts("net saved")
    })?;

    let before = ctx.rig.status()?["replacement"]["received_at"].clone();
    ctx.rig.power_cycle()?;
    let report = ctx.offline(|ctx| {
        ctx.until_status(
            "the build's credentials after fallback",
            FALLBACK_SECONDS,
            |s| {
                if s["replacement"]["received_at"] == before {
                    return Err("health predates the boot".into());
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
    let state = ctx.ask(json!({"op": "wifi"}))?;
    if state["stored"].is_null() {
        return Ok(());
    }

    reply_ok(&ctx.ask(json!({"op": "wifi", "clear": true}))?)?;
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

fn connects(health: &Value) -> u64 {
    health["transport"]["connects"].as_u64().unwrap_or(0)
}

fn network_bounce(ctx: &mut Ctx) -> Result<()> {
    let before = connects(&ctx.health(HEALTH_SECONDS)?);
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
    let after = connects(&ctx.health(HEALTH_SECONDS)?);
    require(
        after > before,
        format!("Health reports {after} transport connects, {before} before the outage"),
    )
}

fn trial_fallback(ctx: &mut Ctx) -> Result<()> {
    let build = ctx.baseline()?.build.clone();
    let failing = ctx.failing("fail")?;
    // Signed after the variant, so bank 0 accepts it as newer.
    let restore = ctx.resign("restore")?;

    let since = ctx.mark()?;
    ctx.rig.offer(Some(&failing))?;
    ctx.expect("PLOTA1 activated", since, INSTALL_SECONDS, |r| {
        r.tag == "PLOTA1" && r.first == "activated"
    })?;
    // Bank 0's recovery role takes whatever is offered after the fallback.
    ctx.rig.offer(Some(&restore))?;

    let outcome = ctx.watch(&failing.build, since, INSTALL_SECONDS)?;
    require(
        matches!(outcome, Outcome::FellBack { .. }),
        format!("The failing build was not rejected: {outcome:?}"),
    )?;

    let since = ctx.mark()?;
    let outcome = ctx.watch(&build, since, INSTALL_SECONDS)?;
    require(
        outcome == Outcome::Passed,
        format!("The baseline did not come back: {outcome:?}"),
    )?;
    ctx.confirm(&build)?;
    ctx.rig.offer(None)
}

fn ota_faults(ctx: &mut Ctx) -> Result<()> {
    // A confirmed build ignores an offer of itself (`same=1`), so another
    // build makes it give way; the fault server then serves the baseline,
    // signed after it.
    let trigger = ctx.failing("trigger")?;
    let staged = ctx.resign("fault")?;

    let result = faults(ctx, &trigger, &staged);
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

fn faults(ctx: &mut Ctx, trigger: &super::Staged, staged: &super::Staged) -> Result<()> {
    let since = ctx.mark()?;
    ctx.rig.offer(Some(trigger))?;
    ctx.expect("PLWCH1 give_way=1", since, OFFER_CHECK_SECONDS, |r| {
        r.is("PLWCH1", &[("give_way", "1")])
    })?;

    // The recovery role takes seconds to boot and join, so it meets the
    // fault server, not `serve`. Each later case needs a cold boot: a
    // stopped update hands over to the transport until the next one.
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
        if i > 0 {
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
