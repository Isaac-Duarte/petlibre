//! Deterministic host model of the SDK calls, ported from the C test fake.
//!
//! Each thread owns independent hardware so tests can run in parallel. The
//! model asserts how the ports drive the SDK (pins, priorities, task shape),
//! models coalesced timer interrupts and GPIO edges, and runs registered
//! interrupt handlers synchronously when interrupts are unmasked. As on the
//! NVIC, an interrupt raised while masked or disabled stays pending (one
//! coalesced pend per line) and runs as soon as it is unmasked and enabled.
//!
//! Pin reads model the pad input path (EXT_PORT), not the output latch: an
//! output pad normally follows its latch, except pads in `dead_input`, which
//! read LOW whatever is driven (PA0-PA7 on the tested unit).

use core::ffi::{CStr, c_void};
use std::cell::RefCell;

use super::{GpioRouting, Irq, Isr, LpClock, TaskStep, WifiState};

const MOTOR_MASK: u32 = (1 << 13) | (1 << 14);
pub const GPIOA_IRQ: usize = 4;
pub const TIMER2_IRQ: usize = 19;

#[derive(Clone, Copy, Default)]
pub struct GpioRegisters {
    pub int_en: u32,
    pub int_mask: u32,
    pub int_bothedge: u32,
    pub int_status: u32,
    pub porta_eoi: u32,
}

#[derive(Clone, Copy, Default)]
pub struct TimerRegisters {
    pub en: u32,
    pub dier: u32,
    pub arr: u32,
    pub sr: u32,
}

#[derive(Clone)]
pub struct Hardware {
    pub gpio_routing: GpioRouting,
    /// LP SYSCTRL `REG_LP_FUNC_EN1` and `REG_LP_CLK_CTRL1`: GPIO and ADC enables.
    pub lp_function: u32,
    pub lp_clock: u32,
    /// Writes to the two LP enable registers are ignored.
    pub blocked_gpio_clock: bool,
    pub output: [u32; 2],
    pub input: [u32; 2],
    pub direction: [u32; 2],
    pub interrupts: u32,
    pub polls: u32,
    pub reads: u32,
    pub clears: u32,
    pub current_adc: u16,
    pub battery_adc: u16,
    /// What the Wi-Fi shim reports; tests script it.
    pub wifi: WifiState,
    /// Internal channel 10: the chip's own supply measurement.
    pub supply_adc: u16,
    pub output_hook: Option<fn()>,
    pub gpio_init_hook: Option<fn()>,
    pub blocked_gpio_write: [u32; 2],
    pub readback_override_mask: [u32; 2],
    pub readback_override: [u32; 2],
    /// Pads whose input path reads LOW regardless of the driven level.
    pub dead_input: [u32; 2],
    pub pad_control: [u32; 64],
    pub adc_enabled: bool,
    pub trigger: bool,
    pub stuck: bool,
    pub efuse_fail: bool,
    pub blocked_motor_write: bool,
    pub blocked_optics_write: bool,
    pub raw_ticks: u32,
    pub timer_phase: u32,
    pub timer_irqs: u32,
    pub gpio_irqs: u32,
    pub fail_timer_register: bool,
    pub fail_gpio_register: bool,
    pub task_create_fail: bool,
    pub task_live: bool,
    pub blocked_optics_off: bool,
    pub ipsr: u32,
    pub hs_control: u32,
    pub lp_control: u32,
    pub watchdog_ticks: u32,
    pub watchdog_period: u32,
    pub watchdog_init_calls: u32,
    pub watchdog_refreshes: u32,
    pub watchdog_reset_requests: u32,
    pub watchdog_expired: bool,
    pub watchdog_scope_blocked: bool,
    pub watchdog_disable_blocked: bool,
    pub watchdog_enable_corrupt: bool,
    pub task: Option<TaskStep>,
    pub task_context: *mut c_void,
    pub task_delays: u32,
    pub task_deletes: u32,
    pub wake_locks: u32,
    pub adc_delay_ticks: u32,
    pub adc_hook: Option<fn(*mut c_void)>,
    pub adc_hook_context: *mut c_void,
    pub irq: [Option<Isr>; 32],
    pub irq_enabled: [bool; 32],
    pub irq_pending: [bool; 32],
    pub adc_channel: u32,
    pub gpio: GpioRegisters,
    pub timer: TimerRegisters,
    pub watchdog_vendor: u32,
    pub backup: [u32; 8],
    pub boot_reason: u32,
    /// SYSTIMER ticks that pass during each busy-wait iteration.
    pub spin_ticks: u32,
    pub spins: u32,
    pub systick: u32,
    /// The 64 KiB storage region as NOR: erase sets 1s, program clears bits.
    pub storage: Vec<u8>,
    pub storage_erases: [u32; 16],
}

