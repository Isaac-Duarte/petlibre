//! What the feeder reports about itself, and its settings.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::Generation;

/// What the feeder reports about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = FeederStatus)]
pub struct Status {
    pub build: String,
    /// The flash bank it booted from.
    pub bank: u8,
    pub uptime_ms: u32,
    pub boot_count: u32,
    pub reset_flags: u32,
    pub faults: u32,
    /// Wi-Fi signal, dBm.
    pub rssi: i32,
    pub wifi: WifiSource,
    pub link_losses: u32,
    /// Losses the feeder's link check noticed (its DHCP lease ran out or
    /// its gateway stopped answering) rather than the association. The name
    /// is from the internet DNS check that check replaced.
    pub dns_losses: u32,
    /// The feeder's clock; `None` while it is invalid.
    pub clock: Option<Timestamp>,
    pub power: Option<Power>,
    /// The panel's child lock; `None` before it is known.
    pub locked: Option<bool>,
    pub settings_generation: Generation,
    pub wifi_generation: Generation,
    /// The newest event's sequence number on the feeder.
    pub last_event: Option<u64>,
    /// Open device API sessions, the server's included.
    pub sessions: u8,
    /// Whether a feed can start now; absent from a feeder that does not
    /// report it.
    pub readiness: Option<Readiness>,
    /// The desiccant reminder.
    pub desiccant: Desiccant,
    /// The supply as the feeder judges it, with the pack's charge; absent
    /// from a feeder that does not report it.
    pub supply: Option<Supply>,
}

/// The desiccant reminder, counted in the feeder's local calendar days
/// from the last replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Desiccant {
    /// No replacement is recorded yet: reset it once the pack is fresh.
    Unset,
    /// The feeder's clock is invalid, so the days cannot be counted.
    Unknown,
    /// Days left of the period; `due` from 0 on, and the count goes on
    /// below zero.
    Remaining { days: i32, due: bool },
}

/// For a person: `not started`, `unknown (no clock)`, `5 days left`, `due
/// today` or `due (3 days over)`.
impl fmt::Display for Desiccant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Unset => f.write_str("not started"),
            Self::Unknown => f.write_str("unknown (no clock)"),
            Self::Remaining { days: 0, .. } => f.write_str("due today"),
            Self::Remaining { days, .. } if days > 0 => write!(f, "{} left", Days(days)),
            Self::Remaining { days, .. } => write!(f, "due ({} over)", Days(days)),
        }
    }
}

/// A count of days, singular for one.
struct Days(i32);

impl fmt::Display for Days {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.unsigned_abs() {
            1 => f.write_str("1 day"),
            days => write!(f, "{days} days"),
        }
    }
}

/// The supply as the feeder judges it: the adapter, the pack and the
/// feeding owner's verdict on its floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Supply {
    /// The adapter's detect pin; absent with no fresh sample. Without the
    /// adapter the feeder runs on its batteries, and turns its Wi-Fi off
    /// 5 s later until the adapter returns.
    pub adapter: Option<bool>,
    pub battery: Battery,
    /// Whether the feeding owner judges the pack below the floor a feed
    /// needs; absent before it has judged.
    pub below_floor: Option<bool>,
}

/// Whether the feeder can start a feed now: the one decision a feed
/// request obeys too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Blocked(Blocked),
}

/// Why a feed cannot start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// The feeding owner has not opened yet.
    Starting,
    /// The feed journal could not be opened: no feed this boot.
    Journal,
    /// The feeding owner stopped on a fault: no feed until a restart.
    Fault,
    /// A feed a reset interrupted is being resolved as uncertain.
    Resolving,
    /// A feed is queued or runs.
    Feeding,
    /// A Wi-Fi join runs or waits.
    Joining,
    /// An install holds the flash.
    Updating,
    /// BLE setup mode runs, which ends in a restart.
    Setup,
    /// The firmware has no feeding owner.
    Unserved,
    /// On its batteries, below the floor a feed needs (or unread), or
    /// after a brownout until the adapter returns.
    Battery,
}

impl Blocked {
    pub const ALL: [Self; 10] = [
        Self::Starting,
        Self::Journal,
        Self::Fault,
        Self::Resolving,
        Self::Feeding,
        Self::Joining,
        Self::Updating,
        Self::Setup,
        Self::Unserved,
        Self::Battery,
    ];

    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Journal => "journal",
            Self::Fault => "fault",
            Self::Resolving => "resolving",
            Self::Feeding => "feeding",
            Self::Joining => "joining",
            Self::Updating => "updating",
            Self::Setup => "setup",
            Self::Unserved => "unserved",
            Self::Battery => "battery",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.code() == code)
    }
}

/// Which Wi-Fi credentials the feeder joined with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WifiSource {
    Build,
    Stored,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Power {
    pub adapter: bool,
    pub battery_mv: Option<u32>,
}

/// The battery pack, as the feeder judged its reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Battery {
    /// No pack fitted.
    Absent,
    Reading {
        mv: u16,
        /// The charge on stock's curve, 0-100 in steps of 10.
        #[schema(maximum = 100)]
        percent: u8,
    },
    /// A value no pack of the board can read, as read.
    Implausible { mv: u16 },
    /// Nothing sampled.
    Unread,
    /// The last sample is this old, past the feeder's limit.
    Stale { age_ms: u32 },
}

