//! The SDK function surface, mirroring `sdk::target`.
//!
//! Same names and signatures as the target build, backed by the per-thread
//! [`Hardware`] model. Constants are host stand-ins for the SDK's values. The
//! asserts encode how the ports must call the SDK (pins, priorities, task
//! shape, interrupt state), so a port that drifts fails its host tests.

use super::*;
use crate::{Accepted, ApiRecv, Datagram};

fn lp_clock_state(bit: u32) -> LpClock {
    with(|h| LpClock {
        bit,
        function_before: h.lp_function,
        clock_before: h.lp_clock,
        function_after: h.lp_function,
        clock_after: h.lp_clock,
    })
}

pub fn gpio_clock_state() -> LpClock {
    lp_clock_state(crate::GPIO_ENABLE_BIT)
}

fn lp_clock_enable(bit: u32) -> LpClock {
    let _guard = crate::IrqGuard::lock();
    let before = lp_clock_state(bit);
    with(|h| {
        if !h.blocked_gpio_clock {
            h.lp_function |= bit;
            h.lp_clock |= bit;
        }
    });
    LpClock {
        function_before: before.function_before,
        clock_before: before.clock_before,
        ..lp_clock_state(bit)
    }
}

pub fn gpio_clock_enable() -> LpClock {
    lp_clock_enable(crate::GPIO_ENABLE_BIT)
}

pub fn adc_clock_enable() -> LpClock {
    lp_clock_enable(crate::ADC_ENABLE_BIT)
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
/// Counter-restart action bit written by WDG_Init and WDG_Refresh.
const WDG_BIT_CLEAR: u32 = 1 << 24;
pub const HS_WDTRST_OPT: u32 = 1 << 23;
pub const LP_WDT_RESET_HS_ALL: u32 = 1 << 15;

pub const BACKUP_USER: u32 = 1;
pub const BOOT_KM4_WATCHDOG: u32 = 1 << 20;
pub const BOOT_KM4_SYSTEM: u32 = 1 << 19;

pub fn backup_read(index: u32) -> u32 {
    with(|h| h.backup[index as usize])
}

pub fn backup_write(index: u32, value: u32) {
    assert!(
        (1..=5).contains(&index),
        "REG0, REG6 and REG7 belong to the system"
    );
    with(|h| h.backup[index as usize] = value);
}

pub fn boot_reason() -> u32 {
    with(|h| h.boot_reason)
}

fn storage_span(sector: u32, offset: u32, len: usize) -> Option<usize> {
    let end = offset as usize + len;
    (sector < STORAGE_SECTORS && end <= SECTOR_BYTES as usize)
        .then(|| (sector * SECTOR_BYTES + offset) as usize)
}

/// # Safety
/// The HAL's flash hooks are armed, so the SDK routine takes the KM0 lease
/// around the write, and the caller owns what it writes.
pub unsafe fn storage_erase(sector: u32) -> bool {
    let Some(at) = storage_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    with(|h| {
        h.storage[at..at + SECTOR_BYTES as usize].fill(u8::MAX);
        h.storage_erases[sector as usize] += 1;
    });
    true
}

/// # Safety
/// The HAL's flash hooks are armed, so the SDK routine takes the KM0 lease
/// around the write, and the caller owns what it writes.
pub unsafe fn storage_program(sector: u32, offset: u32, data: &[u8]) -> bool {
    let Some(at) = storage_span(sector, offset, data.len()) else {
        return false;
    };
    if !offset.is_multiple_of(4) || !data.len().is_multiple_of(4) {
        return false;
    }
    with(|h| {
        for (cell, &byte) in h.storage[at..at + data.len()].iter_mut().zip(data) {
            *cell &= byte;
        }
    });
    true
}

/// Erases of storage sector `sector` since the fake was reset.
pub fn storage_erase_count(sector: u32) -> u32 {
    with(|h| {
        usize::try_from(sector)
            .ok()
            .and_then(|sector| h.storage_erases.get(sector).copied())
            .unwrap_or(0)
    })
}

/// Erases of bank-1 sectors since the fake was reset.
pub fn bank1_erase_count() -> u32 {
    with(|h| h.bank1_erases)
}

/// The modelled tasks' stacks into `out`; how many it filled.
pub fn task_stacks(out: &mut [crate::TaskStack]) -> usize {
    with(|h| {
        let n = h.task_stacks.len().min(out.len());
        out[..n].copy_from_slice(&h.task_stacks[..n]);
        n
    })
}

pub fn storage_read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
    let Some(at) = storage_span(sector, offset, bytes.len()) else {
        return false;
    };
    with(|h| bytes.copy_from_slice(&h.storage[at..at + bytes.len()]));
    true
}