impl Default for Hardware {
    /// Reset state: inputs show outlet clear and DC present; nominal ADC values.
    fn default() -> Self {
        Self {
            gpio_routing: GpioRouting::default(),
            lp_function: 0,
            lp_clock: 0,
            blocked_gpio_clock: false,
            output: [0; 2],
            input: [(1 << 16) | (1 << 19), 0],
            direction: [0; 2],
            interrupts: 0,
            polls: 0,
            reads: 0,
            clears: 0,
            current_adc: 500,
            battery_adc: 2800,
            supply_adc: 3000,
            wifi: WifiState::default(),
            output_hook: None,
            gpio_init_hook: None,
            blocked_gpio_write: [0; 2],
            readback_override_mask: [0; 2],
            readback_override: [0; 2],
            // Physical observation on the research unit: PA0-PA7 read LOW.
            dead_input: [0xff, 0],
            pad_control: [0; 64],
            adc_enabled: false,
            trigger: false,
            stuck: false,
            efuse_fail: false,
            blocked_motor_write: false,
            blocked_optics_write: false,
            raw_ticks: 0,
            timer_phase: 0,
            timer_irqs: 0,
            gpio_irqs: 0,
            fail_timer_register: false,
            fail_gpio_register: false,
            task_create_fail: false,
            task_live: false,
            blocked_optics_off: false,
            ipsr: 0,
            hs_control: 0,
            lp_control: 0,
            watchdog_ticks: 0,
            watchdog_period: 0,
            watchdog_init_calls: 0,
            watchdog_refreshes: 0,
            watchdog_reset_requests: 0,
            watchdog_expired: false,
            watchdog_scope_blocked: false,
            watchdog_disable_blocked: false,
            watchdog_enable_corrupt: false,
            task: None,
            task_context: core::ptr::null_mut(),
            task_delays: 0,
            task_deletes: 0,
            wake_locks: 0,
            adc_delay_ticks: 0,
            adc_hook: None,
            adc_hook_context: core::ptr::null_mut(),
            irq: [None; 32],
            irq_enabled: [false; 32],
            irq_pending: [false; 32],
            adc_channel: 0,
            gpio: GpioRegisters::default(),
            timer: TimerRegisters::default(),
            watchdog_vendor: 0,
            backup: [0; 8],
            boot_reason: 0,
            spin_ticks: 33,
            spins: 0,
            systick: 7,
            storage: vec![u8::MAX; 16 * 4096],
            storage_erases: [0; 16],
        }
    }
}

thread_local! {
    static HARDWARE: RefCell<Hardware> = RefCell::new(Hardware::default());
}

/// Access this thread's hardware. Never call SDK functions inside `f`.
pub fn with<R>(f: impl FnOnce(&mut Hardware) -> R) -> R {
    HARDWARE.with(|h| f(&mut h.borrow_mut()))
}

pub fn reset_hardware() {
    with(|h| *h = Hardware::default());
}

fn irq_index(irq: Irq) -> usize {
    match irq {
        Irq::Timer2 => TIMER2_IRQ,
        Irq::GpioA => GPIOA_IRQ,
    }
}

/// Raise an interrupt line: it runs now when enabled, unmasked and no
/// handler is active; otherwise it stays pending until it can run.
pub fn irq(index: usize) {
    with(|h| h.irq_pending[index] = true);
    deliver_pending();
}

/// Run pending interrupts that can now be taken, lowest line first. The
/// ports' handlers share one priority, so none runs inside another.
pub fn deliver_pending() {
    while let Some(index) = with(|h| {
        if h.interrupts != 0 || h.ipsr != 0 {
            return None;
        }
        (0..32).find(|&i| h.irq_pending[i] && h.irq_enabled[i])
    }) {
        run_irq(index);
    }
}

