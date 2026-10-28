//! The feeder's feeding plans: up to [`PLANS`] of them, each in a fixed
//! slot, as stock keeps at most ten.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Generation;
use crate::status::{WindowError, minute};

/// Plan slots a feeder keeps.
pub const PLANS: usize = 10;
/// The most portions one feed asks for, whoever asks (a client, a plan or
/// Dispense); a request outside 1 to this is refused
/// [`Refusal::Portions`](crate::Refusal::Portions).
pub const PORTIONS_MAX: u8 = 48;

/// One feeding plan: `portions` at minute `minute` of the feeder's local
/// day, on the weekdays `days` lists, or once at its next occurrence when
/// it lists none. A plan that ran once stays in its slot, disabled.
///
/// With no valid clock, plans hold; an occurrence more than 15 minutes
/// late (the feeder off, without the time, or busy) never runs, and the
/// history records it missed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// 0-1439.
    pub minute: u16,
    /// 1 to [`PORTIONS_MAX`].
    pub portions: u8,
    pub days: Weekdays,
    pub enabled: bool,
    /// How many times the meal call plays before the feed, 0 to
    /// [`CALL_PLAYS_MAX`](crate::CALL_PLAYS_MAX); 0 plays none. It plays
    /// only with a clip stored and sound allowed, ending as the meal is
    /// due, and never delays it.
    pub call: u8,
}

impl Plan {
    /// Whether the feeder accepts the plan.
    pub fn valid(&self) -> bool {
        self.minute < 24 * 60
            && (1..=PORTIONS_MAX).contains(&self.portions)
            && self.call <= crate::CALL_PLAYS_MAX
    }
}

/// The plans as the feeder holds them. Its `generation` goes back as the
/// `base` of a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plans {
    pub generation: Generation,
    /// Whether the plans store can take a write now.
    pub writable: bool,
    /// The plan in each slot.
    pub plans: [Option<Plan>; PLANS],
}

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Self; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Mon => "mon",
            Self::Tue => "tue",
            Self::Wed => "wed",
            Self::Thu => "thu",
            Self::Fri => "fri",
            Self::Sat => "sat",
            Self::Sun => "sun",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The weekdays a plan runs on; none is a plan that runs once. JSON lists
/// them (`["mon", "thu"]`); the command line takes `daily`, `weekdays`,
/// `weekends`, `once` or a comma-separated list (`mon,thu`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Weekdays(u8);

impl Weekdays {
    pub const ONCE: Self = Self(0);
    pub const DAILY: Self = Self(0x7f);

    /// The feeder's bit mask (bit 0 Monday to bit 6 Sunday). A bit past
    /// Sunday, which no feeder stores, is dropped.
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::DAILY.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn once(self) -> bool {
        self.0 == 0
    }

    pub fn days(self) -> impl Iterator<Item = Weekday> {
        Weekday::ALL
            .into_iter()
            .filter(move |day| self.0 & day.bit() != 0)
    }
}

impl FromIterator<Weekday> for Weekdays {
    fn from_iter<I: IntoIterator<Item = Weekday>>(days: I) -> Self {
        Self(days.into_iter().fold(0, |bits, day| bits | day.bit()))
    }
}

impl Serialize for Weekdays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.days())
    }
}

impl<'de> Deserialize<'de> for Weekdays {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<Weekday>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl fmt::Display for Weekdays {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ONCE => f.write_str("once"),
            Self::DAILY => f.write_str("daily"),
            days => {
                let mut first = true;
                for day in days.days() {
                    if !first {
                        f.write_str(",")?;
                    }
                    first = false;
                    f.write_str(day.code())?;
                }
                Ok(())
            }
        }
    }
}

/// Why a plan's days did not parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not daily, weekdays, weekends, once or a list like mon,thu")]
pub struct WeekdaysError(String);

impl FromStr for Weekdays {
    type Err = WeekdaysError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let weekdays = [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ];
        match text {
            "daily" => return Ok(Self::DAILY),
            "weekdays" => return Ok(weekdays.into_iter().collect()),
            "weekends" => return Ok([Weekday::Sat, Weekday::Sun].into_iter().collect()),
            "once" => return Ok(Self::ONCE),
            _ => {}
        }
        text.split(',')
            .map(|code| {
                Weekday::ALL
                    .into_iter()
                    .find(|day| day.code() == code)
                    .ok_or_else(|| WeekdaysError(text.to_owned()))
            })
            .collect()
    }
}

/// A plan's time of day, `HH:MM` on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeOfDay(pub u16);

impl FromStr for TimeOfDay {
    type Err = WindowError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        minute(text).map(Self)
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::{TimeOfDay, Weekday, Weekdays};

    #[test]
    fn days_read_back_as_written() {
        for text in ["daily", "once", "mon,thu", "sat,sun", "mon,tue,wed,thu,fri"] {
            let days: Weekdays = text.parse().unwrap();
            assert_eq!(days.to_string().parse::<Weekdays>(), Ok(days), "{text}");
        }
        assert_eq!("weekdays".parse::<Weekdays>().unwrap().bits(), 0x1f);
        assert_eq!("weekends".parse::<Weekdays>().unwrap().bits(), 0x60);
        assert!("mon,funday".parse::<Weekdays>().is_err());
        assert!("".parse::<Weekdays>().is_err());
    }

    #[test]
    fn days_are_a_list_in_json() {
        let days: Weekdays = [Weekday::Thu, Weekday::Mon].into_iter().collect();
        assert_eq!(serde_json::to_string(&days).unwrap(), r#"["mon","thu"]"#);
        assert_eq!(
            serde_json::from_str::<Weekdays>("[]").unwrap(),
            Weekdays::ONCE
        );
    }

    #[test]
    fn a_time_of_day_is_hours_and_minutes() {
        assert_eq!("07:30".parse::<TimeOfDay>().unwrap(), TimeOfDay(450));
        assert_eq!(TimeOfDay(450).to_string(), "07:30");
        assert!("24:00".parse::<TimeOfDay>().is_err());
    }
}