/// The feeder's settings. Its `generation` goes back in `If-Match` with a
/// change, and is the `ETag` of a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    pub generation: Generation,
    pub lights: bool,
    pub sound: bool,
    /// 0-100.
    pub volume: u8,
    /// When the lights may show while `lights` is on.
    pub lights_window: Window,
    /// When sounds may play while `sound` is on and `volume` is above 0.
    pub sound_window: Window,
    /// The desiccant reminder's period: days from a replacement until the
    /// next is due.
    #[schema(minimum = 1, maximum = 90)]
    pub desiccant_days: u8,
    /// The feeder's local date of the last desiccant replacement; `None`
    /// before the first.
    #[schema(value_type = Option<String>, format = Date)]
    pub desiccant_replaced: Option<jiff::civil::Date>,
    /// Whether the settings store can take a write now.
    pub writable: bool,
}

/// The settings to change; absent fields stay as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SettingsPatch {
    #[serde(default)]
    pub lights: Option<bool>,
    #[serde(default)]
    pub sound: Option<bool>,
    /// 0-100.
    #[serde(default)]
    pub volume: Option<u8>,
    #[serde(default)]
    pub lights_window: Option<Window>,
    #[serde(default)]
    pub sound_window: Option<Window>,
    /// The desiccant reminder's period in days, 1-90.
    #[serde(default)]
    #[schema(minimum = 1, maximum = 90)]
    pub desiccant_days: Option<u8>,
}

/// When lights or sound that are on may show or play: always, or only from
/// minute `start` to minute `end` of the feeder's local day (0-1439, both
/// included; an end before the start wraps past midnight). While the
/// feeder's clock is invalid every window is open.
///
/// Spelled `always` or `HH:MM-HH:MM` on the command line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Window {
    #[default]
    Always,
    Between {
        #[schema(maximum = 1439)]
        start: u16,
        #[schema(maximum = 1439)]
        end: u16,
    },
}

/// Why a window's text did not parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WindowError {
    #[error("`{0}` is not `always` or HH:MM-HH:MM")]
    Format(String),
    #[error("`{0}` is not a time of day (00:00 to 23:59)")]
    Time(String),
}

impl Window {
    /// Whether both bounds are minutes of a day (0-1439); the feeder
    /// refuses any other.
    pub const fn valid(self) -> bool {
        match self {
            Self::Always => true,
            Self::Between { start, end } => start < DAY_MINUTES && end < DAY_MINUTES,
        }
    }
}

/// Minutes in a day: a window's bounds are below it.
const DAY_MINUTES: u16 = 24 * 60;

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Always => f.write_str("always"),
            Self::Between { start, end } => write!(
                f,
                "{:02}:{:02}-{:02}:{:02}",
                start / 60,
                start % 60,
                end / 60,
                end % 60
            ),
        }
    }
}

impl FromStr for Window {
    type Err = WindowError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text == "always" {
            return Ok(Self::Always);
        }
        let Some((start, end)) = text.split_once('-') else {
            return Err(WindowError::Format(text.to_owned()));
        };
        Ok(Self::Between {
            start: minute(start)?,
            end: minute(end)?,
        })
    }
}

/// `HH:MM` as a minute of the day.
pub(crate) fn minute(text: &str) -> Result<u16, WindowError> {
    let invalid = || WindowError::Time(text.to_owned());
    let (hours, minutes) = text.split_once(':').ok_or_else(invalid)?;
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(hours) || !digits(minutes) {
        return Err(invalid());
    }
    let hours: u16 = hours.parse().map_err(|_| invalid())?;
    let minutes: u16 = minutes.parse().map_err(|_| invalid())?;
    if hours >= 24 || minutes >= 60 {
        return Err(invalid());
    }
    Ok(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::{Window, WindowError};

    #[test]
    fn a_window_parses_from_and_back_to_its_text() {
        for text in [
            "always",
            "07:00-22:00",
            "22:30-06:15",
            "00:00-00:00",
            "23:59-00:00",
        ] {
            let window: Window = text.parse().unwrap();
            assert_eq!(window.to_string(), text);
        }
        assert_eq!(
            "22:30-06:15".parse(),
            Ok(Window::Between {
                start: 22 * 60 + 30,
                end: 6 * 60 + 15
            })
        );
    }

    #[test]
    fn a_window_that_is_not_two_times_of_day_is_refused() {
        for text in [
            "",
            "never",
            "07:00",
            "7:00-22:00",
            "24:00-01:00",
            "07:60-08:00",
            "+7:00-08:00",
        ] {
            let refused = text.parse::<Window>();
            assert!(
                matches!(refused, Err(WindowError::Format(_) | WindowError::Time(_))),
                "{text}: {refused:?}"
            );
        }
    }

    #[test]
    fn a_window_is_json_tagged_by_mode() {
        let window = Window::Between {
            start: 420,
            end: 1320,
        };
        let json = serde_json::to_string(&window).unwrap();
        assert_eq!(json, r#"{"mode":"between","start":420,"end":1320}"#);
        assert_eq!(serde_json::from_str::<Window>(&json).unwrap(), window);
        assert_eq!(
            serde_json::to_string(&Window::Always).unwrap(),
            r#"{"mode":"always"}"#
        );
    }
}
