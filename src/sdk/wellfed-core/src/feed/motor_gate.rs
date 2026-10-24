//! Motor output with deadtime, a renewable command lease and latched faults.
//!
//! Drives the PA13/PA14 motor pair. STOP is HIGH/HIGH (driver brake, as
//! stock does); a change of direction always passes through STOP for at
//! least `deadtime_ms`, and a drive command expires unless renewed within
//! `lease_ms`, which an independent timer tick enforces while the owner
//! stalls. Any fault latches STOP.

use core::ffi::c_void;
use core::ptr;

use crate::HALF_RANGE;
use crate::feed::controller::MotorCommand;

/// Update both pins coherently and report whether the result verified.
/// HIGH/HIGH is STOP.
pub type MotorWritePair =
    unsafe extern "C" fn(context: *mut c_void, pa13_high: bool, pa14_high: bool) -> bool;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum MotorGateFault {
    #[default]
    Ok = 0,
    Io = 1,
    Clock = 2,
    Expired = 3,
    Command = 4,
}

impl MotorGateFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MotorGate {
    pub write_pair: Option<MotorWritePair>,
    pub context: *mut c_void,

    pub deadtime_ms: u32,
    pub lease_ms: u32,

    pub stopped_at: u32,
    pub renewed_at: u32,
    pub observed_at: u32,
    /// Timestamp of the last successfully verified output transition.
    pub applied_at: u32,

    pub desired: MotorCommand,
    pub applied: MotorCommand,
    pub fault: MotorGateFault,
    pub applied_known: bool,
}

impl Default for MotorGate {
    fn default() -> Self {
        Self {
            write_pair: None,
            context: ptr::null_mut(),
            deadtime_ms: 0,
            lease_ms: 0,
            stopped_at: 0,
            renewed_at: 0,
            observed_at: 0,
            applied_at: 0,
            desired: MotorCommand::Stop,
            applied: MotorCommand::Stop,
            fault: MotorGateFault::Ok,
            applied_known: false,
        }
    }
}

impl MotorGate {
    /// Hardware must already have an inactive startup arrangement. A valid init
    /// writes/checks HIGH/HIGH once; invalid arguments do not touch outputs or
    /// `self`. All calls, including an independent timer tick, require
    /// serialized access.
    ///
    /// # Safety
    /// `write_pair` must be sound to call with `context` for as long as this
    /// gate (or any copy of it) is used.
    #[allow(unsafe_code)]
    pub unsafe fn init(
        &mut self,
        write_pair: Option<MotorWritePair>,
        context: *mut c_void,
        deadtime_ms: u32,
        lease_ms: u32,
        now_ms: u32,
    ) -> bool {
        if write_pair.is_none()
            || deadtime_ms == 0
            || lease_ms >= HALF_RANGE
            || lease_ms <= deadtime_ms
        {
            return false;
        }

        *self = Self {
            write_pair,
            context,
            deadtime_ms,
            lease_ms,
            observed_at: now_ms,
            renewed_at: now_ms,
            ..Self::default()
        };

        if !self.stop_output(now_ms) {
            self.fault = MotorGateFault::Io;
            return false;
        }
        true
    }

    #[allow(unsafe_code)]
    fn write(&self, pa13_high: bool, pa14_high: bool) -> bool {
        match self.write_pair {
            // SAFETY: `init`'s contract makes the callback valid for `context`.
            Some(write_pair) => unsafe { write_pair(self.context, pa13_high, pa14_high) },
            None => false,
        }
    }

    fn stop_output(&mut self, now: u32) -> bool {
        let result = self.write(true, true);
        self.applied_known = result;
        if result {
            self.applied = MotorCommand::Stop;
            self.stopped_at = now;
            self.applied_at = now;
        }
        result
    }

    fn fail(&mut self, fault: MotorGateFault, now: u32) -> bool {
        self.desired = MotorCommand::Stop;
        self.fault = fault;
        if !self.stop_output(now) {
            self.fault = MotorGateFault::Io;
        }
        false
    }

    fn observe(&mut self, now: u32) -> bool {
        if self.fault != MotorGateFault::Ok {
            return false;
        }

        let gap = now.wrapping_sub(self.observed_at);
        self.observed_at = now;

        if self.desired != MotorCommand::Stop {
            if gap >= HALF_RANGE {
                return self.fail(MotorGateFault::Clock, now);
            }
            if now.wrapping_sub(self.renewed_at) >= self.lease_ms {
                return self.fail(MotorGateFault::Expired, now);
            }
        } else if gap >= HALF_RANGE {
            // No active motor: rebase an ambiguous idle clock, then wait deadtime.
            self.stopped_at = now;
        }
        true
    }

    /// Latch an external peripheral failure and attempt STOP immediately.
    pub fn trip(&mut self, now_ms: u32) {
        self.fail(MotorGateFault::Io, now_ms);
    }

    /// Tick from an independent timer to expire commands while the owner is
    /// stalled. Faults latch STOP; `applied_known == false` means output
    /// verification failed. This cannot enforce a stop if the timer/CPU path
    /// also stops running.
    pub fn tick(&mut self, now_ms: u32) -> bool {
        if self.fault != MotorGateFault::Ok {
            if !self.applied_known {
                self.stop_output(now_ms);
            }
            return false;
        }

        if !self.observe(now_ms) {
            return false;
        }
        if self.desired == MotorCommand::Stop {
            return true;
        }
        if !self.applied_known {
            return self.fail(MotorGateFault::Io, now_ms);
        }

        if self.applied == MotorCommand::Stop
            && now_ms.wrapping_sub(self.stopped_at) >= self.deadtime_ms
        {
            let a = self.desired == MotorCommand::Drive0;
            if !self.write(a, !a) {
                return self.fail(MotorGateFault::Io, now_ms);
            }
            self.applied = self.desired;
            self.applied_known = true;
            self.applied_at = now_ms;
        }
        true
    }

    /// Renew a command; an already expired command cannot be revived by renewal.
    pub fn request(&mut self, command: MotorCommand, now_ms: u32) -> bool {
        if !self.observe(now_ms) {
            // A STOP is always attempted, even when an earlier fault is latched.
            if command == MotorCommand::Stop && !self.stop_output(now_ms) {
                self.fault = MotorGateFault::Io;
            }
            return false;
        }

        self.desired = command;
        self.renewed_at = now_ms;
        // Stopping or reversing goes through STOP first, unless STOP is already verified.
        if (command == MotorCommand::Stop
            || (self.applied != MotorCommand::Stop && self.applied != command))
            && (!self.applied_known || self.applied != MotorCommand::Stop)
            && !self.stop_output(now_ms)
        {
            return self.fail(MotorGateFault::Io, now_ms);
        }

        self.tick(now_ms)
    }
}