fn run_irq(index: usize) {
    let handler = with(|h| {
        h.irq_pending[index] = false;
        // An unregistered line reaches the SDK default handler: nothing to model.
        let handler = h.irq[index]?;
        let previous = h.ipsr;
        h.ipsr = index as u32 + 16;
        if index == TIMER2_IRQ {
            h.timer_irqs += 1;
        }
        if index == GPIOA_IRQ {
            h.gpio_irqs += 1;
        }
        Some((handler, previous))
    });
    if let Some((handler, previous)) = handler {
        // SAFETY: handlers are the ports' registered ISRs, run synchronously.
        unsafe { handler(core::ptr::null_mut()) };
        with(|h| {
            h.ipsr = previous;
            if index == GPIOA_IRQ {
                h.gpio.int_status &= !h.gpio.porta_eoi;
                h.gpio.porta_eoi = 0;
            }
        });
    }
}

/// Pad input word for one port: driven outputs follow the latch, inputs the
/// external level; dead pads read LOW; test overrides replace either.
fn pad_input(h: &Hardware, p: usize) -> u32 {
    let normal =
        ((h.output[p] & h.direction[p]) | (h.input[p] & !h.direction[p])) & !h.dead_input[p];
    (normal & !h.readback_override_mask[p]) | (h.readback_override[p] & h.readback_override_mask[p])
}

/// Independent watchdog clock; does not advance SYSTIMER or run an ISR.
pub fn watchdog_advance(ticks: u32) {
    with(|h| {
        if h.watchdog_vendor & api::WDG_BIT_ENABLE == 0 || h.watchdog_expired {
            return;
        }
        let elapsed = u64::from(h.watchdog_ticks) + u64::from(ticks);
        if elapsed >= u64::from(h.watchdog_period) {
            h.watchdog_ticks = h.watchdog_period;
            h.watchdog_expired = true;
            h.watchdog_reset_requests += 1;
            h.watchdog_vendor |= api::WDG_BIT_ISR_CLEAR;
        } else {
            h.watchdog_ticks = elapsed as u32;
        }
    });
}

/// Advance SYSTIMER and TIM2, delivering one coalesced update interrupt.
pub fn advance(ticks: u32, run_timer: bool) {
    with(|h| h.raw_ticks = h.raw_ticks.wrapping_add(ticks));
    watchdog_advance(ticks);
    let fire = with(|h| {
        if h.timer.en & api::TIM_CR_CNT_STS == 0 {
            return false;
        }
        h.timer_phase = h.timer_phase.wrapping_add(ticks);
        let period = u64::from(h.timer.arr) + 1;
        if u64::from(h.timer_phase) < period {
            return false;
        }
        h.timer_phase = (u64::from(h.timer_phase) % period) as u32;
        h.timer.sr |= api::TIM_IT_UPDATE;
        // Model a coalesced pending IRQ, not one callback per lost period.
        run_timer
    });
    if fire {
        irq(TIMER2_IRQ);
    }
}

/// Drive the feedback input (PA18) and raise its interrupt when enabled.
pub fn edge(high: bool) {
    let mask = 1u32 << 18;
    let raise = with(|h| {
        if high {
            h.input[0] |= mask;
        } else {
            h.input[0] &= !mask;
        }
        let raise = h.gpio.int_en & mask != 0 && h.gpio.int_mask & mask == 0;
        if raise {
            h.gpio.int_status |= mask;
        }
        raise
    });
    if raise {
        irq(GPIOA_IRQ);
    }
}

/// Run one complete iteration of the created task, as if it yielded at its delay.
pub fn run_task() {
    let (step, context) = with(|h| {
        assert!(h.task_live && h.task.is_some());
        (h.task.unwrap(), h.task_context)
    });
    // SAFETY: the step and context were registered by the port under test.
    let delay = unsafe { step(context) };
    with(|h| {
        assert_eq!(
            h.interrupts, 0,
            "tasks yield and delete with interrupts enabled"
        );
        if delay == 0 {
            h.task_live = false;
            h.task_deletes += 1;
        } else {
            h.task_delays += 1;
        }
    });
}

pub mod api;
