//! Tests for the motor output gate: deadtime before every drive or
//! reversal, lease expiry from an independent tick, I/O failures and clock
//! wrap.
//!
//! The fake pin callback rejects LOW/LOW. Every fault leaves both pins HIGH
//! (STOP), and a failed STOP write does not claim the outputs are verified.

use core::ffi::c_void;

use petlibre_core::feed::controller::MotorCommand;
use petlibre_core::feed::motor_gate::{MotorGate, MotorGateFault, MotorPins};

#[derive(Default)]
struct Output {
    writes: u32,
    stops: u32,
    a: bool,
    b: bool,
    fail_drive: bool,
    fail_stop: bool,
}

unsafe extern "C" fn write_pair(context: *mut c_void, a: bool, b: bool) -> bool {
    // SAFETY: `context` is the fixture's leaked pins, unaliased during the callback.
    let pins = unsafe { &mut *context.cast::<Output>() };
    assert!(a || b, "no command may produce LOW/LOW");
    pins.writes += 1;
    if a && b {
        pins.stops += 1;
    }

    if if a && b {
        pins.fail_stop
    } else {
        pins.fail_drive
    } {
        return false;
    }
    pins.a = a;
    pins.b = b;
    true
}

/// The fixture's pins, read through the pointer between gate calls.
struct Pins(*mut Output);

impl MotorPins for Pins {
    fn write_pair(&mut self, a: bool, b: bool) -> bool {
        // SAFETY: every test keeps the pins alive for the gate's lifetime.
        unsafe { write_pair(self.0.cast(), a, b) }
    }
}

fn init(
    gate: &mut MotorGate<Pins>,
    pins: *mut Output,
    deadtime: u32,
    lease: u32,
    now: u32,
) -> bool {
    gate.init(Pins(pins), deadtime, lease, now)
}

/// Run `body` with a gate wired to fresh pins; pins are read through the pointer.
fn with_gate(body: impl FnOnce(&mut MotorGate<Pins>, *mut Output)) {
    let pins = Box::into_raw(Box::new(Output::default()));
    let mut gate = MotorGate::<Pins>::default();
    body(&mut gate, pins);
    // SAFETY: the box leaked when the fixture was built, freed once here.
    drop(unsafe { Box::from_raw(pins) });
}

/// Short-lived access between gate calls; never held across a callback.
fn pins(p: *mut Output) -> &'static mut Output {
    // SAFETY: the leaked pins outlive the fixture and nothing else borrows them here.
    unsafe { &mut *p }
}

#[test]
fn startup_and_reversal_wait_for_stop() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(pins(p).a && pins(p).b);

        assert!(gate.request(MotorCommand::Drive0, 0));
        assert!(gate.tick(2));
        assert!(pins(p).a && pins(p).b);
        assert!(gate.tick(3));
        assert!(pins(p).a && !pins(p).b);
        assert_eq!(gate.applied_at, 3);

        assert!(gate.request(MotorCommand::Drive0, 4));
        assert_eq!(gate.applied_at, 3); // Renewal is not a new drive transition.

        assert!(gate.request(MotorCommand::Drive1, 4));
        assert!(pins(p).a && pins(p).b);
        assert_eq!(gate.applied_at, 4);
        assert!(gate.tick(6));
        assert!(pins(p).a && pins(p).b);
        assert!(gate.tick(7));
        assert!(!pins(p).a && pins(p).b);
        assert_eq!(gate.applied_at, 7);
        assert_eq!(pins(p).stops, 2);
    });
}

#[test]
fn independent_tick_expires_a_stalled_owner() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::Drive0, 0));
        assert!(gate.tick(3));
        assert!(gate.tick(9));
        assert!(!gate.tick(10));
        assert!(pins(p).a && pins(p).b && gate.applied_known);
        assert_eq!(gate.fault, MotorGateFault::Expired);
        assert!(!gate.request(MotorCommand::Drive1, 11));
        assert!(pins(p).a && pins(p).b);
    });
}

#[test]
fn renewals_and_late_renewals() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::Drive0, 3));
        assert!(gate.request(MotorCommand::Drive0, 12));
        assert!(gate.tick(21));
        assert!(!gate.request(MotorCommand::Drive0, 22));
        assert_eq!(gate.fault, MotorGateFault::Expired);
        assert!(pins(p).a && pins(p).b);
    });
}

#[test]
fn cancelling_pending_drive_does_not_restart() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::Drive0, 0));
        assert!(gate.request(MotorCommand::Stop, 1));
        assert!(gate.tick(1000));
        assert!(pins(p).a && pins(p).b);
        assert_eq!(pins(p).writes, 1);
    });
}

#[test]
fn io_failure_stops_and_does_not_claim_verification() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        pins(p).fail_drive = true;
        assert!(!gate.request(MotorCommand::Drive0, 3));
        assert_eq!(gate.fault, MotorGateFault::Io);
        assert!(pins(p).a && pins(p).b && gate.applied_known);

        *pins(p) = Output::default();
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::Drive0, 3));

        pins(p).fail_stop = true;
        assert!(!gate.request(MotorCommand::Stop, 4));
        assert_eq!(gate.fault, MotorGateFault::Io);
        assert!(!gate.applied_known);

        pins(p).fail_stop = false;
        assert!(!gate.tick(5));
        assert!(gate.applied_known && pins(p).a && pins(p).b);
        assert_eq!(gate.fault, MotorGateFault::Io);
    });
}

#[test]
fn invalid_command_and_clock_wrap() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, u32::MAX - 1));
        assert!(gate.request(MotorCommand::Drive1, u32::MAX - 1));
        assert!(gate.tick(1));
        assert!(!pins(p).a && pins(p).b);
        assert!(!gate.tick(u32::MAX));
        assert_eq!(gate.fault, MotorGateFault::Clock);
        assert!(pins(p).a && pins(p).b);

        assert!(!init(gate, p, 10, 10, 0));
    });
}
