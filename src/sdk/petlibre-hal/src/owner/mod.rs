//! EXPERIMENTAL FreeRTOS feeding owner: durable reservation, preparation,
//! supervised feeding, result recording and cooperative shutdown.
//!
//! Starting it creates a task that WILL open GPIO/ADC/IRQs when run. The
//! normal diagnostic does not call it. Requires a verified board/recovery,
//! exclusive peripherals/ADC and an unused PMU customer slot (not SDIO 17).
//!
//! [`Owner`] is the handle the commanding tasks share: `submit`, `cancel`,
//! `acknowledge`, `stop` and `status` take `&self` and exchange only the
//! mailbox and the published status, inside `IrqGuard`. The rest is
//! [`OwnerTask`], which only the `feed-owner` task borrows, so a command
//! never aliases the feeding state. Known limit: the supervision and board
//! state inside it are also reached, inside `IrqGuard`, by the timer and
//! feedback interrupts (and `submit` reads the event clock the same way)
//! while the owner task may hold a borrow across an interrupt-enabled window.
//!
//! The product firmware will not give the owner a task of its own: the
//! [product architecture] steps it from the loop task, under the two-task
//! firmware budget, and the loop commands it through this same handle.
//! Making it steppable without `feed-owner` is the feeding card's work.
//!
//! [product architecture]: ../../../../../docs/firmware/product-architecture.md

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr;

use petlibre_core::feed::controller::{FeedController, FeedRequest};
use petlibre_core::feed::progress::BowlMode;
use petlibre_core::feed::sensor_inputs::SensorInputs;
use petlibre_core::feed::storage::{OwnerRecovery, OwnerResult, OwnerStorage};
use petlibre_core::flash_coord::{FlashFault, FlashPhase};

use crate::board::{self, Board};
use crate::sdk::{self, IrqGuard};
use crate::supervision::active::active_pointer;
use crate::supervision::events::{Events, EventsFault};
use crate::supervision::watchdog::{Watchdog, WatchdogFault};

mod config;
mod state;

pub use config::OwnerConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OwnerState {
    #[default]
    Starting = 0,
    Idle = 1,
    Queued = 2,
    Preparing = 3,
    Feeding = 4,
    Reporting = 5,
    Terminal = 6,
    Fault = 7,
    Stopped = 8,
}

impl OwnerState {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum OwnerFault {
    #[default]
    Ok = 0,
    Setup = 1,
    Recovery = 2,
    Storage = 3,
    Hardware = 4,
    Watchdog = 5,
    Flash = 6,
}

impl OwnerFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SubmitResult {
    #[default]
    Enqueued = 0,
    RejectBusy = 1,
    RejectId = 2,
    RejectRequest = 3,
    RejectPhase = 4,
    RejectStopped = 5,
}

impl SubmitResult {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

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

/// What the commanding tasks and the owner task exchange. Reached only
/// inside `IrqGuard` and never borrowed past it, by `Owner`'s commands and by
/// `OwnerTask::shared`.
#[derive(Debug, Default)]
struct Shared {
    mailbox: OwnerMailbox,
    published: OwnerStatus,
    last_submitted_id: u64,
    /// Copied from the configuration at start: what `submit` checks.
    max_portions: u16,
    bowl_mode: BowlMode,
}

/// The feeding owner: fixed-address storage the caller keeps in place from
/// `start` until STOPPED, and the handle every commanding task shares.
#[derive(Debug)]
pub struct Owner<S> {
    shared: UnsafeCell<Shared>,
    task: UnsafeCell<OwnerTask<S>>,
}

// SAFETY: the commands touch only `shared`, inside `IrqGuard`, which on the
// single KM4 core excludes every other task and interrupt; only the owner
// task borrows `task` (see the module's known limit), and the storage that
// `start` moves into it must be able to cross to that task.
unsafe impl<S: Send> Sync for Owner<S> {}

impl<S> Default for Owner<S> {
    fn default() -> Self {
        Self {
            shared: UnsafeCell::default(),
            task: UnsafeCell::default(),
        }
    }
}

/// The owner task's state: the board, supervision, controller and journal
/// progress. Only the `feed-owner` task borrows it.
#[derive(Debug)]
pub struct OwnerTask<S> {
    pub config: OwnerConfig,
    /// The durable journal, moved in by `start`.
    pub storage: Option<S>,
    pub board: Board,
    pub events: Events,
    pub watchdog: Watchdog,
    pub sensors: SensorInputs,
    pub controller: FeedController,

