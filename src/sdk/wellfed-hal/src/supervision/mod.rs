//! KM4 supervision: the TIM2 tick and feedback capture, the feeding owner's
//! beat, the watchdog and when it is fed, the breadcrumb a silent reset
//! leaves, and the bench-derived constants they run with.

pub(crate) mod active;
pub mod beat;
pub mod config;
pub mod crumb;
pub mod events;
pub mod supervise;
pub mod watchdog;

/// The watchdog feeder stops feeding on purpose (an intentional restart,
/// or a supervision decision such as a trial's deadline), so a reset
/// follows within the watchdog's expiry. The breadcrumb records that the
/// reset is the feeder's own, and the Wi-Fi task leaves the network
/// meanwhile, so the access point does not reject the next boot's
/// association with status 30. Idempotent; nothing here waits.
pub fn stopping() {
    crumb::stopping();
    crate::link::leave_for_restart();
}
