//! FW34 BLE setup records as line bodies (`PLBLE1`, `petlibre_hal::ble`).
//! The setup task itself is the HAL's; this only lets a firmware log its
//! records through [`crate::log`].

use core::fmt;

use petlibre_hal::ble;

/// A `PLBLE1 …` body.
#[derive(Clone, Copy, Debug)]
pub struct Setup(pub ble::Record);

impl fmt::Display for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        ble::describe(self.0, f)
    }
}