fn bank1_span(sector: u32, offset: u32, len: usize) -> Option<usize> {
    let end = offset as usize + len;
    with(|h| h.running_bank == Some(0))
        .then_some(())
        .filter(|_| sector < crate::BANK1_SECTORS && end <= SECTOR_BYTES as usize)
        .map(|_| (sector * SECTOR_BYTES + offset) as usize)
}

/// # Safety
/// The HAL's flash hooks are armed, so the SDK routine takes the KM0 lease
/// around the write, and the caller owns what it writes.
pub unsafe fn bank1_erase(sector: u32) -> bool {
    let Some(at) = bank1_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    with(|h| {
        h.bank1[at..at + SECTOR_BYTES as usize].fill(u8::MAX);
        h.bank1_erases += 1;
    });
    true
}

/// # Safety
/// The HAL's flash hooks are armed, so the SDK routine takes the KM0 lease
/// around the write, and the caller owns what it writes.
pub unsafe fn bank1_program(sector: u32, offset: u32, data: &[u8]) -> bool {
    let Some(at) = bank1_span(sector, offset, data.len()) else {
        return false;
    };
    if !offset.is_multiple_of(4) || !data.len().is_multiple_of(4) {
        return false;
    }
    with(|h| {
        for (cell, &byte) in h.bank1[at..at + data.len()].iter_mut().zip(data) {
            *cell &= byte;
        }
    });
    true
}

pub fn bank1_read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
    let Some(at) = bank1_span(sector, offset, bytes.len()) else {
        return false;
    };
    with(|h| bytes.copy_from_slice(&h.bank1[at..at + bytes.len()]));
    true
}

/// The fake has no cache: the same bytes as `bank1_read`.
pub fn bank1_read_uncached(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
    bank1_read(sector, offset, bytes)
}

/// Bank 0's bytes, its first 8 from `bank0_marker`.
pub fn bank0_read(offset: u32, bytes: &mut [u8]) -> bool {
    let start = offset as usize;
    let end = start + bytes.len();
    if end > (crate::BANK1_OFFSET - crate::BANK0_OFFSET) as usize {
        return false;
    }
    with(|h| {
        for (i, byte) in bytes.iter_mut().enumerate() {
            let at = start + i;
            *byte = match h.bank0_marker.get(at) {
                Some(&marker) => marker,
                None => h.bank0.get(at).copied().unwrap_or(u8::MAX),
            };
        }
    });
    true
}

/// As the target: clear bank 1's first marker byte, only from bank 1 with
/// both markers valid.
///
/// # Safety
/// The HAL's flash hooks are armed, so the SDK routine takes the KM0 lease
/// around the write, and the caller owns what it writes.
pub unsafe fn bank1_give_way() -> bool {
    with(|h| {
        let ok = h.running_bank == Some(1)
            && h.bank1[..8] == crate::MARKER
            && h.bank0_marker == crate::MARKER;
        if ok {
            h.bank1[0] = 0;
        }
        ok
    })
}

pub fn flash_read_jedec() -> [u8; 3] {
    [0xc8, 0x40, 0x17]
}

pub fn ipc_interrupts() -> (u32, u32) {
    (0, 0)
}

pub fn systick_ctrl() -> u32 {
    with(|h| h.systick)
}

/// CLKSOURCE (bit 2) reads 1 whatever is written, as on the KM4 Cortex-M33.
pub fn set_systick_ctrl(value: u32) {
    with(|h| h.systick = value | 4);
}

/// Time passes while the caller spins: SYSTIMER, TIM2 and the watchdog advance.
pub fn spin_once() {
    let ticks = with(|h| {
        h.spins += 1;
        h.spin_ticks
    });
    advance(ticks, true);
}

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
    deliver_pending();
}

/// # Safety
/// The caller owns every pin it writes or reconfigures. The motor pins
/// ([`MOTOR_MASK`](wellfed_board_plaf103::pins::MOTOR_MASK)) belong to the HAL's board and
/// stopped-output owners, every other output to the port that drives it.
pub unsafe fn gpio_port_write(port: u32, mask: u32, value: u32) {
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
        pad_input(h, port as usize) & mask
    })
}

pub fn gpio_no_pull(pin: u32) {
    assert!(pin < 64);
    with(|h| h.pad_control[pin as usize] &= !(3 << 8));
}

