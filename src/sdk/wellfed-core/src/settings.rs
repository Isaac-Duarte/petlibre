//! The device settings as the runtime's services see them: the view the
//! settings writer publishes (the speaker honours `sound`, `volume` and the
//! sound window; the loop's lights honour `lights` and the lights window;
//! and the schedule's meal reminder reads `meal_reminder`), and a
//! `settings` request's fields and result, carried by
//! `wellfed_hal::settings` between the device API and the writer.

/// Minutes in a local day: a window's bounds are below it.
pub const DAY_MINUTES: u16 = 24 * 60;

/// When a light or sound setting that is on may show or play, as stock's
/// "aging type" selects: always, or only inside a window of the local day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Window {
    #[default]
    Always,
    /// From minute `start` to minute `end` of the local day, both included;
    /// a window whose end is before its start wraps past midnight, and
    /// `start == end` is that one minute, as stock compares them.
    Between { start: u16, end: u16 },
}

impl Window {
    /// Whether both bounds are minutes of a day.
    pub const fn valid(self) -> bool {
        match self {
            Self::Always => true,
            Self::Between { start, end } => start < DAY_MINUTES && end < DAY_MINUTES,
        }
    }

    /// Whether the window allows the local wall time `local_ms` (Unix
    /// milliseconds as if local were UTC, `None` while the clock is
    /// invalid or has no offset). Without a local time every window
    /// allows: lights and sound behave as always on until the clock syncs,
    /// so a fault light is never hidden by a clock the feeder lost.
    pub fn allows(self, local_ms: Option<i64>) -> bool {
        let Self::Between { start, end } = self else {
            return true;
        };
        let Some(local_ms) = local_ms else {
            return true;
        };

        let minute = local_ms
            .div_euclid(60_000)
            .rem_euclid(i64::from(DAY_MINUTES));
        let (start, end) = (i64::from(start), i64::from(end));
        if start <= end {
            (start..=end).contains(&minute)
        } else {
            minute >= start || minute <= end
        }
    }
}

/// Whether the feeder announces each scheduled meal ahead of it, and how
/// far ahead: a `MealSoon` event on the event stream, never a sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MealReminder {
    #[default]
    Off,
    /// [`REMINDER_MINUTES_MIN`] to [`REMINDER_MINUTES_MAX`] minutes before
    /// each occurrence.
    Before { minutes: u8 },
}

/// The shortest lead a meal reminder takes.
pub const REMINDER_MINUTES_MIN: u8 = 1;
/// The longest lead a meal reminder takes: an hour, a choice of ours, as
/// stock's cloud field (`feedingPlanAdvanceNoticeTime`) gives no unit.
pub const REMINDER_MINUTES_MAX: u8 = 60;

impl MealReminder {
    /// Whether the lead is one the store accepts.
    pub const fn valid(self) -> bool {
        match self {
            Self::Off => true,
            Self::Before { minutes } => {
                minutes >= REMINDER_MINUTES_MIN && minutes <= REMINDER_MINUTES_MAX
            }
        }
    }

    /// The lead in minutes, or `None` when off.
    pub const fn minutes(self) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::Before { minutes } => Some(minutes),
        }
    }
}

/// The fields of a `settings` request; none present reads the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsFields {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    pub volume: Option<u8>,
    pub lights_window: Option<Window>,
    pub sound_window: Option<Window>,
    /// The desiccant reminder's period in days
    /// ([`crate::desiccant::PERIOD_MIN`] to [`crate::desiccant::PERIOD_MAX`]).
    pub desiccant_days: Option<u8>,
    /// Record a desiccant replacement made on this local calendar day
    /// ([`crate::desiccant::today`]); set only by the reset request, which
    /// reads the day from a valid clock.
    pub desiccant_reset: Option<i32>,
    pub meal_reminder: Option<MealReminder>,
}

impl SettingsFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.lights.is_some()
            || self.sound.is_some()
            || self.volume.is_some()
            || self.lights_window.is_some()
            || self.sound_window.is_some()
            || self.desiccant_days.is_some()
            || self.desiccant_reset.is_some()
            || self.meal_reminder.is_some()
    }

    /// Whether every field present is one the store accepts.
    pub fn valid(&self) -> bool {
        self.volume.is_none_or(|volume| volume <= MAX_VOLUME)
            && self.lights_window.is_none_or(Window::valid)
            && self.sound_window.is_none_or(Window::valid)
            && self
                .desiccant_days
                .is_none_or(crate::desiccant::period_valid)
            && self.meal_reminder.is_none_or(MealReminder::valid)
    }
}

/// Largest `volume` a `settings` request may set, and the store accepts.
/// Volume 0 mutes every sound, as stock's does.
pub const MAX_VOLUME: u8 = 100;

/// The settings as a `settings` reply reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    pub lights_window: Window,
    pub sound_window: Window,
    /// The desiccant reminder's period in days.
    pub desiccant_days: u8,
    /// The local calendar day of the last desiccant replacement; `None`
    /// before the first.
    pub desiccant_reset: Option<i32>,
    pub meal_reminder: MealReminder,
    /// The stored record's generation; 0 for defaults never saved.
    pub generation: u32,
    /// Whether NOR holds a record this firmware can update.
    pub writable: bool,
}

impl SettingsView {
    /// Whether the lights may show at `local_ms` ([`Window::allows`]).
    pub fn lights_at(&self, local_ms: Option<i64>) -> bool {
        self.lights && self.lights_window.allows(local_ms)
    }

    /// Whether a sound may play at `local_ms`: the switch on, a volume
    /// above 0 and inside the sound window.
    pub fn sound_at(&self, local_ms: Option<i64>) -> bool {
        self.sound && self.volume > 0 && self.sound_window.allows(local_ms)
    }

    /// The desiccant reminder on local calendar day `today` (`None` while
    /// the clock is invalid).
    pub fn desiccant(&self, today: Option<i32>) -> crate::desiccant::Count {
        crate::desiccant::Count::new(self.desiccant_days, self.desiccant_reset, today)
    }
}

/// How a `settings` request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsResult {
    /// The current settings; `pending` once a change is queued for the writer.
    Current { view: SettingsView, pending: bool },
    /// Not loaded yet, or a change already pending.
    Busy,
    /// The store cannot be written (corrupt, newer schema, worn or unarmed).
    ReadOnly,
}

/// `off`, or the lead in minutes, as records spell it.
impl core::fmt::Display for MealReminder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Off => f.write_str("off"),
            Self::Before { minutes } => write!(f, "{minutes}"),
        }
    }
}

/// `always`, or the window's bounds as `HH:MM-HH:MM`, as records spell it.
/// `wellfed_model::Window` (the CLI's and JSON clients' type, which this
/// no_std crate cannot share) spells it the same, and parses it.
impl core::fmt::Display for Window {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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