    pub recovery: OwnerRecovery,
    pub request: FeedRequest,
    pub result: OwnerResult,

    pub state: OwnerState,
    pub fault: OwnerFault,
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

    /// The owner's shared half, set by `start`.
    shared: *mut Shared,
}

impl<S> Default for OwnerTask<S> {
    fn default() -> Self {
        Self {
            config: OwnerConfig::default(),
            storage: None,
            board: Board::default(),
            events: Events::default(),
            watchdog: Watchdog::default(),
            sensors: SensorInputs::default(),
            controller: FeedController::default(),
            recovery: OwnerRecovery::default(),
            request: FeedRequest::default(),
            result: OwnerResult::default(),
            state: OwnerState::default(),
            fault: OwnerFault::default(),
            watchdog_progress: 0,
            queued_at: 0,
            initialized: false,
            board_attempted: false,
            wake_owned: false,
            reserved: false,
            finish_attempted: false,
            optics_started: false,
            result_durable: false,
            init_failed: false,
            shared: ptr::null_mut(),
        }
    }
}

// The running owner, type-erased: the pointer only admits one owner at a
// time and tells an owner whether it is the one running.
active_pointer!(());

impl<S: OwnerStorage> Owner<S> {
    /// Starts the experimental owner task. Retain this object until status
    /// STOPPED; stop is cooperative, never forced.
    ///
    /// # Safety
    /// `config.flash` must be a live authority for as long as the owner runs
    /// and beyond STOPPED until global transport teardown. `self` must not
    /// move until STOPPED.
    pub unsafe fn start(&self, config: &OwnerConfig, storage: S) -> bool {
        if !config.valid() {
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

            // SAFETY: no owner task runs (`active` is null), so nothing
            // borrows either half; the guard excludes the commands.
            unsafe {
                *self.task.get() = OwnerTask {
                    config: *config,
                    storage: Some(storage),
                    state: OwnerState::Starting,
                    shared: self.shared.get(),
                    ..OwnerTask::default()
                };
                *self.shared.get() = Shared {
                    published: OwnerStatus {
                        state: OwnerState::Starting,
                        ..OwnerStatus::default()
                    },
                    max_portions: config.feed.max_portions,
                    bowl_mode: config.bowl_mode,
                    ..Shared::default()
                };
            }
            set_active(ptr::from_ref(self).cast_mut().cast());
        }

        // SAFETY: `owner_step` takes this owner, which the caller's contract
        // keeps in place until STOPPED.
        if !unsafe {
            sdk::task_create(
                c"feed-owner",
                2048,
                3,
                owner_step::<S>,
                ptr::from_ref(self).cast_mut().cast(),
            )
        } {
            let _guard = IrqGuard::lock();
            set_active(ptr::null_mut());
            // SAFETY: no owner task was created; the guard excludes the commands.
            unsafe {
                (*self.task.get()).state = OwnerState::Stopped;
                let shared = &mut *self.shared.get();
                shared.published.state = OwnerState::Stopped;
                shared.published.fault = OwnerFault::Setup;
            }
            return false;
        }

        true
    }
}

impl<S> Owner<S> {
    fn running(&self) -> bool {
        active().cast_const() == ptr::from_ref(self).cast()
    }

