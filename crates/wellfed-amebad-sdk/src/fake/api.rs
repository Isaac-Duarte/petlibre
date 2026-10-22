//! The SDK function surface, mirroring `sdk::target`.
//!
//! Same names and signatures as the target build, backed by the per-thread
//! [`Hardware`] model. Constants are host stand-ins for the SDK's values. The
//! asserts encode how the ports must call the SDK (pins, priorities, task
//! shape, interrupt state), so a port that drifts fails its host tests.

use super::*;

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

pub const STORAGE_OFFSET: u32 = 0x34_0000;
pub const STORAGE_SECTORS: u32 = 16;
pub const SECTOR_BYTES: u32 = 0x1000;

fn storage_span(sector: u32, offset: u32, len: usize) -> Option<usize> {
    let end = offset as usize + len;
    (sector < STORAGE_SECTORS && end <= SECTOR_BYTES as usize)
        .then(|| (sector * SECTOR_BYTES + offset) as usize)
}

pub fn storage_erase(sector: u32) -> bool {
    let Some(at) = storage_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    with(|h| {
        h.storage[at..at + SECTOR_BYTES as usize].fill(u8::MAX);
        h.storage_erases[sector as usize] += 1;
    });
    true
}

pub fn storage_program(sector: u32, offset: u32, data: &[u8]) -> bool {
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

pub fn bank1_erase(sector: u32) -> bool {
    let Some(at) = bank1_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    with(|h| h.bank1[at..at + SECTOR_BYTES as usize].fill(u8::MAX));
    true
}

pub fn bank1_program(sector: u32, offset: u32, data: &[u8]) -> bool {
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

/// As the target: clear bank 1's first marker byte, only from bank 1 with
/// both markers valid.
pub fn bank1_give_way() -> bool {
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
        pad_input(h, port as usize) & mask
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
            10 => h.supply_adc,
            channel => panic!("unmodelled ADC channel {channel}"),
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

pub fn wdg_init(count_id: u32, divisor: u32) {
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

/// Mirrors the target's panic-path stop: mask, then raw latch writes only
/// when the GPIO block is clocked and enabled.
pub fn panic_outputs_safe() {
    with(|h| {
        h.interrupts = 1;
        let enabled = crate::GPIO_ENABLE_BIT;
        if h.lp_function & enabled == 0 || h.lp_clock & enabled == 0 {
            return;
        }
        h.output[0] = (h.output[0] | MOTOR_MASK) & !(1 << 15);
        h.output[1] &= !(1 << 3);
    });
}

pub fn log(message: &CStr) {
    print!("{}", message.to_string_lossy());
}

/// Fixed synthetic broker settings.
pub fn transport_config() -> Option<TransportConfig> {
    Some(FAKE_TRANSPORT)
}

pub const FAKE_TRANSPORT: TransportConfig = TransportConfig {
    broker: 0x0a2a_6701,
    port: 1883,
    client_id: "wellfed-fw-dev1",
    username: "wellfed-fw",
    password: b"0123456789abcdef0123456789abcdef",
    namespace: "wellfed/1/dev1/",
    request_key: Some(FAKE_REQUEST_KEY),
};

pub const FAKE_REQUEST_KEY: [u8; 32] = [0x42; 32];

/// Distinct on every call, like the board's connect-time inputs.
pub fn entropy() -> [u32; 4] {
    static CALLS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let call = CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    [call, uptime_ms(), systimer_ticks(), 0x5eed]
}

pub fn audio_open() -> bool {
    with(|h| {
        if h.audio_open || h.audio_open_fails {
            return false;
        }
        h.audio_open = true;
        h.audio_opens += 1;
        h.audio_underruns = 0;
        true
    })
}

pub fn audio_page() -> Option<&'static mut [i16; AUDIO_PAGE_SAMPLES]> {
    let page = with(|h| {
        if !h.audio_open || h.audio_queue.len() >= AUDIO_PAGES {
            return None;
        }
        if h.audio_scratch.is_null() {
            h.audio_scratch = Box::into_raw(Box::new([0; AUDIO_PAGE_SAMPLES]));
        }
        Some(h.audio_scratch)
    })?;
    // SAFETY: leaked for the thread; the port fills one page at a time.
    Some(unsafe { &mut *page })
}

pub fn audio_submit() {
    with(|h| {
        assert!(h.audio_open, "submit with the codec closed");
        // SAFETY: handed out by `audio_page` on this thread.
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

pub fn tcp_connect(_ip: u32, _port: u16, timeout_ms: u32) -> Option<i32> {
    with(|h| {
        let result = h.tcp_connects.pop_front().flatten();
        if result.is_none() {
            h.uptime_ms = h.uptime_ms.wrapping_add(timeout_ms);
        }
        result
    })
}

pub fn tcp_send(socket: i32, data: &[u8]) -> bool {
    with(|h| {
        if h.tcp_send_fails {
            return false;
        }
        h.tcp_tx.push((socket, data.to_vec()));
        true
    })
}

pub fn tcp_recv(_socket: i32, buffer: &mut [u8], timeout_ms: u32) -> TcpRecv {
    with(|h| match h.tcp_rx.pop_front() {
        None | Some(TcpRecvScript::Timeout) => {
            h.uptime_ms = h.uptime_ms.wrapping_add(timeout_ms);
            TcpRecv::Timeout
        }
        Some(TcpRecvScript::Closed) => TcpRecv::Closed,
        Some(TcpRecvScript::Data(mut data)) => {
            let n = data.len().min(buffer.len());
            buffer[..n].copy_from_slice(&data[..n]);
            if n < data.len() {
                h.tcp_rx.push_front(TcpRecvScript::Data(data.split_off(n)));
            }
            TcpRecv::Data(n)
        }
    })
}

pub fn tcp_close(socket: i32) {
    with(|h| h.tcp_closed.push(socket));
}

pub fn uptime_ms() -> u32 {
    with(|h| h.uptime_ms)
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
        let recorded = h.wifi_events.len() as u32;
        if seq == 0 || seq > recorded {
            WifiPoll::Pending
        } else if recorded - seq >= h.wifi_ring {
            WifiPoll::Lost
        } else {
            WifiPoll::Event(h.wifi_events[seq as usize - 1])
        }
    })
}