/// # Safety
/// The caller owns every pin it writes or reconfigures. The motor pins
/// ([`MOTOR_MASK`](wellfed_board_plaf103::pins::MOTOR_MASK)) belong to the HAL's board and
/// stopped-output owners, every other output to the port that drives it.
pub unsafe fn gpio_init(pin: u32, output: bool) {
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

pub fn swd_pinmux() -> u32 {
    with(|h| h.swd_pmux)
}

pub fn swd_off() -> u32 {
    with(|h| {
        if !h.swd_stuck {
            h.swd_pmux &= !crate::SWD_PMUX_BIT;
        }
        h.swd_pmux
    })
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

/// # Safety
/// The caller owns every pin it writes or reconfigures. The motor pins
/// ([`MOTOR_MASK`](wellfed_board_plaf103::pins::MOTOR_MASK)) belong to the HAL's board and
/// stopped-output owners, every other output to the port that drives it.
pub unsafe fn gpio_write_bit(pin: u32, high: bool) {
    let blocked = with(|h| {
        pin == pins::IR_PB && ((high && h.blocked_optics_write) || (!high && h.blocked_optics_off))
    });
    if blocked {
        return;
    }
    let mask = 1u32 << (pin & 31);
    // SAFETY: the caller's contract, for the same one pin.
    unsafe { gpio_port_write(pin >> 5, mask, if high { mask } else { 0 }) };
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
    let Some((delay, hook)) = with(|h| {
        assert!(h.adc_enabled && h.trigger);
        h.polls += 1;
        // An unclocked ADC never converts.
        if h.lp_function & h.lp_clock & crate::ADC_ENABLE_BIT == 0 {
            return None;
        }
        let delay = core::mem::take(&mut h.adc_delay_ticks);
        let hook = h.adc_hook.take().map(|hook| (hook, h.adc_hook_context));
        Some((delay, hook))
    }) else {
        return false;
    };

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
        match h.adc_channel {
            4 => h.current_adc,
            5 => h.battery_adc,
            10 if h.adc_noise != 0 => {
                // xorshift32 in the low byte, as noise would be.
                h.adc_noise ^= h.adc_noise << 13;
                h.adc_noise ^= h.adc_noise >> 17;
                h.adc_noise ^= h.adc_noise << 5;
                (h.supply_adc & !0xff) | u16::from(h.adc_noise.to_le_bytes()[0])
            }
            10 => h.supply_adc,
            channel => panic!("unmodelled ADC channel {channel}"),
        }
    })
}

pub fn systimer_ticks() -> u32 {
    with(|h| {
        let ticks = h.raw_ticks;
        h.raw_ticks = ticks.wrapping_add(h.ticks_per_read);
        ticks
    })
}

pub fn systick() -> Systick {
    with(|h| {
        let value = h.systick_value;
        let period = h.systick_reload + 1;
        h.systick_value = (value + period - h.systick_step % period) % period;
        Systick {
            value,
            reload: h.systick_reload,
        }
    })
}

pub fn heap_free() -> u32 {
    with(|h| h.heap_free)
}

pub fn heap_min_free() -> u32 {
    with(|h| h.heap_min_free)
}

pub fn gtimer_clock_enable() {}

pub fn gpio_int_config(pin: u32, enabled: bool) {
    assert_eq!(pin, pins::FEEDBACK);
    with(|h| {
        if enabled {
            h.gpio.int_mask &= !(1 << pin);
        } else {
            h.gpio.int_mask |= 1 << pin;
        }
    });
}

