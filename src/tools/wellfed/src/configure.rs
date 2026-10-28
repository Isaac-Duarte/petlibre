//! Changing the feeder over the device API: its settings, the panel lock,
//! the network it joins, its time settings, the portion calibration and the
//! desiccant reminder.
//! Each change names the generation it was read at, so a change made
//! meanwhile is refused rather than overwritten.

use anyhow::{Context, Result, bail};
use clap::Args;
use wellfed_model::{
    self as model, Answer, DESICCANT_DAYS_MAX, DESICCANT_DAYS_MIN, Desiccant, MealReminder,
    NtpServer, Op, SettingsPatch, StatsSection, TimeChanges, Wifi, Window,
};

use crate::{Target, Toggle, ask_and_print, connect, passphrase, print_answer, print_json};

#[derive(Args)]
pub(crate) struct SettingsArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long)]
    lights: Option<Toggle>,
    #[arg(long)]
    sound: Option<Toggle>,
    /// 0-100.
    #[arg(long)]
    volume: Option<u8>,
    /// When lit lights may show: `always`, or `HH:MM-HH:MM` on the
    /// feeder's local clock (inclusive; may wrap midnight).
    #[arg(long, value_name = "WINDOW")]
    lights_window: Option<Window>,
    /// When sounds may play: `always`, or `HH:MM-HH:MM` as for
    /// `--lights-window`.
    #[arg(long, value_name = "WINDOW")]
    sound_window: Option<Window>,
    /// Days from a desiccant replacement until the next is due (1-90).
    #[arg(long, value_name = "DAYS", value_parser = desiccant_days)]
    desiccant_days: Option<u8>,
    /// A `meal_soon` event this many minutes (1-60) before each scheduled
    /// meal, or `off`.
    #[arg(long, value_name = "off|MINUTES")]
    meal_reminder: Option<MealReminder>,
}

/// A `--desiccant-days` value: the period's limits are the feeder's.
pub(crate) fn desiccant_days(text: &str) -> Result<u8, String> {
    match text.parse::<u8>() {
        Ok(days) if (DESICCANT_DAYS_MIN..=DESICCANT_DAYS_MAX).contains(&days) => Ok(days),
        _ => Err(format!(
            "{text:?} is not {DESICCANT_DAYS_MIN}-{DESICCANT_DAYS_MAX} days"
        )),
    }
}

#[derive(Args)]
pub(crate) struct DesiccantArgs {
    #[command(flatten)]
    target: Target,
    /// Record that the desiccant was replaced today (by the feeder's
    /// clock, which must be valid).
    #[arg(long)]
    reset: bool,
}

/// The reminder as `wellfed desiccant` prints it.
#[derive(serde::Serialize)]
struct DesiccantView {
    /// Where the count stands, from the status.
    count: Desiccant,
    /// The same, for a person.
    text: String,
    period_days: u8,
    replaced: Option<jiff::civil::Date>,
}

#[derive(Args)]
pub(crate) struct LockArgs {
    #[command(flatten)]
    target: Target,
    state: Toggle,
}

pub(crate) async fn lock_command(args: LockArgs) -> Result<()> {
    let locked = args.state.on();
    ask_and_print(&args.target, &Op::SetLock { locked }).await
}

#[derive(Args)]
pub(crate) struct WifiArgs {
    #[command(flatten)]
    target: Target,
    /// The network to store; the passphrase is read from standard input.
    #[arg(long, conflicts_with = "clear")]
    ssid: Option<String>,
    /// Forget the stored network; the build's applies again.
    #[arg(long)]
    clear: bool,
}

#[derive(Args)]
pub(crate) struct TimeArgs {
    #[command(flatten)]
    target: Target,
    /// The feeder's time zone, an IANA name (`America/Denver`), or
    /// `system` for this computer's; sent as the zone's POSIX rule.
    #[arg(long, conflicts_with = "clear_zone")]
    zone: Option<String>,
    /// Forget the zone; local time then follows the syncs' offsets.
    #[arg(long)]
    clear_zone: bool,
    /// Where the feeder asks for the time: `off`, `gateway`, `gateway:PORT`,
    /// `IP` or `IP:PORT` (port 123 unless named).
    #[arg(long, value_parser = parse_ntp)]
    ntp: Option<NtpServer>,
    /// Read the time settings and the clock's state.
    #[arg(long, conflicts_with_all = ["zone", "clear_zone", "ntp"])]
    show: bool,
}

/// An NTP server as `--ntp` takes it.
fn parse_ntp(text: &str) -> Result<NtpServer, String> {
    const PORT: u16 = 123;
    let port = |text: Option<&str>| match text {
        None => Ok(PORT),
        Some(port) => match port.parse::<u16>() {
            Ok(0) | Err(_) => Err(format!("{port:?} is not a port")),
            Ok(port) => Ok(port),
        },
    };
    let (host, port_text) = match text.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (text, None),
    };
    match host {
        "off" if port_text.is_none() => Ok(NtpServer::Off),
        "gateway" => Ok(NtpServer::Gateway {
            port: port(port_text)?,
        }),
        _ => match host.parse() {
            Ok(ip) => Ok(NtpServer::Address {
                ip,
                port: port(port_text)?,
            }),
            Err(_) => Err(format!(
                "{text:?} is not off, gateway[:PORT] or an IPv4 address[:PORT]"
            )),
        },
    }
}

