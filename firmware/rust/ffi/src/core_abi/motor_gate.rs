//! Exports declared by `motor_gate.h`.

use core::ffi::c_void;

use petlibre_core::feed_controller::MotorCommand;
use petlibre_core::motor_gate::{MotorGate, MotorWritePair};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_init(
    gate: *mut MotorGate,
    write_pair: Option<MotorWritePair>,
    context: *mut c_void,
    deadtime_ms: u32,
    lease_ms: u32,
    now_ms: u32,
) -> bool {
    let Some(gate) = (unsafe { gate.as_mut() }) else {
        return false;
    };
    // SAFETY: the C contract keeps the callback valid for `context`.
    unsafe { gate.init(write_pair, context, deadtime_ms, lease_ms, now_ms) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_request(
    gate: *mut MotorGate,
    command: MotorCommand,
    now_ms: u32,
) -> bool {
    unsafe { &mut *gate }.request(command, now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_tick(gate: *mut MotorGate, now_ms: u32) -> bool {
    unsafe { &mut *gate }.tick(now_ms)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_motor_gate_trip(gate: *mut MotorGate, now_ms: u32) {
    unsafe { &mut *gate }.trip(now_ms);
}