pub fn gpio_int_mode(pin: u32, enabled: bool) {
    assert_eq!(pin, pins::FEEDBACK);
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
    with(|h| pad_input(h, 0))
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
    deliver_pending();
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

/// # Safety
/// The caller is the watchdog's one owner (the HAL's supervision
/// watchdog): another writer could disarm it or let it starve.
pub unsafe fn wdg_init(count_id: u32, divisor: u32) {
    with(|h| {
        assert!(h.interrupts == 1 && count_id <= 11 && divisor > 0 && divisor <= 65535);
        h.watchdog_init_calls += 1;
        h.watchdog_period = ((1u32 << (count_id + 1)) - 1) * (divisor + 1);
        h.watchdog_ticks = 0;
        h.watchdog_expired = false;

        // The SDK writes CLEAR (bit 24) and ISR_CLEAR (bit 31) with the
        // configuration; both are write-1 actions that read back as 0.
        let written =
            (count_id << 25) | divisor | WDG_BIT_CLEAR | WDG_BIT_RST_MODE | WDG_BIT_ISR_CLEAR;
        h.watchdog_vendor = written & !(WDG_BIT_CLEAR | WDG_BIT_ISR_CLEAR);
    });
    set_lp_km0_ctrl(lp_km0_ctrl() | LP_WDT_RESET_HS_ALL);
}

/// # Safety
/// The caller is the watchdog's one owner (the HAL's supervision
/// watchdog): another writer could disarm it or let it starve.
pub unsafe fn wdg_cmd(enabled: bool) {
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

/// # Safety
/// The caller is the watchdog's one owner (the HAL's supervision
/// watchdog): another writer could disarm it or let it starve.
pub unsafe fn wdg_refresh() {
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

/// The fake records the task; `fake::run_task` steps it.
///
/// # Safety
/// `step` must be sound to call with `context`, again and again, from the
/// new task until it returns zero; `context` stays valid for that long.
pub unsafe fn task_create(
    name: &'static CStr,
    stack_words: u16,
    priority: u32,
    step: TaskStep,
    context: *mut c_void,
) -> bool {
    let _ = (name, stack_words, priority);
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

/// Mirrors the target's panic-path stop: mask, then raw latch writes only
/// when the GPIO block is clocked and enabled.
pub fn panic_outputs_safe() {
    with(|h| {
        h.interrupts = 1;
        let enabled = crate::GPIO_ENABLE_BIT;
        if h.lp_function & enabled == 0 || h.lp_clock & enabled == 0 {
            return;
        }
        h.output[0] = (h.output[0] | MOTOR_MASK) & !IR_PA_BIT;
        h.output[1] &= !IR_PB_BIT;
    });
}

pub fn log(message: &CStr) {
    print!("{}", message.to_string_lossy());
    crate::LOG_TAIL.push(message.to_bytes());
}

/// One host thread drives the fake, so nothing else can be scheduled.
pub fn unscheduled<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Each host thread stands for one task: the test harness runs tests on
/// separate threads, and a static claimed by one must refuse another.
pub fn current_task() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(1);
    std::thread_local! {
        static TASK: usize = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TASK.with(|task| *task)
}

pub fn audio_open(rate_hz: u32) -> bool {
    with(|h| {
        if h.audio_open || h.audio_open_fails || ![8000, 16000, 32000, 48000].contains(&rate_hz) {
            return false;
        }
        h.audio_open = true;
        h.audio_rate = rate_hz;
        h.audio_opens += 1;
        h.audio_underruns = 0;
        true
    })
}

pub(crate) fn audio_take() -> bool {
    with(|h| !core::mem::replace(&mut h.audio_taken, true))
}

pub(crate) fn audio_release() {
    with(|h| h.audio_taken = false);
}

pub(crate) fn audio_page_ptr() -> *mut [i16; AUDIO_PAGE_SAMPLES] {
    with(|h| {
        if !h.audio_open || h.audio_queue.len() >= AUDIO_PAGES {
            return core::ptr::null_mut();
        }
        if h.audio_scratch.is_null() {
            h.audio_scratch = Box::into_raw(Box::new([0; AUDIO_PAGE_SAMPLES]));
        }
        h.audio_scratch
    })
}

pub(crate) fn audio_submit() {
    with(|h| {
        assert!(h.audio_open, "submit with the codec closed");
        // SAFETY: leaked for the thread by `audio_page_ptr`; the page that
        // borrowed it ended with its submit.
        let page = unsafe { *h.audio_scratch };
        h.audio_queue.push_back(page);
    });
}

pub fn audio_drained() -> bool {
    with(|h| h.audio_queue.is_empty())
}

pub fn audio_underruns() -> u32 {
    with(|h| h.audio_underruns)
}

pub fn audio_close() {
    with(|h| {
        h.audio_open = false;
        h.audio_queue.clear();
    });
}

pub fn tcp_close(socket: i32) {
    with(|h| h.tcp_closed.push(socket));
}

pub fn uptime_ms() -> u32 {
    with(|h| h.uptime_ms)
}

/// The task's wait passes on the fake clock.
pub fn delay_ms(ms: u32) {
    with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(ms));
}

/// Starts once; tests push `Hardware::wifi_events` as the shim would.
pub fn wifi_start() -> bool {
    with(|h| {
        let first = !h.wifi_started;
        h.wifi_started = true;
        first
    })
}

pub fn wifi_event(seq: u32) -> WifiPoll {
    with(|h| {
        let recorded =
            u32::try_from(h.wifi_events.len()).expect("a test records fewer than 2^32 events");
        if seq == 0 || seq > recorded {
            WifiPoll::Pending
        } else if recorded - seq >= h.wifi_ring {
            WifiPoll::Lost
        } else {
            WifiPoll::Event(h.wifi_events[seq as usize - 1])
        }
    })
}

/// Starts once; tests record events with [`ble_record`].
pub fn ble_start(service: &BleService) -> bool {
    with(|h| {
        let first = h.ble_started.is_none();
        h.ble_started = Some(*service);
        first
    })
}

pub fn ble_event(seq: u32) -> BlePoll {
    with(|h| {
        let recorded =
            u32::try_from(h.ble_events.len()).expect("a test records fewer than 2^32 events");
        if seq == 0 || seq > recorded {
            BlePoll::Pending
        } else if recorded - seq >= h.ble_ring {
            BlePoll::Lost
        } else {
            BlePoll::Event(h.ble_events[seq as usize - 1])
        }
    })
}

fn drain(queue: &mut VecDeque<u8>, buffer: &mut [u8]) -> usize {
    let n = queue.len().min(buffer.len());
    for (slot, byte) in buffer.iter_mut().zip(queue.drain(..n)) {
        *slot = byte;
    }
    n
}

pub fn ble_control_read(buffer: &mut [u8]) -> usize {
    with(|h| drain(&mut h.ble_control, buffer))
}

#[must_use]
pub fn ble_notify(data: &[u8]) -> bool {
    with(|h| {
        if h.ble_notify_fails {
            h.ble_stats.notify_dropped += u32::try_from(data.len()).unwrap_or(u32::MAX);
            return false;
        }
        h.ble_notified.extend_from_slice(data);
        true
    })
}

pub fn ble_advertise(advertising: Advertising) -> bool {
    with(|h| h.ble_advertising = advertising);
    true
}

pub fn ble_disconnect() -> bool {
    with(|h| h.ble_disconnects += 1);
    true
}

pub fn ble_stats() -> BleStats {
    with(|h| h.ble_stats)
}

pub fn api_listen(_port: u16) -> Option<i32> {
    with(|h| (!h.tcp_listen_fails).then_some(200))
}

/// Takes the next waiting connection.
pub fn api_accept(_listener: i32) -> Accepted {
    with(|h| {
        h.api_waiting
            .pop_front()
            .map_or(Accepted::None, Accepted::Socket)
    })
}

pub fn api_recv(socket: i32, buffer: &mut [u8]) -> ApiRecv {
    with(|h| {
        let waiting = h.api_rx.entry(socket).or_default();
        if waiting.is_empty() {
            return if h.api_hung_up.contains(&socket) {
                ApiRecv::Closed
            } else {
                ApiRecv::Empty
            };
        }
        let n = waiting.len().min(buffer.len());
        for (out, byte) in buffer.iter_mut().zip(waiting.drain(..n)) {
            *out = byte;
        }
        ApiRecv::Data(n)
    })
}

pub fn api_send(socket: i32, data: &[u8]) -> Option<usize> {
    with(|h| {
        if h.tcp_send_fails || h.api_hung_up.contains(&socket) {
            return None;
        }
        let n = data.len().min(h.api_send_room);
        h.api_tx
            .entry(socket)
            .or_default()
            .extend_from_slice(&data[..n]);
        Some(n)
    })
}

pub fn mdns_open() -> Option<i32> {
    with(|h| (!h.tcp_listen_fails).then_some(300))
}

/// The fake's sockets 300 (mDNS) and 301 (NTP) keep their datagrams apart.
const UDP: i32 = 301;

pub fn udp_open() -> Option<i32> {
    with(|h| (!h.tcp_listen_fails).then_some(UDP))
}

pub fn rtc_registers() -> [u32; 4] {
    with(|h| h.rtc)
}

pub fn gateway() -> Option<u32> {
    with(|h| (h.gateway != 0).then_some(h.gateway))
}

pub fn mdns_recv(socket: i32, buffer: &mut [u8]) -> Datagram {
    with(|h| {
        match if socket == UDP {
            h.udp_rx.pop_front()
        } else {
            h.mdns_rx.pop_front()
        } {
            None => Datagram::None,
            Some((data, ip, port)) => {
                let len = data.len().min(buffer.len());
                buffer[..len].copy_from_slice(&data[..len]);
                Datagram::Got { len, ip, port }
            }
        }
    })
}

#[must_use]
pub fn mdns_send(socket: i32, data: &[u8], ip: u32, port: u16) -> bool {
    with(|h| {
        let sent = if socket == UDP {
            &mut h.udp_tx
        } else {
            &mut h.mdns_tx
        };
        sent.push((data.to_vec(), ip, port));
    });
    true
}

pub fn ipv4() -> Option<u32> {
    with(|h| (h.ipv4 != 0).then_some(h.ipv4))
}
