//! EXPERIMENTAL FreeRTOS feeding owner: durable reservation, preparation,
//! supervised feeding, result recording and cooperative shutdown.
//!
//! Starting it creates a task that WILL open GPIO/ADC/IRQs when run. The
//! normal diagnostic does not call it. Requires a verified board/recovery,
//! exclusive peripherals/ADC and an unused PMU customer slot (not SDIO 17).

use core::ffi::c_void;
use core::ptr;

use wellfed_core::feed_controller::{FeedController, FeedRequest};
use wellfed_core::feed_progress::BowlMode;
use wellfed_core::feed_storage::{OwnerRecovery, OwnerResult, OwnerStorage};
use wellfed_core::flash_coord::{FlashFault, FlashPhase};
use wellfed_core::sensor_inputs::SensorInputs;

use crate::active::active_pointer;
use crate::board::{self, Board};
use crate::events::{Events, EventsFault};
use crate::sdk::{self, IrqGuard};
use crate::watchdog::{Watchdog, WatchdogFault};

mod config;
mod state;

pub use config::OwnerConfig;

c_enum!(OwnerState {
    STARTING = 0,
    IDLE = 1,
    QUEUED = 2,
    PREPARING = 3,
    FEEDING = 4,
    REPORTING = 5,
    TERMINAL = 6,
    FAULT = 7,
    STOPPED = 8,
});
c_enum!(OwnerFault { OK = 0, SETUP = 1, RECOVERY = 2, STORAGE = 3, HARDWARE = 4, WATCHDOG = 5, FLASH = 6 });
c_enum!(SubmitResult {
    ENQUEUED = 0,
    REJECT_BUSY = 1,
    REJECT_ID = 2,
    REJECT_REQUEST = 3,
    REJECT_PHASE = 4,
    REJECT_STOPPED = 5,
});

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerStatus {
    pub state: OwnerState,
    pub fault: OwnerFault,
    pub supervisor_fault: EventsFault,
    pub watchdog_fault: WatchdogFault,
    pub flash_phase: FlashPhase,
    pub flash_fault: FlashFault,
    pub result: OwnerResult,
    pub high_water_id: u64,
    pub phase_known: bool,
    pub intent_reserved: bool,
    pub result_durable: bool,
    pub stopped: bool,
    pub optics_off: bool,
    pub watchdog_claimed: bool,
    pub watchdog_armed: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerMailbox {
    pub id: u64,
    pub portions: u16,
    pub queued_at: u32,
    pub has_request: bool,
    pub cancel: bool,
    pub cancel_id: u64,
    pub acknowledge: bool,
    pub stop: bool,
}

/// Caller-owned fixed-address storage; members private to the implementation.
/// Use status/command methods instead of reading/mutating fields concurrently.
#[repr(C)]
#[derive(Debug, Default)]
pub struct Owner {
    pub config: OwnerConfig,
    pub storage: OwnerStorage,
    pub board: Board,
    pub events: Events,
    pub watchdog: Watchdog,
    pub sensors: SensorInputs,
    pub controller: FeedController,

    pub recovery: OwnerRecovery,
    pub request: FeedRequest,
    pub result: OwnerResult,
    pub published: OwnerStatus,
    pub mailbox: OwnerMailbox,

    pub state: OwnerState,
    pub fault: OwnerFault,
    pub last_submitted_id: u64,
    pub watchdog_progress: u64,
    pub queued_at: u32,

    pub initialized: bool,
    pub board_attempted: bool,
    pub wake_owned: bool,
    pub reserved: bool,
    pub finish_attempted: bool,
    pub optics_started: bool,
    pub result_durable: bool,

    /// Initialization failed after the watchdog was armed (SETUP or RECOVERY).
    /// The fault stays latched; the watchdog is refreshed only while each
    /// iteration re-verifies STOP and infrared OFF, so a determined failure
    /// does not reset-loop while a hang or output loss still resets.
    pub init_failed: bool,
}

active_pointer!(Owner);

impl Owner {
    /// Starts the experimental owner task. Retain this object until status
    /// STOPPED; stop is cooperative, never forced.
    ///
    /// # Safety
    /// `config.flash` must be a live authority, and `storage` callbacks sound
    /// for their context, for as long as the owner runs and beyond STOPPED
    /// until global transport teardown. `self` must not move until STOPPED.
    pub unsafe fn start(&mut self, config: &OwnerConfig, storage: &OwnerStorage) -> bool {
        if !config.valid() || !storage.complete() {
            return false;
        }

        {
            let _guard = IrqGuard::lock();
            // SAFETY: the contract requires a live authority; null fails here.
            let fits =
                unsafe { config.flash.as_ref() }.is_some_and(|f| config.lease_fits(&f.limits));
            if !active().is_null() || board::outputs_blocked(config.flash) || !fits {
                return false;
            }
            *self = Self {
                config: *config,
                storage: *storage,
                state: OwnerState::STARTING,
                published: OwnerStatus {
                    state: OwnerState::STARTING,
                    ..OwnerStatus::default()
                },
                ..Self::default()
            };
            set_active(self);
        }

        if !sdk::task_create(
            c"feed-owner",
            2048,
            3,
            owner_step,
            ptr::from_mut(self).cast(),
        ) {
            let _guard = IrqGuard::lock();
            set_active(ptr::null_mut());
            self.state = OwnerState::STOPPED;
            self.published.state = OwnerState::STOPPED;
            self.published.fault = OwnerFault::SETUP;
            return false;
        }

        true
    }

    fn running(&self) -> bool {
        active().cast_const() == ptr::from_ref(self)
    }

    /// ENQUEUED is transport acceptance, not durable acceptance or dispensing.
    pub fn submit(&mut self, id: u64, portions: u16) -> SubmitResult {
        let _guard = IrqGuard::lock();
        if !self.running() || self.mailbox.stop || self.published.state == OwnerState::FAULT {
            return SubmitResult::REJECT_STOPPED;
        }
        if self.published.state != OwnerState::IDLE || self.mailbox.has_request {
            return SubmitResult::REJECT_BUSY;
        }
        if id == 0 || id <= self.last_submitted_id {
            return SubmitResult::REJECT_ID;
        }
        if portions == 0 || portions > self.config.feed.max_portions {
            return SubmitResult::REJECT_REQUEST;
        }
        if self.config.bowl_mode == BowlMode::DOUBLE && !self.published.phase_known {
            return SubmitResult::REJECT_PHASE;
        }

        let Some(value) = self.events.now_ms() else {
            return SubmitResult::REJECT_STOPPED;
        };

        self.mailbox.id = id;
        self.mailbox.portions = portions;
        self.mailbox.queued_at = value;
        self.mailbox.has_request = true;
        self.mailbox.cancel = false;
        self.last_submitted_id = id;
        SubmitResult::ENQUEUED
    }

    pub fn cancel(&mut self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let result = self.running()
            && ((self.mailbox.has_request && self.mailbox.id == id)
                || ((self.published.state == OwnerState::PREPARING
                    || self.published.state == OwnerState::FEEDING)
                    && self.published.result.id == id));
        if result {
            self.mailbox.cancel = true;
            self.mailbox.cancel_id = id;
        }
        result
    }

    pub fn acknowledge(&mut self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let result = self.running()
            && self.published.state == OwnerState::TERMINAL
            && self.published.result.id == id
            && self.published.result_durable;
        if result {
            self.mailbox.acknowledge = true;
        }
        result
    }

    pub fn stop(&mut self) {
        let _guard = IrqGuard::lock();
        if self.running() {
            self.mailbox.stop = true;
        }
    }

    pub fn status(&self) -> OwnerStatus {
        let _guard = IrqGuard::lock();
        let mut status = self.published;
        if self.mailbox.has_request && status.state == OwnerState::IDLE {
            status.state = OwnerState::QUEUED;
            status.result = OwnerResult {
                id: self.mailbox.id,
                requested: self.mailbox.portions,
                ..OwnerResult::default()
            };
            status.intent_reserved = false;
            status.result_durable = false;
        }
        status
    }
}

unsafe extern "C" fn owner_step(argument: *mut c_void) -> u32 {
    // SAFETY: `start` registered this fixed-address owner as the task context.
    let owner = argument.cast::<Owner>();
    if !unsafe { Owner::poll(owner) } {
        return unsafe { (*owner).config.sample_period_ms };
    }

    let _guard = IrqGuard::lock();
    set_active(ptr::null_mut());
    0
}
