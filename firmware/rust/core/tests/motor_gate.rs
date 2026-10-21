use core::ffi::c_void;

use petlibro_core::feed_controller::MotorCommand;
use petlibro_core::motor_gate::{MotorGate, MotorGateFault};

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

fn init(gate: &mut MotorGate, pins: *mut Output, deadtime: u32, lease: u32, now: u32) -> bool {
    // SAFETY: every test keeps `pins` alive for the gate's lifetime.
    unsafe { gate.init(Some(write_pair), pins.cast(), deadtime, lease, now) }
}

/// Run `body` with a gate wired to fresh pins; pins are read through the pointer.
fn with_gate(body: impl FnOnce(&mut MotorGate, *mut Output)) {
    let pins = Box::into_raw(Box::new(Output::default()));
    let mut gate = MotorGate::default();
    body(&mut gate, pins);
    drop(unsafe { Box::from_raw(pins) });
}

/// Short-lived access between gate calls; never held across a callback.
fn pins(p: *mut Output) -> &'static mut Output {
    unsafe { &mut *p }
}

#[test]
fn startup_and_reversal_wait_for_stop() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(pins(p).a && pins(p).b);
        assert!(gate.request(MotorCommand::DRIVE_0, 0));
        assert!(gate.tick(2));
        assert!(pins(p).a && pins(p).b);
        assert!(gate.tick(3));
        assert!(pins(p).a && !pins(p).b);
        assert_eq!(gate.applied_at, 3);
        assert!(gate.request(MotorCommand::DRIVE_0, 4));
        assert_eq!(gate.applied_at, 3); // Renewal is not a new drive transition.
        assert!(gate.request(MotorCommand::DRIVE_1, 4));
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
        assert!(gate.request(MotorCommand::DRIVE_0, 0));
        assert!(gate.tick(3));
        assert!(gate.tick(9));
        assert!(!gate.tick(10));
        assert!(pins(p).a && pins(p).b && gate.applied_known);
        assert_eq!(gate.fault, MotorGateFault::EXPIRED);
        assert!(!gate.request(MotorCommand::DRIVE_1, 11));
        assert!(pins(p).a && pins(p).b);
    });
}

#[test]
fn renewals_and_late_renewals() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::DRIVE_0, 3));
        assert!(gate.request(MotorCommand::DRIVE_0, 12));
        assert!(gate.tick(21));
        assert!(!gate.request(MotorCommand::DRIVE_0, 22));
        assert_eq!(gate.fault, MotorGateFault::EXPIRED);
        assert!(pins(p).a && pins(p).b);
    });
}

#[test]
fn cancelling_pending_drive_does_not_restart() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::DRIVE_0, 0));
        assert!(gate.request(MotorCommand::STOP, 1));
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
        assert!(!gate.request(MotorCommand::DRIVE_0, 3));
        assert_eq!(gate.fault, MotorGateFault::IO);
        assert!(pins(p).a && pins(p).b && gate.applied_known);
        *pins(p) = Output::default();
        assert!(init(gate, p, 3, 10, 0));
        assert!(gate.request(MotorCommand::DRIVE_0, 3));
        pins(p).fail_stop = true;
        assert!(!gate.request(MotorCommand::STOP, 4));
        assert_eq!(gate.fault, MotorGateFault::IO);
        assert!(!gate.applied_known);
        pins(p).fail_stop = false;
        assert!(!gate.tick(5));
        assert!(gate.applied_known && pins(p).a && pins(p).b);
        assert_eq!(gate.fault, MotorGateFault::IO);
    });
}

#[test]
fn invalid_command_and_clock_wrap() {
    with_gate(|gate, p| {
        assert!(init(gate, p, 3, 10, u32::MAX - 1));
        assert!(gate.request(MotorCommand::DRIVE_1, u32::MAX - 1));
        assert!(gate.tick(1));
        assert!(!pins(p).a && pins(p).b);
        assert!(!gate.tick(u32::MAX));
        assert_eq!(gate.fault, MotorGateFault::CLOCK);
        assert!(pins(p).a && pins(p).b);
        assert!(init(gate, p, 3, 10, 0));
        assert!(!gate.request(MotorCommand(99), 0));
        assert_eq!(gate.fault, MotorGateFault::COMMAND);
        assert!(pins(p).a && pins(p).b);
        assert!(!init(gate, p, 10, 10, 0));
    });
}