    /// ENQUEUED is transport acceptance, not durable acceptance or dispensing.
    pub fn submit(&self, id: u64, portions: u16) -> SubmitResult {
        let _guard = IrqGuard::lock();
        let running = self.running();
        // SAFETY: inside the guard, the only borrow of the shared half.
        let shared = unsafe { &mut *self.shared.get() };
        if !running || shared.mailbox.stop || shared.published.state == OwnerState::Fault {
            return SubmitResult::RejectStopped;
        }
        if shared.published.state != OwnerState::Idle || shared.mailbox.has_request {
            return SubmitResult::RejectBusy;
        }
        if id == 0 || id <= shared.last_submitted_id {
            return SubmitResult::RejectId;
        }
        if portions == 0 || portions > shared.max_portions {
            return SubmitResult::RejectRequest;
        }
        if shared.bowl_mode == BowlMode::Double && !shared.published.phase_known {
            return SubmitResult::RejectPhase;
        }

        // SAFETY: the event clock, read inside the guard as the timer and
        // feedback interrupts read it (the module's known limit).
        let Some(value) = (unsafe { (*self.task.get()).events.now_ms() }) else {
            return SubmitResult::RejectStopped;
        };

        shared.mailbox.id = id;
        shared.mailbox.portions = portions;
        shared.mailbox.queued_at = value;
        shared.mailbox.has_request = true;
        shared.mailbox.cancel = false;
        shared.last_submitted_id = id;
        SubmitResult::Enqueued
    }

    pub fn cancel(&self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let running = self.running();
        // SAFETY: as for `submit`.
        let shared = unsafe { &mut *self.shared.get() };
        let result = running
            && ((shared.mailbox.has_request && shared.mailbox.id == id)
                || ((shared.published.state == OwnerState::Preparing
                    || shared.published.state == OwnerState::Feeding)
                    && shared.published.result.id == id));
        if result {
            shared.mailbox.cancel = true;
            shared.mailbox.cancel_id = id;
        }
        result
    }

    pub fn acknowledge(&self, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        let _guard = IrqGuard::lock();
        let running = self.running();
        // SAFETY: as for `submit`.
        let shared = unsafe { &mut *self.shared.get() };
        let result = running
            && shared.published.state == OwnerState::Terminal
            && shared.published.result.id == id
            && shared.published.result_durable;
        if result {
            shared.mailbox.acknowledge = true;
        }
        result
    }

    pub fn stop(&self) {
        let _guard = IrqGuard::lock();
        if self.running() {
            // SAFETY: as for `submit`.
            unsafe { (*self.shared.get()).mailbox.stop = true };
        }
    }

    pub fn status(&self) -> OwnerStatus {
        let _guard = IrqGuard::lock();
        // SAFETY: as for `submit`.
        let shared = unsafe { &*self.shared.get() };
        let mut status = shared.published;
        if shared.mailbox.has_request && status.state == OwnerState::Idle {
            status.state = OwnerState::Queued;
            status.result = OwnerResult {
                id: shared.mailbox.id,
                requested: shared.mailbox.portions,
                ..OwnerResult::default()
            };
            status.intent_reserved = false;
            status.result_durable = false;
        }
        status
    }

    /// The owner task's half, for the host tests and the virtual feeder,
    /// which stand in for the hardware and the flash peer between task
    /// iterations. Never dereference it while the owner task runs.
    #[doc(hidden)]
    pub fn task_state(&self) -> *mut OwnerTask<S> {
        self.task.get()
    }
}

unsafe extern "C" fn owner_step<S: OwnerStorage>(argument: *mut c_void) -> u32 {
    // `start` registered this fixed-address owner as the task context.
    let owner = argument.cast_const().cast::<Owner<S>>();
    // SAFETY: the started owner; only this task borrows its task half.
    let task = unsafe { (*owner).task.get() };
    // SAFETY: as above.
    if !unsafe { OwnerTask::poll(task) } {
        // SAFETY: as above; `poll` holds no borrow once it returns.
        return unsafe { (*task).config.sample_period_ms };
    }

    let _guard = IrqGuard::lock();
    set_active(ptr::null_mut());
    0
}
