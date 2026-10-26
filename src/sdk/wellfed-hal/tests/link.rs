//! The Wi-Fi task's watch export: one machine per join, liveness published
//! to `net_status`, and the shim's loss events counted by cause.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::liveness::{Cause, PROBE_PERIOD_MS, Verdict};
use wellfed_hal::link::wellfed_wifi_watch;
use wellfed_hal::net_status;
use wellfed_hal::network::{wellfed_wifi_outcome, reset};
use wellfed_hal::sdk::{WifiEvent, fake};

/// `net_status` and the watch are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

/// The shim's probe codes.
const NONE: i32 = 0;
const ANSWERED: i32 = 1;
const SILENT: i32 = 2;

fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    net_status::reset();
    // SAFETY: the test is the only Wi-Fi task.
    unsafe { reset() };
    guard
}

fn joined() {
    // SAFETY: the test is the only Wi-Fi task, and no select is running.
    unsafe { wellfed_wifi_outcome(1) };
}

fn watch(associated: bool, leased: bool, probe: i32) -> Verdict {
    // SAFETY: the test is the only Wi-Fi task, between a join and its leave.
    let code = unsafe { wellfed_wifi_watch(associated.into(), leased.into(), probe) };
    Verdict::from_code(code).expect("the export answers a verdict's code")
}

fn later(ms: u32) {
    fake::with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(ms));
}

fn lost(cause: Cause) {
    net_status::observe(&WifiEvent {
        kind: WifiEvent::LOST,
        result: i32::from(cause.code()),
        value: 30_000,
        ..WifiEvent::default()
    });
}

#[test]
fn a_join_is_live_once_its_gateway_answers() {
    let _guard = start();
    joined();
    assert!(!net_status::read().live);

    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert!(!net_status::read().live);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);
    assert!(net_status::read().live);

    later(500);
    assert_eq!(watch(true, true, NONE), Verdict::Hold);
    assert!(net_status::read().live);
}

#[test]
fn a_missed_echo_is_not_live_and_an_answer_restores_it() {
    let _guard = start();
    joined();
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);

    later(PROBE_PERIOD_MS);
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, SILENT), Verdict::Hold);
    assert!(!net_status::read().live);

    later(PROBE_PERIOD_MS);
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Hold);
    assert!(net_status::read().live);
}

#[test]
fn a_leave_clears_liveness_before_the_loss_event_is_drained() {
    let _guard = start();
    joined();
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);

    assert_eq!(watch(true, false, NONE), Verdict::Leave(Cause::Lease));
    assert!(!net_status::read().live);
}

#[test]
fn every_join_starts_a_fresh_watch() {
    let _guard = start();
    joined();
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);
    assert_eq!(watch(false, true, NONE), Verdict::Leave(Cause::Link));

    // A failed attempt changes nothing; the next success starts over, and
    // probes at once rather than a period after the last join's probe.
    // SAFETY: the test is the only Wi-Fi task, and no select is running.
    unsafe { wellfed_wifi_outcome(0) };
    joined();
    assert!(!net_status::read().live);
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);
    assert!(net_status::read().live);
}

#[test]
fn losses_count_by_cause() {
    let _guard = start();

    lost(Cause::Link);
    let status = net_status::read();
    assert_eq!((status.link_losses, status.check_losses), (1, 0));
    assert_eq!(status.last_loss, 1);

    lost(Cause::Gateway);
    lost(Cause::Lease);
    let status = net_status::read();
    assert_eq!((status.link_losses, status.check_losses), (1, 2));
    assert_eq!(status.last_loss, 2);
    assert_eq!(status.generation, 3, "every loss is a new generation");
}
