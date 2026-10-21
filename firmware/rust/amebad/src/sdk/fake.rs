//! Deterministic host model of the SDK calls, ported from the C test fake.
//!
//! Each thread owns independent hardware so tests can run in parallel. The
//! model asserts how the ports drive the SDK (pins, priorities, task shape),
//! models coalesced timer interrupts and GPIO edges, and runs registered
//! interrupt handlers synchronously when interrupts are unmasked.

use core::ffi::{CStr, c_void};
use std::cell::RefCell;

use super::{GpioClock, GpioRouting, Irq, Isr, TaskStep};

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
    pub ext_port: u32,
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
    pub gpio_function: u32,
    pub gpio_clock: u32,
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
    pub output_hook: Option<fn()>,
    pub gpio_init_hook: Option<fn()>,
    pub blocked_gpio_write: [u32; 2],
    pub readback_override_mask: [u32; 2],
    pub readback_override: [u32; 2],
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
    pub adc_channel: u32,
    pub gpio: GpioRegisters,
    pub timer: TimerRegisters,
    pub watchdog_vendor: u32,
}

impl Default for Hardware {
    /// Reset state: inputs show outlet clear and DC present; nominal ADC values.
    fn default() -> Self {
        Self {
            gpio_routing: GpioRouting::default(),
            gpio_function: 0,
            gpio_clock: 0,
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
            output_hook: None,
            gpio_init_hook: None,
            blocked_gpio_write: [0; 2],
            readback_override_mask: [0; 2],
            readback_override: [0; 2],
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
            adc_channel: 0,
            gpio: GpioRegisters::default(),
            timer: TimerRegisters::default(),
            watchdog_vendor: 0,
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

/// Deliver an interrupt when it is registered, enabled and unmasked.
pub fn irq(index: usize) {
    let handler = with(|h| {
        let ready = h.irq_enabled[index] && h.interrupts == 0;
        let handler = h.irq[index].filter(|_| ready)?;
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
        h.gpio.ext_port = h.input[0];
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

/// The SDK function surface, mirroring `sdk::target`.
pub mod api {
    use super::*;

    pub fn gpio_clock_state() -> GpioClock {
        with(|h| GpioClock {
            function_before: h.gpio_function,
            clock_before: h.gpio_clock,
            function_after: h.gpio_function,
            clock_after: h.gpio_clock,
        })
    }

    pub fn gpio_clock_enable() -> GpioClock {
        let _guard = crate::sdk::IrqGuard::lock();
        let before = gpio_clock_state();
        with(|h| {
            if !h.blocked_gpio_clock {
                h.gpio_function |= crate::sdk::GPIO_ENABLE_BIT;
                h.gpio_clock |= crate::sdk::GPIO_ENABLE_BIT;
            }
        });
        GpioClock {
            function_before: before.function_before,
            clock_before: before.clock_before,
            ..gpio_clock_state()
        }
    }

    pub const GPIO_PORT_A: u32 = 0;
    pub const PMU_USER_BASE: u32 = 16;
    pub const PMU_MAX: u32 = 31;
    pub const TIM_CR_CNT_START: u32 = 1;
    pub const TIM_CR_CNT_RUN: u32 = 256;
    pub const TIM_CR_CNT_STS: u32 = 65536;
    pub const TIM_IT_UPDATE: u32 = 1;
    pub const WDG_BIT_ENABLE: u32 = 1 << 16;
    pub const WDG_BIT_RST_MODE: u32 = 1 << 30;
    pub const WDG_BIT_ISR_CLEAR: u32 = 1 << 31;
    pub const HS_WDTRST_OPT: u32 = 1 << 23;
    pub const LP_WDT_RESET_HS_ALL: u32 = 1 << 15;

    pub fn primask() -> u32 {
        with(|h| h.interrupts)
    }

    pub fn ipsr() -> u32 {
        with(|h| h.ipsr)
    }

    pub fn disable_irq() {
        with(|h| h.interrupts = 1);
    }

    pub fn set_primask(mask: u32) {
        with(|h| h.interrupts = mask);
    }

    pub fn gpio_port_write(port: u32, mask: u32, value: u32) {
        let hook = with(|h| {
            assert!(port < 2);
            if port == 0 && mask == MOTOR_MASK && h.blocked_motor_write {
                return None;
            }
            let p = port as usize;
            let mask = mask & !h.blocked_gpio_write[p];
            h.output[p] = (h.output[p] & !mask) | (value & mask);
            Some(h.output_hook)
        });
        if let Some(Some(hook)) = hook {
            hook();
        }
    }

    pub fn gpio_port_read(port: u32, mask: u32) -> u32 {
        with(|h| {
            assert!(port < 2);
            let p = port as usize;
            let normal = (h.output[p] & h.direction[p]) | (h.input[p] & !h.direction[p]);
            ((normal & !h.readback_override_mask[p])
                | (h.readback_override[p] & h.readback_override_mask[p]))
                & mask
        })
    }

    pub fn gpio_no_pull(pin: u32) {
        assert!(pin < 64);
        with(|h| h.pad_control[pin as usize] &= !(3 << 8));
    }

    pub fn gpio_init(pin: u32, output: bool) {
        let hook = with(|h| {
            assert!(pin < 64);
            let (port, mask) = ((pin >> 5) as usize, 1u32 << (pin & 31));
            if output {
                h.direction[port] |= mask;
            } else {
                h.direction[port] &= !mask;
            }
            h.gpio_init_hook
        });
        if let Some(hook) = hook {
            hook();
        }
    }

    pub fn gpio_routing() -> GpioRouting {
        with(|h| h.gpio_routing)
    }

    pub fn gpio_direction(port: u32) -> u32 {
        assert!(port < 2);
        with(|h| h.direction[port as usize])
    }

    pub fn gpio_output_latch(port: u32) -> u32 {
        assert!(port < 2);
        with(|h| h.output[port as usize])
    }

    pub fn pad_control(pin: u32) -> u32 {
        assert!(pin < 64);
        with(|h| h.pad_control[pin as usize])
    }

    pub fn gpio_write_bit(pin: u32, high: bool) {
        let blocked = with(|h| {
            pin == 35 && ((high && h.blocked_optics_write) || (!high && h.blocked_optics_off))
        });
        if blocked {
            return;
        }
        let mask = 1u32 << (pin & 31);
        gpio_port_write(pin >> 5, mask, if high { mask } else { 0 });
    }

    pub fn gpio_read_bit(pin: u32) -> bool {
        gpio_port_read(pin >> 5, 1 << (pin & 31)) != 0
    }

    pub fn efuse_read8(address: u32) -> Option<u8> {
        assert!((0x1d0..=0x1d3).contains(&address));
        with(|h| (!h.efuse_fail).then_some(255))
    }

    pub fn pad_disable(pin: u8) {
        assert!(pin == 33 || pin == 34);
    }

    pub fn adc_init_manual() {}

    pub fn adc_cmd(enabled: bool) {
        with(|h| h.adc_enabled = enabled);
    }

    pub fn adc_select(channel: u32) {
        with(|h| h.adc_channel = channel);
    }

    pub fn adc_clear_fifo() {
        with(|h| h.clears += 1);
    }

    pub fn adc_sw_trigger(enabled: bool) {
        with(|h| h.trigger = enabled);
    }

    pub fn adc_readable() -> bool {
        let (delay, hook) = with(|h| {
            assert!(h.adc_enabled && h.trigger);
            h.polls += 1;
            let delay = core::mem::take(&mut h.adc_delay_ticks);
            let hook = h.adc_hook.take().map(|hook| (hook, h.adc_hook_context));
            (delay, hook)
        });
        if delay != 0 {
            advance(delay, true);
        }
        if let Some((hook, context)) = hook {
            hook(context);
        }
        with(|h| !h.stuck)
    }

    pub fn adc_read() -> u16 {
        with(|h| {
            assert!(!h.trigger);
            h.reads += 1;
            assert!(h.adc_channel == 4 || h.adc_channel == 5);
            if h.adc_channel == 4 {
                h.current_adc
            } else {
                h.battery_adc
            }
        })
    }

    pub fn systimer_ticks() -> u32 {
        with(|h| h.raw_ticks)
    }

    pub fn gtimer_clock_enable() {}

    pub fn gpio_int_config(pin: u32, enabled: bool) {
        assert_eq!(pin, 18);
        with(|h| {
            if enabled {
                h.gpio.int_mask &= !(1 << pin);
            } else {
                h.gpio.int_mask |= 1 << pin;
            }
        });
    }

    pub fn gpio_int_mode(pin: u32, enabled: bool) {
        assert_eq!(pin, 18);
        with(|h| {
            if enabled {
                h.gpio.int_en |= 1 << pin;
            } else {
                h.gpio.int_en &= !(1 << pin);
            }
            h.gpio.int_bothedge |= 1 << pin;
        });
    }

    pub fn gpioa_int_status() -> u32 {
        with(|h| h.gpio.int_status)
    }

    pub fn gpioa_int_en() -> u32 {
        with(|h| h.gpio.int_en)
    }

    pub fn gpioa_int_mask() -> u32 {
        with(|h| h.gpio.int_mask)
    }

    pub fn gpioa_int_bothedge() -> u32 {
        with(|h| h.gpio.int_bothedge)
    }

    pub fn gpioa_ext_port() -> u32 {
        with(|h| h.gpio.ext_port)
    }

    pub fn gpioa_eoi(mask: u32) {
        with(|h| h.gpio.porta_eoi = mask);
    }

    pub fn tim2_en() -> u32 {
        with(|h| h.timer.en)
    }

    pub fn tim2_dier() -> u32 {
        with(|h| h.timer.dier)
    }

    pub fn tim2_arr() -> u32 {
        with(|h| h.timer.arr)
    }

    pub fn tim2_init(period: u32) {
        with(|h| {
            h.timer.arr = period;
            h.timer.en = 0;
            h.timer.dier = 0;
        });
    }

    pub fn tim2_int_config(enabled: bool) {
        with(|h| {
            if enabled {
                h.timer.dier |= TIM_IT_UPDATE;
            } else {
                h.timer.dier &= !TIM_IT_UPDATE;
            }
        });
    }

    pub fn tim2_cmd(enabled: bool) {
        with(|h| h.timer.en = if enabled { TIM_CR_CNT_STS } else { 0 });
    }

    pub fn tim2_int_clear() {
        with(|h| h.timer.sr = 0);
    }

    pub fn irq_register(irq: Irq, isr: Isr, priority: u32) -> bool {
        assert_eq!(priority, 4);
        with(|h| {
            if (irq == Irq::Timer2 && h.fail_timer_register)
                || (irq == Irq::GpioA && h.fail_gpio_register)
            {
                return false;
            }
            h.irq[irq_index(irq)] = Some(isr);
            true
        })
    }

    pub fn irq_unregister(irq: Irq) -> bool {
        with(|h| h.irq[irq_index(irq)] = None);
        true
    }

    pub fn irq_enable(irq: Irq, priority: u32) {
        assert_eq!(priority, 4);
        with(|h| h.irq_enabled[irq_index(irq)] = true);
    }

    pub fn irq_disable(irq: Irq) {
        with(|h| h.irq_enabled[irq_index(irq)] = false);
    }

    pub fn wakelock_acquire(id: u32) {
        with(|h| {
            assert!((16..=31).contains(&id) && id != 17 && h.interrupts == 0);
            h.wake_locks |= 1 << id;
        });
    }

    pub fn wakelock_release(id: u32) {
        with(|h| {
            assert!((16..=31).contains(&id) && id != 17 && h.interrupts == 0);
            assert_eq!(h.timer.en, 0);
            h.wake_locks &= !(1 << id);
        });
    }

    pub fn wdg_vendor() -> u32 {
        with(|h| h.watchdog_vendor)
    }

    pub fn wdg_init(count_id: u32, divisor: u32) {
        with(|h| {
            assert!(h.interrupts == 1 && count_id <= 11 && divisor > 0 && divisor <= 65535);
            h.watchdog_init_calls += 1;
            h.watchdog_period = ((1u32 << (count_id + 1)) - 1) * (divisor + 1);
            h.watchdog_ticks = 0;
            h.watchdog_expired = false;
            h.watchdog_vendor = (count_id << 25) | divisor | WDG_BIT_RST_MODE;
        });
        set_lp_km0_ctrl(lp_km0_ctrl() | LP_WDT_RESET_HS_ALL);
    }

    pub fn wdg_cmd(enabled: bool) {
        with(|h| {
            assert_eq!(h.interrupts, 1);
            if enabled {
                h.watchdog_vendor |= WDG_BIT_ENABLE;
                if h.watchdog_enable_corrupt {
                    h.watchdog_vendor ^= 1;
                }
            } else if !h.watchdog_disable_blocked {
                h.watchdog_vendor &= !WDG_BIT_ENABLE;
            }
            h.watchdog_vendor &= !WDG_BIT_ISR_CLEAR;
        });
    }

    pub fn wdg_refresh() {
        with(|h| {
            assert_eq!(h.interrupts, 1);
            h.watchdog_refreshes += 1;
            h.watchdog_ticks = 0;
        });
    }

    pub fn hs_platform_para() -> u32 {
        with(|h| h.hs_control)
    }

    pub fn set_hs_platform_para(value: u32) {
        with(|h| {
            if !h.watchdog_scope_blocked {
                h.hs_control = value;
            }
        });
    }

    pub fn lp_km0_ctrl() -> u32 {
        with(|h| h.lp_control)
    }

    pub fn set_lp_km0_ctrl(value: u32) {
        with(|h| {
            if !h.watchdog_scope_blocked {
                h.lp_control = value;
            }
        });
    }

    pub fn task_create(
        name: &'static CStr,
        stack_words: u16,
        priority: u32,
        step: TaskStep,
        context: *mut c_void,
    ) -> bool {
        assert!(name == c"feed-owner" && stack_words == 2048 && priority == 3);
        with(|h| {
            assert!(!h.task_live);
            if h.task_create_fail {
                return false;
            }
            h.task = Some(step);
            h.task_context = context;
            h.task_live = true;
            true
        })
    }

    pub fn log(message: &CStr) {
        print!("{}", message.to_string_lossy());
    }
}