#[derive(Args)]
pub(crate) struct CalibrateArgs {
    #[command(flatten)]
    target: Target,
    /// What the counted portions weighed together, in grams (`98.4`): the
    /// scale's reading of the bowl, less the empty bowl.
    #[arg(long, value_parser = parse_grams, requires = "portions")]
    weighed: Option<u32>,
    /// How many portions that food is: the `completed` counts of the feeds
    /// that dropped it.
    #[arg(long, requires = "weighed")]
    portions: Option<u32>,
    /// One portion's weight in grams, when already known.
    #[arg(long, value_parser = parse_grams, conflicts_with_all = ["weighed", "clear"])]
    portion: Option<u32>,
    /// Forget the stored weight.
    #[arg(long, conflicts_with = "weighed")]
    clear: bool,
}

/// A `--weighed` or `--portion` value that is not a weight in grams.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{text:?} is not a weight in grams (like 9.8)")]
struct NotGrams {
    text: String,
}

/// Grams with up to three decimals, as milligrams.
fn parse_grams(text: &str) -> Result<u32, NotGrams> {
    let invalid = || NotGrams {
        text: text.to_owned(),
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() || fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole: u32 = whole.parse().map_err(|_| invalid())?;
    let padded = format!("{fraction:0<3}");
    let fraction: u32 = padded.parse().map_err(|_| invalid())?;
    whole
        .checked_mul(1000)
        .and_then(|mg| mg.checked_add(fraction))
        .ok_or_else(invalid)
}

/// The mean of `portions` portions that weighed `weighed_mg` together,
/// rounded to the milligram.
fn mean_mg(weighed_mg: u32, portions: u32) -> Result<u32> {
    if portions == 0 {
        bail!("--portions must count at least one portion");
    }
    let mean = (u64::from(weighed_mg) + u64::from(portions) / 2) / u64::from(portions);
    Ok(u32::try_from(mean)?)
}

pub(crate) async fn settings_command(args: SettingsArgs) -> Result<()> {
    let changes = SettingsPatch {
        lights: args.lights.map(Toggle::on),
        sound: args.sound.map(Toggle::on),
        volume: args.volume,
        lights_window: args.lights_window,
        sound_window: args.sound_window,
        desiccant_days: args.desiccant_days,
        meal_reminder: args.meal_reminder,
    };
    if changes == SettingsPatch::default() {
        return ask_and_print(&args.target, &Op::GetSettings).await;
    }
    let (_, _, mut connection) = connect(&args.target).await?;
    let Answer::Settings(settings) = connection.ask(&Op::GetSettings).await? else {
        bail!("the feeder did not return its settings");
    };
    let answer = connection
        .ask(&Op::PutSettings {
            base: settings.generation,
            changes,
        })
        .await?;
    print_answer(&answer)
}

/// The reminder, read from the status and settings; with `--reset`, a
/// replacement recorded today first.
pub(crate) async fn desiccant_command(args: DesiccantArgs) -> Result<()> {
    let (_, _, mut connection) = connect(&args.target).await?;
    if args.reset {
        // An op the feeder predates would end the session rather than be
        // refused, so ask only a feeder that claims the reminder.
        if !connection.capabilities().desiccant {
            bail!("this feeder's build has no desiccant reminder; update it first");
        }
        match connection.ask(&Op::ResetDesiccant).await? {
            Answer::Done { .. } => eprintln!("Desiccant replaced today."),
            Answer::Refused(model::Refusal::NoClock) => bail!(
                "refused: the feeder's clock is invalid, so it cannot date the \
                 replacement; run `wellfed time` first"
            ),
            other => return print_answer(&other),
        }
    }

    let Answer::Status(status) = connection.ask(&Op::Status).await? else {
        bail!("the feeder did not return its status");
    };
    let Answer::Settings(settings) = connection.ask(&Op::GetSettings).await? else {
        bail!("the feeder did not return its settings");
    };
    print_json(&DesiccantView {
        count: status.desiccant,
        text: status.desiccant.to_string(),
        period_days: settings.desiccant_days,
        replaced: settings.desiccant_replaced,
    })
}

pub(crate) async fn wifi_command(args: WifiArgs) -> Result<()> {
    let passphrase = match &args.ssid {
        Some(_) => Some(passphrase()?),
        None if args.clear => None,
        None => bail!("pass --ssid or --clear"),
    };
    let (_, _, mut connection) = connect(&args.target).await?;
    let Answer::Status(status) = connection.ask(&Op::Status).await? else {
        bail!("the feeder did not return its status");
    };
    let wifi = match (args.ssid, passphrase) {
        (Some(ssid), Some(psk)) => Some(Wifi {
            ssid: ssid.into_bytes(),
            psk,
        }),
        _ => None,
    };
    let answer = connection
        .ask(&Op::PutWifi {
            base: status.wifi_generation,
            wifi,
        })
        .await?;
    print_answer(&answer)
}

pub(crate) async fn calibrate_command(args: CalibrateArgs) -> Result<()> {
    let portion_mg = match (args.weighed, args.portions, args.portion, args.clear) {
        (Some(weighed), Some(portions), None, false) => Some(Some(mean_mg(weighed, portions)?)),
        (None, None, Some(portion), false) => Some(Some(portion)),
        (None, None, None, true) => Some(None),
        _ => None,
    };

    let (_, _, mut connection) = connect(&args.target).await?;
    // An op the feeder predates would end the session rather than be
    // refused, so ask only a feeder that claims the calibration.
    if !connection.capabilities().calibration {
        bail!("this feeder's build keeps no calibration; update it first");
    }
    let Answer::Calibration(view) = connection.ask(&Op::GetCalibration).await? else {
        bail!("the feeder did not return its calibration");
    };
    let Some(portion_mg) = portion_mg else {
        print_json(&view)?;
        let portions = Op::Stats {
            section: StatsSection::Portions,
        };
        return print_answer(&connection.ask(&portions).await?);
    };

    if let Some(mg) = portion_mg {
        eprintln!("One portion: {mg} mg.");
    }
    let calibration = model::Calibration {
        portion_mg,
        ..view.calibration
    };
    let answer = connection
        .ask(&Op::PutCalibration {
            base: view.generation,
            calibration,
        })
        .await?;
    if answer == Answer::Refused(model::Refusal::Invalid) {
        bail!("refused: a portion must weigh 1-50 g");
    }
    print_answer(&answer)
}

pub(crate) async fn time_command(args: TimeArgs) -> Result<()> {
    let now = jiff::Timestamp::now();
    if args.show {
        return ask_and_print(&args.target, &Op::GetTime).await;
    }
    let zone = match (args.zone.as_deref(), args.clear_zone) {
        (_, true) => Some(String::new()),
        (None, false) => None,
        (Some(name), false) => {
            let name = match name {
                "system" => jiff::tz::TimeZone::system()
                    .iana_name()
                    .context("this computer's time zone has no IANA name")?
                    .to_owned(),
                name => name.to_owned(),
            };
            let host = jiff::tz::TimeZone::get(&name).ok();
            Some(wellfed_client::time::rule(&name, now, host.as_ref())?)
        }
    };
    if zone.is_none() && args.ntp.is_none() {
        let sync = wellfed_client::time::sync(now, &jiff::tz::TimeZone::system());
        return ask_and_print(&args.target, &Op::TimeSync(sync)).await;
    }

    let (_, _, mut connection) = connect(&args.target).await?;
    let Answer::Time(view) = connection.ask(&Op::GetTime).await? else {
        bail!("the feeder did not return its time settings (a build without them?)");
    };
    let answer = connection
        .ask(&Op::PutTime {
            base: view.generation,
            changes: TimeChanges {
                zone,
                ntp: args.ntp,
            },
        })
        .await?;
    print_answer(&answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grams_parse_to_milligrams() {
        assert_eq!(parse_grams("9.8"), Ok(9_800));
        assert_eq!(parse_grams("98.425"), Ok(98_425));
        assert_eq!(parse_grams("12"), Ok(12_000));
        for bad in ["", ".5", "9.8765", "9,8", "-1", "1e3", "9.x"] {
            assert!(parse_grams(bad).is_err(), "{bad}");
        }
        assert_eq!(mean_mg(98_400, 10).unwrap(), 9_840);
        assert_eq!(mean_mg(10_000, 3).unwrap(), 3_333);
        assert!(mean_mg(1_000, 0).is_err());
    }

    #[test]
    fn a_desiccant_period_is_one_to_ninety_days() {
        assert_eq!(desiccant_days("1"), Ok(1));
        assert_eq!(desiccant_days("90"), Ok(90));
        for bad in ["0", "91", "-1", "14d", ""] {
            assert!(desiccant_days(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ntp_servers_parse() {
        assert_eq!(parse_ntp("off"), Ok(NtpServer::Off));
        assert_eq!(parse_ntp("gateway"), Ok(NtpServer::Gateway { port: 123 }));
        assert_eq!(
            parse_ntp("gateway:1123"),
            Ok(NtpServer::Gateway { port: 1123 })
        );
        assert_eq!(
            parse_ntp("10.42.103.1:1123"),
            Ok(NtpServer::Address {
                ip: [10, 42, 103, 1].into(),
                port: 1123
            })
        );
        for bad in ["off:123", "gateway:0", "pool.ntp.org", "10.42.103.1:x"] {
            assert!(parse_ntp(bad).is_err(), "{bad}");
        }
    }
}
