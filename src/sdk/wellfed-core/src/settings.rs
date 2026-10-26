//! The device settings as the runtime's services see them: the view the
//! settings writer publishes (the speaker honours `sound` and `volume`), and
//! a `settings` request's fields and result, carried by
//! `wellfed_hal::settings` between the device API and the writer.

/// The fields of a `settings` request; none present reads the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsFields {
    pub lights: Option<bool>,
    pub sound: Option<bool>,
    pub volume: Option<u8>,
}

impl SettingsFields {
    /// Whether the request changes anything (and so must be signed).
    pub fn changes(&self) -> bool {
        self.lights.is_some() || self.sound.is_some() || self.volume.is_some()
    }
}

/// Largest `volume` a `settings` request may set, and the store accepts.
pub const MAX_VOLUME: u8 = 100;

/// The settings as a `settings` reply reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub lights: bool,
    pub sound: bool,
    pub volume: u8,
    /// The stored record's generation; 0 for defaults never saved.
    pub generation: u32,
    /// Whether NOR holds a record this firmware can update.
    pub writable: bool,
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
