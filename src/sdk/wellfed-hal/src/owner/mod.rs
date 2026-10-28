//! EXPERIMENTAL feeding owner: durable reservation, preparation,
//! supervised feeding, result recording and cooperative shutdown.
//!
//! Opening it claims nothing; its first step opens GPIO/ADC/IRQs. Requires
//! a verified board/recovery, exclusive peripherals/ADC and an unused PMU
//! customer slot (not SDIO 17).
//!
//! The owner has no task of its own: one firmware task steps it (the
//! [product architecture] puts it in the loop task, under the firmware's
//! task budget), and the first task to step it is the only one that may.
//! [`Owner`] is a shared handle in three parts, so that no borrow ever
//! spans a window in which another context reaches the same state:
//! - `submit`, `cancel`, `acknowledge`, `stop` and `status` take `&self` and
//!   exchange only the mailbox and the published status, inside `IrqGuard`;
//! - the board and its supervisor are handles of their own, which the
//!   stepping task and the timer and feedback interrupts reach through `&`;
//! - the rest is [`OwnerTask`], which only a step borrows.
//!
//! The runtime owns the hardware watchdog. The owner publishes a
//! [`beat`](crate::supervision::beat) wherever it would have refreshed it,
//! and faults itself when its own beat falls overdue.
//!
//! [product architecture]: ../../../../../docs/firmware/product-architecture.md

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use wellfed_core::feed::controller::{FeedController, FeedReason, FeedRequest};
use wellfed_core::feed::observe::Observation;
use wellfed_core::feed::progress::BowlMode;
use wellfed_core::feed::sensor_inputs::SensorInputs;
use wellfed_core::feed::storage::{OwnerRecovery, OwnerResult, OwnerStorage, RotorPhase};
use wellfed_core::flash_coord::{FlashFault, FlashPhase};

use crate::board::{self, Board};
use crate::sdk::{self, IrqGuard};
use crate::supervision::active::active_pointer;
use crate::supervision::beat;
use crate::supervision::events::{EDGE_LOG, Events, EventsFault, PinWatch};

mod config;
mod state;

pub use config::OwnerConfig;

pub use wellfed_core::feed::owner::{OwnerFault, OwnerState};

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

/// What one [`Owner::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Step again within this many milliseconds.
    Wait(u32),
    /// The owner is not running: never opened, or stopped.
    Stopped,
    /// Another task steps this owner.
    Refused,
}

/// What one feed's drive measured, for the bench: every feedback edge,
/// qualified or not, and the motor current, so a run shows the mechanism's
/// behaviour before the limits are tuned to it. Reset when a request is
/// taken; complete once the result is reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedTrace {
    /// How long the motor was commanded to move; 0 when it never started.
    pub drive_ms: u32,
    pub rising: u16,
    pub falling: u16,
    /// From the drive's start to the first feedback edge.
    pub first_edge_ms: Option<u32>,
    /// The scaled current channel (stock's unit) while driving.
    pub current_max: u32,
    pub current_sum: u32,
    pub current_samples: u16,
    /// Times the drive backed off a jam.
    pub reversals: u8,
    /// The food beam during this feed: `Some(false)` once it read empty,
    /// `Some(true)` while it has read only food, `None` before it
    /// qualified a reading. A feed never waits on it, so a short or
    /// cancelled one can end unread.
    pub hopper: Option<bool>,
    /// Each logged edge, from the drive's start (0 if it predates it), and
    /// the level after it.
    pub edge_ms: [Option<(u32, bool)>; EDGE_LOG],
    /// Every pad that changed while capture was on.
    pub pins: PinWatch,
    /// From the drive's start to the homing mark, when the feed started
    /// with the rotor's phase unknown and reached a mark inside the
    /// inhibit window ([`FeedRequest::homing`]).
    pub homed_ms: Option<u32>,
}

