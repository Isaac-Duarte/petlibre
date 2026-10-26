//! The Wi-Fi task's watch export: one machine per join, liveness published
//! to `net_status`, and the shim's loss events counted by cause.

use std::sync::{Mutex, MutexGuard};

use wellfed_core::liveness::{Cause, PROBE_PERIOD_MS, Verdict};
use wellfed_hal::link::{self, wellfed_wifi_restarting, wellfed_wifi_watch};
use wellfed_hal::net_status;
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
    link::reset();
    guard
}

/// A join got its address, as the join outcome reports it.
fn joined() {
    // SAFETY: the test is the only Wi-Fi task, between watches.
    unsafe { link::joined() };
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

    // The next success starts over, and probes at once rather than a
    // period after the last join's probe.
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

#[test]
fn a_restart_leaves_a_live_link_for_good() {
    let _guard = start();
    joined();
    assert_eq!(watch(true, true, NONE), Verdict::Probe);
    assert_eq!(watch(true, true, ANSWERED), Verdict::Settled);
    assert!(net_status::read().live);
    assert_eq!(wellfed_wifi_restarting(), 0);

    link::leave_for_restart();

    // The shim sees it between its waits, before the next watch tick.
    assert_eq!(wellfed_wifi_restarting(), 1);
    // Whatever the link shows, and whatever probe was asked for, the next
    // watch leaves it, and the link is no longer live for confirmation.
    assert_eq!(watch(true, true, ANSWERED), Verdict::Leave(Cause::Restart));
    assert!(!net_status::read().live);
    assert_eq!(watch(false, false, NONE), Verdict::Leave(Cause::Restart));
}

#[test]
fn a_restart_during_a_join_leaves_as_soon_as_it_joins() {
    let _guard = start();

    // Asked for between attempts or while one runs: the shim stops joining,
    // and a join that still got an address leaves at its first watch.
    link::leave_for_restart();
    assert_eq!(wellfed_wifi_restarting(), 1);
    joined();
    assert_eq!(watch(true, true, NONE), Verdict::Leave(Cause::Restart));
    assert!(!net_status::read().live);
}

fn event(kind: u32, result: i32, value: u32, ms: u32) {
    net_status::observe(&WifiEvent {
        kind,
        result,
        value,
        ms,
        ..WifiEvent::default()
    });
}

/// The statistics' link figures: answered probes and their round trip,
/// renewals, the access point's channel and how often it changed.
#[test]
fn the_link_figures_follow_the_shim_events() {
    let _guard = start();

    event(WifiEvent::GATEWAY, -4, 0x0a00_0001, 1000);
    let status = net_status::read();
    assert_eq!((status.gateway_answers, status.gateway_ms), (0, 0));
    event(WifiEvent::GATEWAY, 0, 0x0a00_0001, 4);
    let status = net_status::read();
    assert_eq!((status.gateway_answers, status.gateway_ms), (1, 4));

    event(WifiEvent::RENEW, 0, 0, 0);
    assert_eq!(net_status::read().dhcp_renewals, 1);

    event(WifiEvent::AP, 6, 0x1122_3344, 0);
    let status = net_status::read();
    assert_eq!((status.channel, status.ap_changes), (Some(6), 0));
    // The same access point again, then an unread BSSID: no change.
    event(WifiEvent::AP, 6, 0x1122_3344, 0);
    event(WifiEvent::AP, -1, 0, 0);
    let status = net_status::read();
    assert_eq!((status.channel, status.ap_changes), (None, 0));
    event(WifiEvent::AP, 11, 0x5566_7788, 0);
    let status = net_status::read();
    assert_eq!((status.channel, status.ap_changes), (Some(11), 1));

    lost(Cause::Link);
    assert_eq!(
        net_status::read().channel,
        None,
        "no channel while not joined"
    );
}