impl FeedTrace {
    pub fn current_mean(&self) -> Option<u32> {
        (self.current_samples != 0).then(|| self.current_sum / u32::from(self.current_samples))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerStatus {
    pub state: OwnerState,
    pub fault: OwnerFault,
    pub supervisor_fault: EventsFault,
    pub flash_phase: FlashPhase,
    pub flash_fault: FlashFault,
    pub result: OwnerResult,
    pub high_water_id: u64,
    pub phase_known: bool,
    /// Where the rotor rests: the next feed homes it while unknown.
    pub phase: RotorPhase,
    pub intent_reserved: bool,
    pub result_durable: bool,
    pub stopped: bool,
    pub optics_off: bool,
    pub trace: FeedTrace,
    /// The food beam's latest qualified reading (`true` with food), kept
    /// after the emitters go off; `None` until a feed has lit them.
    pub food: Option<bool>,
    /// The pack against its floor (`SensorInputs::floor`), `None` before
    /// the owner has judged it.
    pub below_floor: Option<bool>,
    /// The supply keeps a feed from starting (`SensorInputs::supply_low`):
    /// on the pack, and the pack not judged above its floor.
    pub supply_low: bool,
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
    /// The supervisor's clock at the last step, while it reads: what
    /// `submit` stamps a request with. At most one step old, so the prepare
    /// timeout it starts can only end early.
    clock_ms: Option<u32>,
    last_submitted_id: u64,
    /// Copied from the configuration at open: what `submit` checks.
    max_portions: u16,
    bowl_mode: BowlMode,
}

/// The feeding owner: fixed-address storage the caller keeps in place from
/// `open` until STOPPED, and the handle every commanding task shares.
#[derive(Debug)]
pub struct Owner<S> {
    /// The stepping task's identity; zero until the first step.
    stepper: AtomicUsize,
    shared: UnsafeCell<Shared>,
    board: Board,
    events: Events,
    task: UnsafeCell<OwnerTask<S>>,
}

// SAFETY: the commands touch only `shared`, inside `IrqGuard`, which on the
// single KM4 core excludes every other task and interrupt; the board and
// supervisor are reached only through `&`, and serialize themselves; only
// the one stepping task borrows `task`, and the storage that `open` moves
// into it must be able to cross to that task.
unsafe impl<S: Send> Sync for Owner<S> {}

impl<S> Default for Owner<S> {
    fn default() -> Self {
        Self {
            stepper: AtomicUsize::new(0),
            shared: UnsafeCell::default(),
            board: Board::default(),
            events: Events::default(),
            task: UnsafeCell::default(),
        }
    }
}

/// The owner task's state: the controller, sensors and journal progress.
/// Only a step borrows it.
#[derive(Debug)]
pub struct OwnerTask<S> {
    pub config: OwnerConfig,
    /// The durable journal, moved in by `open`.
    pub storage: Option<S>,
    pub sensors: SensorInputs,
    pub controller: FeedController,

    pub recovery: OwnerRecovery,
    /// Where the rotor rests between feeds.
    pub phase: RotorPhase,
    pub request: FeedRequest,
    pub result: OwnerResult,
    pub trace: FeedTrace,
    /// The supervisor's clock when the drive started, until it is reported.
    pub drive_at: Option<u32>,
    /// What the last poll of a feed saw, until the stepping task takes it.
    pub observation: Option<Observation>,

    pub state: OwnerState,
    pub fault: OwnerFault,
    /// SYSTIMER at the last beat, or at open.
    pub beat_raw: u32,
    /// A beat came due after the gap had passed; latched, so no later beat
    /// is ever published.
    pub beat_lost: bool,
    pub queued_at: u32,

    pub initialized: bool,
    pub board_attempted: bool,
    pub wake_owned: bool,
    pub reserved: bool,
    pub finish_attempted: bool,
    pub optics_started: bool,
    pub result_durable: bool,

    /// A determined failure the owner holds rather than resets on:
    /// initialization (SETUP or RECOVERY) after the beat began, or a sensor
    /// read (SENSOR). The fault stays latched; the owner beats only while
    /// each iteration re-verifies STOP and infrared OFF, so a determined
    /// failure does not reset-loop while a hang or output loss still resets.
    pub held: bool,
    /// Why the request being prepared could not start yet: what it expires
    /// with if it never does.
    pub blocked_by: FeedReason,
    /// The food beam's latest qualified reading.
    pub food: Option<bool>,

    /// The owner's other parts, set by `open`.
    shared: *mut Shared,
    board: *const Board,
    events: *const Events,
}

impl<S> Default for OwnerTask<S> {
    fn default() -> Self {
        Self {
            config: OwnerConfig::default(),
            storage: None,
            sensors: SensorInputs::default(),
            controller: FeedController::default(),
            recovery: OwnerRecovery::default(),
            phase: RotorPhase::default(),
            request: FeedRequest::default(),
            result: OwnerResult::default(),
            trace: FeedTrace::default(),
            drive_at: None,
            observation: None,
            state: OwnerState::default(),
            fault: OwnerFault::default(),
            beat_raw: 0,
            beat_lost: false,
            queued_at: 0,
            initialized: false,
            board_attempted: false,
            wake_owned: false,
            reserved: false,
            finish_attempted: false,
            optics_started: false,
            result_durable: false,
            held: false,
            blocked_by: FeedReason::None,
            food: None,
            shared: ptr::null_mut(),
            board: ptr::null(),
            events: ptr::null(),
        }
    }
}

// The running owner, type-erased: the pointer only admits one owner at a
// time and tells an owner whether it is the one running.
active_pointer!(());

impl<S: OwnerStorage> Owner<S> {
    /// Opens the owner for stepping. Retain this object until status
    /// STOPPED; stop is cooperative, never forced.
    ///
    /// # Safety
    /// `config.flash` must be a live authority for as long as the owner runs
    /// and beyond STOPPED until global transport teardown. `self` must not
    /// move until STOPPED.
    pub unsafe fn open(&self, config: &OwnerConfig, storage: S) -> bool {
        if !config.valid() {
            return false;
        }

        let _guard = IrqGuard::lock();
        // SAFETY: the contract requires a live authority; null fails here.
        let fits = unsafe { config.flash.as_ref() }.is_some_and(|f| config.lease_fits(&f.limits));
        if !active().is_null() || board::outputs_blocked(config.flash) || !fits {
            return false;
        }

        // SAFETY: no owner runs (`active` is null), so no step borrows the
        // task half; the guard excludes the commands.
        unsafe {
            *self.task.get() = OwnerTask {
                config: *config,
                storage: Some(storage),
                state: OwnerState::Starting,
                beat_raw: sdk::systimer_ticks(),
                shared: self.shared.get(),
                board: &raw const self.board,
                events: &raw const self.events,
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
        beat::open(sdk::uptime_ms(), config.idle_gap_ms);
        set_active(ptr::from_ref(self).cast_mut().cast());
        true
    }

    /// One owner iteration, for the one task that steps it: the first to
    /// call is that task, and a call from any other is refused.
    pub fn step(&self) -> Step {
        if !self.running() {
            return Step::Stopped;
        }
        let task = sdk::current_task();
        let stepper =
            match self
                .stepper
                .compare_exchange(0, task, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(_) => task,
                Err(stepper) => stepper,
            };
        if task == 0 || stepper != task {
            return Step::Refused;
        }

        let task = self.task.get();
        // SAFETY: the owner is running and this is its one stepping task, so
        // nothing else borrows the task half; `poll` borrows it afresh for
        // each of its own steps.
        if !unsafe { OwnerTask::poll(task) } {
            // SAFETY: as above; `poll` holds no borrow once it returns.
            return Step::Wait(unsafe { (*task).config.sample_period_ms });
        }

        let _guard = IrqGuard::lock();
        beat::close();
        set_active(ptr::null_mut());
        Step::Stopped
    }

    /// What the last step saw of a feed being prepared or driven, taken
    /// once, for the task that steps the owner (`None` to any other). It
    /// is read between steps, so whatever keeps a trace of it runs after
    /// the step's stop and motor decisions, never inside them.
    pub fn take_observation(&self) -> Option<Observation> {
        let task = sdk::current_task();
        if task == 0 || self.stepper.load(Ordering::Acquire) != task {
            return None;
        }
        // SAFETY: this is the one stepping task and no step is running on
        // it, so nothing else borrows the task half.
        unsafe { (*self.task.get()).observation.take() }
    }
}

impl<S> Owner<S> {
    /// Opened and not yet stepped to STOPPED.
    pub fn running(&self) -> bool {
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
        // An install holds the flash (or the feeder is about to restart into
        // bank 0 or the new bank 1): no feed starts until it lets go.
        if shared.published.state != OwnerState::Idle
            || shared.mailbox.has_request
            || crate::ota::mailbox::installing()
        {
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
        let Some(value) = shared.clock_ms else {
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

    /// The board the owner drives, for the host tests and the virtual
    /// feeder, which stand in for the flash peer between steps.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The owner's supervisor, for the same callers as `board`.
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// The owner task's half, for the host tests, which stand in for the
    /// hardware between steps. Never dereference it during a step.
    #[doc(hidden)]
    pub fn task_state(&self) -> *mut OwnerTask<S> {
        self.task.get()
    }
}
