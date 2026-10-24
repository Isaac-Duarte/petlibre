//! KM4 calls into the pinned AmebaD SDK.
//!
//! The target half of the crate: each function wraps one SDK/ROM routine or
//! one fixed register access and matches the signature in `fake::api`, so the
//! ports build unchanged for host tests. The `const` asserts pin the SDK
//! constants (register bases, enable bits, 1 kHz tick) the ports were
//! reviewed against; a different SDK fails the build rather than the board.
#![allow(unsafe_code)]

use core::ffi::{CStr, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{
    AUDIO_PAGE_SAMPLES, FlashMmuEntry, GpioRouting, IR_PA_BIT, IR_PB_BIT, Irq, Isr, LpClock,
    MOTOR_MASK, SECTOR_BYTES, STORAGE_OFFSET, STORAGE_SECTORS, SYSTICK_CTRL, Systick, TaskStep,
    TcpRecv, TransportConfig, WifiEvent, WifiPoll,
};
use super::{Accepted, ApiRecv, Datagram};
#[cfg(feature = "bluetooth")]
use super::{Advertising, BleEvent, BlePoll, BleService, BleStats};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all,
    clippy::undocumented_unsafe_blocks
)]
mod raw {
    include!(concat!(env!("OUT_DIR"), "/sdk.rs"));
}

use raw::wellfed_sdk_constant as k;

pub const GPIO_PORT_A: u32 = k::WELLFED_GPIO_PORT_A;
pub const PMU_USER_BASE: u32 = k::WELLFED_PMU_DEV_USER_BASE;
pub const PMU_MAX: u32 = k::WELLFED_PMU_MAX;

pub const TIM_CR_CNT_START: u32 = k::WELLFED_TIM_CR_CNT_START;
pub const TIM_CR_CNT_RUN: u32 = k::WELLFED_TIM_CR_CNT_RUN;
pub const TIM_CR_CNT_STS: u32 = k::WELLFED_TIM_CR_CNT_STS;
pub const TIM_IT_UPDATE: u32 = k::WELLFED_TIM_IT_UPDATE;

pub const WDG_BIT_ENABLE: u32 = k::WELLFED_WDG_BIT_ENABLE;
pub const WDG_BIT_RST_MODE: u32 = k::WELLFED_WDG_BIT_RST_MODE;
pub const WDG_BIT_ISR_CLEAR: u32 = k::WELLFED_WDG_BIT_ISR_CLEAR;
pub const HS_WDTRST_OPT: u32 = k::WELLFED_BIT_HSYS_PLFM_WDTRST_OPT;
pub const LP_WDT_RESET_HS_ALL: u32 = k::WELLFED_BIT_LSYS_WDT_RESET_HS_ALL;

pub const BACKUP_USER: u32 = k::WELLFED_BKUP_REG1;
pub const BOOT_KM4_WATCHDOG: u32 = k::WELLFED_BOOT_KM4WDG_RESET;
pub const BOOT_KM4_SYSTEM: u32 = k::WELLFED_BOOT_KM4SYS_RESET;

const _: () = assert!(
    k::WELLFED_WDG_BASE == 0x4000_2800,
    "requires the KM4 watchdog mapping"
);

const _: () = assert!(
    k::WELLFED_TICK_RATE_HZ == 1000,
    "owner polling requires the pinned 1 kHz SDK tick"
);

fn state(enabled: bool) -> u32 {
    if enabled {
        k::WELLFED_ENABLE
    } else {
        k::WELLFED_DISABLE
    }
}

fn reg(address: u32) -> *mut u32 {
    address as usize as *mut u32
}

const _: () = assert!(
    k::WELLFED_GPIO_FUNC_BIT == super::GPIO_ENABLE_BIT
        && k::WELLFED_GPIO_CLOCK_BIT == super::GPIO_ENABLE_BIT
);

const _: () = assert!(k::WELLFED_BIT_SWD_PMUX == super::SWD_PMUX_BIT);

/// LP `REG_SWD_PMUX_EN`.
pub fn swd_pinmux() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_SWD_PMUX)) }
}

/// Release PA27/PB3 from SWD as stock does (`sys_jtag_off`, before it
/// configures PB3), preserving the other bits. Returns the readback.
pub fn swd_off() -> u32 {
    let _guard = super::IrqGuard::lock();
    let before = swd_pinmux();

    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        ptr::write_volatile(
            reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_SWD_PMUX),
            before & !super::SWD_PMUX_BIT,
        )
    };

    swd_pinmux()
}

const _: () = assert!(
    k::WELLFED_ADC_FUNC_BIT == super::ADC_ENABLE_BIT
        && k::WELLFED_ADC_CLOCK_BIT == super::ADC_ENABLE_BIT
);

fn lp_clock_state(bit: u32) -> LpClock {
    let function =
        // SAFETY: a fixed register in the pinned SDK memory map.
        unsafe { ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_FUNC)) };
    let clock =
        // SAFETY: a fixed register in the pinned SDK memory map.
        unsafe { ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_CLOCK)) };
    LpClock {
        bit,
        function_before: function,
        clock_before: clock,
        function_after: function,
        clock_after: clock,
    }
}

pub fn gpio_clock_state() -> LpClock {
    lp_clock_state(super::GPIO_ENABLE_BIT)
}

/// The KM4 RCC entry controls HS registers. GPIO and the ADC belong to LP
/// SYSCTRL; use its SDK register constants directly. Match the LP ROM enable
/// order: clock first, then function, preserving every other bit (no reset).
fn lp_clock_enable(bit: u32) -> LpClock {
    let _guard = super::IrqGuard::lock();
    let before = lp_clock_state(bit);

    let clock = reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_CLOCK);
    let function = reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_FUNC);
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::write_volatile(clock, before.clock_before | bit) };
    // SAFETY: as above.
    unsafe { ptr::write_volatile(function, before.function_before | bit) };

    cortex_m::asm::dsb();
    LpClock {
        function_before: before.function_before,
        clock_before: before.clock_before,
        ..lp_clock_state(bit)
    }
}

pub fn gpio_clock_enable() -> LpClock {
    lp_clock_enable(super::GPIO_ENABLE_BIT)
}

pub fn adc_clock_enable() -> LpClock {
    lp_clock_enable(super::ADC_ENABLE_BIT)
}

// Every call below is a bounded SDK/ROM routine or a
// volatile access to a fixed peripheral register in the pinned memory map;
// callers provide the serialization the C ports documented.

pub fn primask() -> u32 {
    u32::from(cortex_m::register::primask::read().is_inactive())
}

pub fn ipsr() -> u32 {
    let value: u32;
    // SAFETY: reads IPSR only.
    unsafe {
        core::arch::asm!("mrs {}, IPSR", out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

pub fn disable_irq() {
    cortex_m::interrupt::disable();
}

pub fn set_primask(mask: u32) {
    if mask & 1 == 0 {
        // SAFETY: unmasks only when restoring a mask the caller saved unmasked.
        unsafe { cortex_m::interrupt::enable() };
    } else {
        cortex_m::interrupt::disable();
    }
}

pub fn gpio_port_write(port: u32, mask: u32, value: u32) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::GPIO_PortWrite(port, mask, value) }
}

pub fn gpio_port_read(port: u32, mask: u32) -> u32 {
    // The pinned ROM returns the entire EXT_PORT word, ignoring its mask argument.
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::GPIO_PortRead(port, mask) & mask }
}

pub fn gpio_routing() -> GpioRouting {
    let _guard = super::IrqGuard::lock();
    let gpio_a = k::WELLFED_GPIOA_BASE as usize as *const raw::GPIO_TypeDef;
    let gpio_b = (k::WELLFED_GPIOA_BASE + 0x400) as usize as *const raw::GPIO_TypeDef;
    GpioRouting {
        // SAFETY: a fixed register in the pinned SDK memory map.
        groups: unsafe {
            ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_GPIO_GROUPS))
        },
        // SAFETY: a fixed register in the pinned SDK memory map.
        aon: unsafe { ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_AON_PAD)) },
        // SAFETY: a fixed register in the pinned SDK memory map.
        control_a: unsafe { ptr::read_volatile(ptr::addr_of!((*gpio_a).PORT[0].CTRL)) },
        // SAFETY: a fixed register in the pinned SDK memory map.
        control_b: unsafe { ptr::read_volatile(ptr::addr_of!((*gpio_b).PORT[0].CTRL)) },
        // SAFETY: a fixed register in the pinned SDK memory map.
        share_pad: unsafe {
            ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_SHARE_PAD))
        },
        ext_a: gpioa_ext_port(),
    }
}

pub fn gpio_direction(port: u32) -> u32 {
    assert!(port < 2);
    let gpio = (k::WELLFED_GPIOA_BASE + port * 0x400) as usize as *const raw::GPIO_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpio).PORT[0].DDR)) }
}

pub fn gpio_output_latch(port: u32) -> u32 {
    assert!(port < 2);
    let gpio = (k::WELLFED_GPIOA_BASE + port * 0x400) as usize as *const raw::GPIO_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpio).PORT[0].DR)) }
}

pub fn pad_control(pin: u32) -> u32 {
    assert!(pin < 64);
    let mux = k::WELLFED_PINMUX_BASE as usize as *const raw::PINMUX_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*mux).PADCTR[pin as usize])) }
}

/// Remove pad pull resistors explicitly; ROM GPIO_Init skips this for outputs.
pub fn gpio_no_pull(pin: u32) {
    assert!(pin < 64);
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::PAD_PullCtrl(pin as u8, k::WELLFED_GPIO_PUPD_NOPULL as u8) }
}

pub fn gpio_init(pin: u32, output: bool) {
    let mut settings = raw::GPIO_InitTypeDef {
        GPIO_Pin: pin,
        GPIO_Mode: if output {
            k::WELLFED_GPIO_MODE_OUT
        } else {
            k::WELLFED_GPIO_MODE_IN
        },
        GPIO_PuPd: k::WELLFED_GPIO_PUPD_NOPULL,
        ..Default::default()
    };
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::GPIO_Init(&mut settings) }
}

pub fn gpio_write_bit(pin: u32, high: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::GPIO_WriteBit(pin, u32::from(high)) }
}

pub fn gpio_read_bit(pin: u32) -> bool {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::GPIO_ReadDataBit(pin) != 0 }
}

pub fn efuse_read8(address: u32) -> Option<u8> {
    let mut value = 0u8;
    let ok =
        // SAFETY: the pinned SDK routine, writing only through a live local.
        unsafe { raw::EFUSERead8(0, address, &mut value, k::WELLFED_L25EOUTVOLTAGE as u8) } != 0;
    ok.then_some(value)
}

pub fn pad_disable(pin: u8) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::PAD_CMD(pin, k::WELLFED_DISABLE as u8) }
}

/// Initialize the ADC with an empty conversion list (manual channel select).
pub fn adc_init_manual() {
    let mut settings = raw::ADC_InitTypeDef::default();
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::ADC_StructInit(&mut settings) };
    settings.ADC_CvlistLen = 0;
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::ADC_Init(&mut settings) }
}

pub fn adc_cmd(enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::ADC_Cmd(state(enabled)) }
}

pub fn adc_select(channel: u32) {
    let adc = k::WELLFED_ADC_BASE as usize as *mut raw::ADC_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::write_volatile(ptr::addr_of_mut!((*adc).ADC_CHSW_LIST[0]), channel) }
}

pub fn adc_clear_fifo() {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::ADC_ClearFIFO() }
}

pub fn adc_sw_trigger(enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::ADC_SWTrigCmd(state(enabled)) }
}

pub fn adc_readable() -> bool {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::ADC_Readable() != 0 }
}

pub fn adc_read() -> u16 {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::ADC_Read() }
}

pub fn systimer_ticks() -> u32 {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::SYSTIMER_TickGet() }
}

pub fn systick() -> Systick {
    // SAFETY: the architectural reload and current-value registers after
    // SYSTICK_CTRL, read only; FreeRTOS owns the timer.
    let reload = unsafe { ptr::read_volatile(reg(SYSTICK_CTRL + 4)) } & 0x00ff_ffff;
    // SAFETY: as above.
    let value = unsafe { ptr::read_volatile(reg(SYSTICK_CTRL + 8)) } & 0x00ff_ffff;
    Systick { value, reload }
}

/// The FreeRTOS heap's free bytes now.
pub fn heap_free() -> u32 {
    // SAFETY: the pinned SDK routine, with no arguments.
    let free = unsafe { raw::xPortGetFreeHeapSize() };
    u32::try_from(free).unwrap_or(u32::MAX)
}

/// The least the FreeRTOS heap has had free since boot.
pub fn heap_min_free() -> u32 {
    // SAFETY: the pinned SDK routine, with no arguments.
    let free = unsafe { raw::xPortGetMinimumEverFreeHeapSize() };
    u32::try_from(free).unwrap_or(u32::MAX)
}

pub fn gtimer_clock_enable() {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe {
        raw::RCC_PeriphClockCmd(
            k::WELLFED_APB_GTIMER,
            k::WELLFED_APB_GTIMER_CLOCK,
            k::WELLFED_ENABLE as u8,
        )
    }
}

pub fn gpio_int_config(pin: u32, enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::GPIO_INTConfig(pin, state(enabled)) }
}

/// Both-edge, active-low, debounced interrupt mode.
pub fn gpio_int_mode(pin: u32, enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe {
        raw::GPIO_INTMode(
            pin,
            state(enabled),
            k::WELLFED_GPIO_INT_BOTHEDGE,
            k::WELLFED_GPIO_INT_ACTIVE_LOW,
            k::WELLFED_GPIO_INT_DEBOUNCE,
        )
    }
}

fn gpioa() -> *mut raw::GPIO_TypeDef {
    k::WELLFED_GPIOA_BASE as usize as *mut raw::GPIO_TypeDef
}

macro_rules! gpioa_read {
    ($($name:ident => $field:ident),*) => {$(
        pub fn $name() -> u32 {
            // SAFETY: a fixed register in the pinned SDK memory map.
            unsafe { ptr::read_volatile(ptr::addr_of!((*gpioa()).$field)) }
        }
    )*};
}
gpioa_read!(gpioa_int_status => INT_STATUS, gpioa_int_en => INT_EN, gpioa_int_mask => INT_MASK,
    gpioa_int_bothedge => INT_BOTHEDGE);

pub fn gpioa_ext_port() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpioa()).EXT_PORT[0])) }
}

pub fn gpioa_eoi(mask: u32) {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::write_volatile(ptr::addr_of_mut!((*gpioa()).PORTA_EOI), mask) }
}

/// Best-effort output stop for the panic handler. Masks interrupts, then, only
/// when the LP GPIO block reads clocked and enabled, sets the motor latches
/// PA13/PA14 HIGH (STOP) and the infrared latches PA15/PB3 LOW with direct
/// read-modify-write of the data registers. It makes no SDK/ROM call, takes no
/// lock and has no panic path; directions are left unchanged, so a pad not
/// yet configured as an output is not newly driven. KM0 can still race these
/// registers, and nothing is verified: the caller stops here regardless.
pub fn panic_outputs_safe() {
    cortex_m::interrupt::disable();
    let clock = gpio_clock_state();
    if clock.function_after & super::GPIO_ENABLE_BIT == 0
        || clock.clock_after & super::GPIO_ENABLE_BIT == 0
    {
        return;
    }

    let gpio_b = (k::WELLFED_GPIOA_BASE + 0x400) as usize as *mut raw::GPIO_TypeDef;
    // SAFETY: the two GPIO port blocks in the pinned SDK memory map.
    let (a, b) = unsafe {
        (
            ptr::addr_of_mut!((*gpioa()).PORT[0].DR),
            ptr::addr_of_mut!((*gpio_b).PORT[0].DR),
        )
    };

    // SAFETY: Port A's data register; interrupts are masked above.
    unsafe { ptr::write_volatile(a, (ptr::read_volatile(a) | MOTOR_MASK) & !IR_PA_BIT) };
    // SAFETY: Port B's data register, likewise.
    unsafe { ptr::write_volatile(b, ptr::read_volatile(b) & !IR_PB_BIT) };

    cortex_m::asm::dsb();
}

fn tim2() -> *mut raw::RTIM_TypeDef {
    k::WELLFED_TIM2_BASE as usize as *mut raw::RTIM_TypeDef
}

pub fn tim2_en() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).EN)) }
}

pub fn tim2_dier() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).DIER)) }
}

pub fn tim2_arr() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).ARR)) }
}

/// Overflow-updating TIM2 time base without ARR protection or SDK callback.
pub fn tim2_init(period: u32) {
    let mut settings = raw::RTIM_TimeBaseInitTypeDef::default();
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::RTIM_TimeBaseStructInit(&mut settings) };
    settings.TIM_Idx = 2;
    settings.TIM_Period = period;
    settings.TIM_UpdateEvent = k::WELLFED_ENABLE;
    settings.TIM_UpdateSource = k::WELLFED_TIM_UPDATE_OVERFLOW;
    settings.TIM_ARRProtection = k::WELLFED_DISABLE;
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::RTIM_TimeBaseInit(tim2(), &mut settings, raw::TIMER2_IRQ, None, 0) }
}

pub fn tim2_int_config(enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::RTIM_INTConfig(tim2(), k::WELLFED_TIM_IT_UPDATE, state(enabled)) }
}

pub fn tim2_cmd(enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::RTIM_Cmd(tim2(), state(enabled)) }
}

pub fn tim2_int_clear() {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::RTIM_INTClear(tim2()) }
}

fn irqn(irq: Irq) -> raw::IRQn_Type {
    match irq {
        Irq::Timer2 => raw::TIMER2_IRQ,
        Irq::GpioA => raw::GPIOA_IRQ,
    }
}

/// `InterruptRegister`, i.e. the SDK's inline `irq_register_check`.
pub fn irq_register(irq: Irq, isr: Isr, priority: u32) -> bool {
    let priority = priority.min(k::WELLFED_MAX_IRQ_PRIORITY) << k::WELLFED_IRQ_PRIORITY_SHIFT;
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::irq_register(Some(isr), irqn(irq), 0, priority) != 0 }
}

pub fn irq_unregister(irq: Irq) -> bool {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::irq_unregister(irqn(irq)) != 0 }
}

/// `InterruptEn(irq, priority)`: the SDK macro ignores the priority.
pub fn irq_enable(irq: Irq, _priority: u32) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::irq_enable(irqn(irq)) }
}

pub fn irq_disable(irq: Irq) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::irq_disable(irqn(irq)) }
}

pub fn wakelock_acquire(id: u32) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::pmu_acquire_wakelock(id) }
}

pub fn wakelock_release(id: u32) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::pmu_release_wakelock(id) }
}

pub fn wdg_vendor() -> u32 {
    let wdg = k::WELLFED_WDG_BASE as usize as *const raw::WDG_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(ptr::addr_of!((*wdg).VENDOR)) }
}

/// Reset-mode watchdog that also resets all HS peripherals.
pub fn wdg_init(count_id: u32, divisor: u32) {
    let mut settings = raw::WDG_InitTypeDef {
        CountProcess: count_id,
        DivFacProcess: divisor,
        RstAllPERI: 1,
    };
    // SAFETY: the pinned SDK routine, writing only through a live local.
    unsafe { raw::WDG_Init(&mut settings) }
}

pub fn wdg_cmd(enabled: bool) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::WDG_Cmd(state(enabled)) }
}

pub fn wdg_refresh() {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::WDG_Refresh() }
}

/// KM0 backup register; the SDK leaves REG1-REG5 to the user. Expected to
/// survive a watchdog platform reset but not power loss (to be observed).
pub fn backup_read(index: u32) -> u32 {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::BKUP_Read(index) }
}

pub fn backup_write(index: u32, value: u32) {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::BKUP_Write(index, value) }
}

pub fn boot_reason() -> u32 {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::BOOT_Reason() }
}

/// The flash JEDEC ID through the SDK's RAM `FLASH_RxCmdXIP`, whose lock and
/// unlock calls resolve to the hook interposer. Read-only (RDID, 0x9F).
pub fn flash_read_jedec() -> [u8; 3] {
    let mut id = [0u8; 3];
    // SAFETY: the SDK's RAM flash read of 3 ID bytes into the live `id`.
    unsafe { raw::FLASH_RxCmdXIP(0x9f, 3, id.as_mut_ptr()) };
    id
}

const _: () = assert!(
    STORAGE_OFFSET.is_multiple_of(SECTOR_BYTES)
        && STORAGE_OFFSET >= 0x30_6000
        && STORAGE_OFFSET + STORAGE_SECTORS * SECTOR_BYTES <= 0x3e_8000
);

const _: () = assert!(k::WELLFED_SPI_FLASH_BASE == 0x0800_0000 && k::WELLFED_ERASE_SECTOR == 2);

fn storage_span(sector: u32, offset: u32, len: usize) -> Option<u32> {
    let end = offset.checked_add(u32::try_from(len).ok()?)?;
    (sector < STORAGE_SECTORS && end <= SECTOR_BYTES)
        .then(|| STORAGE_OFFSET + sector * SECTOR_BYTES + offset)
}

/// Sector erase through the SDK's RAM `FLASH_EraseXIP` (hooked lock, erase,
/// cache flush, unlock).
pub fn storage_erase(sector: u32) -> bool {
    let Some(address) = storage_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    // SAFETY: the SDK's RAM flash routine, inside the span checked above; the page buffer is live.
    unsafe { raw::FLASH_EraseXIP(k::WELLFED_ERASE_SECTOR, address) };
    true
}

/// Program through the SDK's RAM `FLASH_TxData256BXIP`, split at 256-byte
/// pages. Offset and length must be 4-byte aligned and stay in the sector.
pub fn storage_program(sector: u32, offset: u32, data: &[u8]) -> bool {
    if storage_span(sector, offset, data.len()).is_none()
        || !offset.is_multiple_of(4)
        || !data.len().is_multiple_of(4)
    {
        return false;
    }

    let mut done = 0;
    while done < data.len() {
        let at = offset + done as u32;
        let chunk = (256 - (at % 256) as usize).min(data.len() - done);
        let mut page = [0u8; 256];
        page[..chunk].copy_from_slice(&data[done..done + chunk]);
        let address = STORAGE_OFFSET + sector * SECTOR_BYTES + at;
        // SAFETY: the SDK's RAM flash routine, inside the span checked above; the page buffer is live.
        unsafe { raw::FLASH_TxData256BXIP(address, chunk as u32, page.as_mut_ptr()) };
        done += chunk;
    }

    true
}

/// A bank-1 address for `sector`/`offset`/`len`, only while running from
/// bank 0: the updater never writes the bank it executes from.
fn bank1_span(sector: u32, offset: u32, len: usize) -> Option<u32> {
    let end = offset.checked_add(u32::try_from(len).ok()?)?;
    (flash_mmu_entry0().bank() == Some(0) && sector < super::BANK1_SECTORS && end <= SECTOR_BYTES)
        .then(|| super::BANK1_OFFSET + sector * SECTOR_BYTES + offset)
}

/// Erase one bank-1 sector through the hooked `FLASH_EraseXIP`.
pub fn bank1_erase(sector: u32) -> bool {
    let Some(address) = bank1_span(sector, 0, SECTOR_BYTES as usize) else {
        return false;
    };
    // SAFETY: the SDK's RAM flash routine, inside the span checked above; the page buffer is live.
    unsafe { raw::FLASH_EraseXIP(k::WELLFED_ERASE_SECTOR, address) };
    true
}

/// Program within one bank-1 sector, split at 256-byte pages; 4-byte aligned.
pub fn bank1_program(sector: u32, offset: u32, data: &[u8]) -> bool {
    let Some(base) = bank1_span(sector, offset, data.len()) else {
        return false;
    };

    if !offset.is_multiple_of(4) || !data.len().is_multiple_of(4) {
        return false;
    }

    let mut done = 0;
    while done < data.len() {
        let at = base + done as u32;
        let chunk = (256 - (at % 256) as usize).min(data.len() - done);
        let mut page = [0u8; 256];
        page[..chunk].copy_from_slice(&data[done..done + chunk]);
        // SAFETY: the SDK's RAM flash routine, inside the span checked above; the page buffer is live.
        unsafe { raw::FLASH_TxData256BXIP(at, chunk as u32, page.as_mut_ptr()) };
        done += chunk;
    }
    true
}

/// Read bank 1 through the physical flash window.
pub fn bank1_read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
    let Some(address) = bank1_span(sector, offset, bytes.len()) else {
        return false;
    };
    let base = k::WELLFED_SPI_FLASH_BASE + address;
    for (i, byte) in bytes.iter_mut().enumerate() {
        // SAFETY: inside the memory-mapped flash window, at an offset checked above.
        *byte = unsafe { ptr::read_volatile((base as usize + i) as *const u8) };
    }
    true
}

/// FW10 give-way from the running normal image: clear only the first byte
/// of bank 1's marker (one page program; bytes 1-3 rewritten unchanged, as
/// NOR programming only clears bits), through the hooked lock. Refused
/// unless running from bank 1 with bank 1's marker intact and bank 0's
/// marker valid, so the loader always has a bank to boot.
pub fn bank1_give_way() -> bool {
    let read = |offset: u32| {
        let mut out = [0u8; 8];
        for (i, byte) in out.iter_mut().enumerate() {
            let at = k::WELLFED_SPI_FLASH_BASE + offset + i as u32;
            // SAFETY: inside the memory-mapped flash window, at an offset checked above.
            *byte = unsafe { ptr::read_volatile(at as usize as *const u8) };
        }
        out
    };

    let bank1 = read(super::BANK1_OFFSET);
    if flash_mmu_entry0().bank() != Some(1)
        || bank1 != super::MARKER
        || read(super::BANK0_OFFSET) != super::MARKER
    {
        return false;
    }

    let mut word = [0x00, bank1[1], bank1[2], bank1[3]];
    // SAFETY: the SDK's RAM flash routine on bank 1's first word, checked above; `word` is live.
    unsafe { raw::FLASH_TxData256BXIP(super::BANK1_OFFSET, 4, word.as_mut_ptr()) };
    read(super::BANK1_OFFSET)[0] == 0
}

/// Read through the memory-mapped flash window; the SDK wrappers flush the
/// cache after every erase or program.
pub fn storage_read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
    let Some(address) = storage_span(sector, offset, bytes.len()) else {
        return false;
    };
    let base = k::WELLFED_SPI_FLASH_BASE + address;
    for (i, byte) in bytes.iter_mut().enumerate() {
        // SAFETY: inside the memory-mapped flash window, at an offset checked above.
        *byte = unsafe { ptr::read_volatile((base as usize + i) as *const u8) };
    }
    true
}

/// Raw latch writes requesting motor STOP (PA13/PA14 HIGH) and infrared OFF
/// (PA15/PB3 LOW). Always inlined so a RAM-resident caller stays leaf code
/// with no call or flash literal (audited).
#[inline(always)]
pub fn ram_outputs_safe() {
    let port_a = k::WELLFED_GPIOA_BASE as usize as *mut raw::GPIO_TypeDef;
    let port_b = (k::WELLFED_GPIOA_BASE + 0x400) as usize as *mut raw::GPIO_TypeDef;
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        let a = ptr::addr_of_mut!((*port_a).PORT[0].DR);
        let b = ptr::addr_of_mut!((*port_b).PORT[0].DR);
        // Separate volatile read-modify-writes (motor STOP first) keep the
        // audited RAM code to plain loads, ORR/BIC and stores.
        ptr::write_volatile(a, ptr::read_volatile(a) | MOTOR_MASK);
        ptr::write_volatile(a, ptr::read_volatile(a) & !IR_PA_BIT);
        ptr::write_volatile(b, ptr::read_volatile(b) & !IR_PB_BIT);
        core::arch::asm!("dsb", options(nostack, preserves_flags));
    }
}

/// KM4 IPC block: current interrupt enables (IER_R) and status (ISR).
pub fn ipc_interrupts() -> (u32, u32) {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        (
            ptr::read_volatile(reg(0x4000_601c)),
            ptr::read_volatile(reg(0x4000_6014)),
        )
    }
}

/// SysTick CTRL; a lease stops it while the peer is parked.
pub fn systick_ctrl() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(reg(SYSTICK_CTRL)) }
}

pub fn set_systick_ctrl(value: u32) {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::write_volatile(reg(SYSTICK_CTRL), value) }
}

/// One busy-wait iteration with interrupts left as they are.
pub fn spin_once() {
    cortex_m::asm::nop();
}

pub fn hs_platform_para() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        ptr::read_volatile(reg(
            k::WELLFED_SYSCTRL_HP + k::WELLFED_REG_HS_PLATFORM_PARA
        ))
    }
}

pub fn set_hs_platform_para(value: u32) {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        ptr::write_volatile(
            reg(k::WELLFED_SYSCTRL_HP + k::WELLFED_REG_HS_PLATFORM_PARA),
            value,
        )
    }
}

pub fn lp_km0_ctrl() -> u32 {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe { ptr::read_volatile(reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_KM0_CTRL)) }
}

pub fn set_lp_km0_ctrl(value: u32) {
    // SAFETY: a fixed register in the pinned SDK memory map.
    unsafe {
        ptr::write_volatile(
            reg(k::WELLFED_SYSCTRL_LP + k::WELLFED_REG_LP_KM0_CTRL),
            value,
        )
    }
}

struct TaskSlot {
    claimed: AtomicBool,
    step: core::cell::UnsafeCell<Option<TaskStep>>,
    context: core::cell::UnsafeCell<*mut c_void>,
}

// SAFETY: a slot's cells are written only by its claimant, before the task
// that reads them exists; the context pointer they carry crosses to that task
// under `task_create`'s contract.
unsafe impl Sync for TaskSlot {}

static TASKS: [TaskSlot; 8] = [const {
    TaskSlot {
        claimed: AtomicBool::new(false),
        step: core::cell::UnsafeCell::new(None),
        context: core::cell::UnsafeCell::new(ptr::null_mut()),
    }
}; 8];

unsafe extern "C" fn trampoline(argument: *mut c_void) {
    // SAFETY: `argument` is the static `TaskSlot` that `task_create` passed.
    let slot = unsafe { &*argument.cast::<TaskSlot>() };
    // SAFETY: the claimant filled both cells before creating this task, and nothing writes them until it ends.
    let (step, context) = unsafe { ((*slot.step.get()).unwrap_unchecked(), *slot.context.get()) };

    loop {
        // SAFETY: `task_create`'s contract for `step` and `context`.
        let delay = unsafe { step(context) };
        if delay == 0 {
            break;
        }
        // configTICK_RATE_HZ is 1000, so pdMS_TO_TICKS(ms) == ms.
        // SAFETY: the pinned SDK routine, with plain-value arguments.
        unsafe { raw::vTaskDelay(delay) };
    }

    slot.claimed.store(false, Ordering::Release);
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::vTaskDelete(ptr::null_mut()) }
}

/// Create a FreeRTOS task that repeatedly runs `step` until it returns zero.
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
    let Some(slot) = TASKS.iter().find(|s| {
        s.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }) else {
        return false;
    };

    // SAFETY: the compare-exchange above claimed the slot, so only this call writes it until the task ends.
    unsafe {
        *slot.step.get() = Some(step);
        *slot.context.get() = context;
    }

    // SAFETY: the pinned SDK routine; `name` is static and the argument is a static slot.
    let created = unsafe {
        raw::xTaskCreate(
            Some(trampoline),
            name.as_ptr(),
            stack_words,
            ptr::from_ref(slot).cast_mut().cast(),
            k::WELLFED_IDLE_PRIORITY + priority,
            ptr::null_mut(),
        )
    } == k::WELLFED_PD_PASS as raw::BaseType_t;

    if !created {
        slot.claimed.store(false, Ordering::Release);
    }
    created
}

unsafe extern "C" {
    fn wellfed_wifi_start() -> i32;
    fn wellfed_wifi_event(seq: u32, out: *mut WifiEvent) -> i32;
}

/// Start the KM4 Wi-Fi shim's task (join and keep the configured network);
/// true once, when the task was created.
pub fn wifi_start() -> bool {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_wifi_start() != 0 }
}

pub fn wifi_event(seq: u32) -> WifiPoll {
    let mut event = WifiEvent::default();
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    match unsafe { wellfed_wifi_event(seq, &mut event) } {
        1 => WifiPoll::Event(event),
        0 => WifiPoll::Pending,
        _ => WifiPoll::Lost,
    }
}

#[repr(C)]
struct RawTransportConfig {
    broker: u32,
    port: u32,
    client_id: *const core::ffi::c_char,
    username: *const core::ffi::c_char,
    password: *const core::ffi::c_char,
    password_len: u32,
    namespace: *const core::ffi::c_char,
    /// 64 lowercase hex characters, or empty without a key.
    request_key: *const core::ffi::c_char,
}

unsafe extern "C" {
    fn wellfed_transport_config(out: *mut RawTransportConfig);
    fn wellfed_entropy(out: *mut [u32; 4]);
    fn wellfed_tcp_connect(ip: u32, port: u32, timeout_ms: u32) -> i32;
    fn wellfed_tcp_send(socket: i32, data: *const u8, length: u32) -> i32;
    fn wellfed_tcp_recv(socket: i32, buffer: *mut u8, length: u32, timeout_ms: u32) -> i32;
    fn wellfed_tcp_close(socket: i32);
    fn wellfed_tcp_listen(port: u32) -> i32;
    fn wellfed_tcp_accept(listener: i32, timeout_ms: u32) -> i32;
}

/// The broker settings compiled into this image; `None` when a string field
/// is not UTF-8 (the build validates them, so that is a build defect).
pub fn transport_config() -> Option<TransportConfig> {
    let mut raw = RawTransportConfig {
        broker: 0,
        port: 0,
        client_id: ptr::null(),
        username: ptr::null(),
        password: ptr::null(),
        password_len: 0,
        namespace: ptr::null(),
        request_key: ptr::null(),
    };

    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_transport_config(&mut raw) };

    // SAFETY: the shim returns pointers to NUL-terminated string literals.
    let text = |p: *const core::ffi::c_char| unsafe { CStr::from_ptr(p) }.to_str().ok();
    Some(TransportConfig {
        broker: raw.broker,
        port: u16::try_from(raw.port).ok()?,
        client_id: text(raw.client_id)?,
        username: text(raw.username)?,
        // SAFETY: a string literal of exactly `password_len` bytes.
        password: unsafe {
            core::slice::from_raw_parts(raw.password.cast(), raw.password_len as usize)
        },
        namespace: text(raw.namespace)?,
        // The build writes 64 hex characters or nothing; anything else is a defect.
        request_key: match text(raw.request_key)? {
            "" => None,
            key => Some(hex_key(key.as_bytes())?),
        },
    })
}

fn hex_key(hex: &[u8]) -> Option<[u8; 32]> {
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    if hex.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, pair) in hex.as_chunks::<2>().0.iter().enumerate() {
        key[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(key)
}

unsafe extern "C" {
    fn wellfed_audio_open(rate_hz: u32) -> i32;
    fn wellfed_audio_page() -> *mut c_void;
    fn wellfed_audio_submit();
    fn wellfed_audio_drained() -> i32;
    fn wellfed_audio_underruns_seen() -> u32;
    fn wellfed_audio_close();
}

/// FW24: start the codec's line output and its DMA ring (`example_entry.c`)
/// at `rate_hz` (8, 16, 32 or 48 kHz); false for another rate, when already
/// open or when no DMA channel was free.
pub fn audio_open(rate_hz: u32) -> bool {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_audio_open(rate_hz) != 0 }
}

/// The next free ring page, while the codec is open and the DMA does not
/// own every page. Fill it, then [`audio_submit`]; one page at a time.
pub fn audio_page() -> Option<&'static mut [i16; AUDIO_PAGE_SAMPLES]> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let page = unsafe { wellfed_audio_page() };
    // SAFETY: a 32-byte-aligned ring page of `AUDIO_PAGE_SAMPLES` samples the
    // DMA does not own until `audio_submit`.
    (!page.is_null()).then(|| unsafe { &mut *page.cast() })
}

pub fn audio_submit() {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_audio_submit() }
}

/// Every submitted page has played.
pub fn audio_drained() -> bool {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_audio_drained() != 0 }
}

/// Zero pages played for want of a filled one since the open.
pub fn audio_underruns() -> u32 {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_audio_underruns_seen() }
}

pub fn audio_close() {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_audio_close() }
}

/// Inputs for a session's request nonce: the ROM generator and timer values
/// at the moment of a connect (`wellfed_entropy` in `example_entry.c`).
/// Not a vetted RNG; the nonce only has to differ between sessions.
pub fn entropy() -> [u32; 4] {
    let mut out = [0; 4];
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_entropy(&mut out) };
    out
}

/// A connected TCP socket, or `None` after `timeout_ms`.
pub fn tcp_connect(ip: u32, port: u16, timeout_ms: u32) -> Option<i32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let socket = unsafe { wellfed_tcp_connect(ip, u32::from(port), timeout_ms) };
    (socket >= 0).then_some(socket)
}

/// Send all of `data`; false on error or a stalled peer.
#[must_use]
pub fn tcp_send(socket: i32, data: &[u8]) -> bool {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_tcp_send(socket, data.as_ptr(), data.len() as u32) == 0 }
}

pub fn tcp_recv(socket: i32, buffer: &mut [u8], timeout_ms: u32) -> TcpRecv {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    match unsafe { wellfed_tcp_recv(socket, buffer.as_mut_ptr(), buffer.len() as u32, timeout_ms) }
    {
        0 => TcpRecv::Timeout,
        n if n > 0 => TcpRecv::Data(n as usize),
        _ => TcpRecv::Closed,
    }
}

pub fn tcp_close(socket: i32) {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_tcp_close(socket) }
}

/// A listening socket on `port`, or `None`.
pub fn tcp_listen(port: u16) -> Option<i32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let socket = unsafe { wellfed_tcp_listen(u32::from(port)) };
    (socket >= 0).then_some(socket)
}

/// A connection accepted within `timeout_ms`, or `None`.
pub fn tcp_accept(listener: i32, timeout_ms: u32) -> Option<i32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let socket = unsafe { wellfed_tcp_accept(listener, timeout_ms) };
    (socket >= 0).then_some(socket)
}

unsafe extern "C" {
    fn wellfed_api_listen(port: u32) -> i32;
    fn wellfed_api_accept(listener: i32) -> i32;
    fn wellfed_api_recv(socket: i32, buffer: *mut u8, length: u32) -> i32;
    fn wellfed_api_send(socket: i32, data: *const u8, length: u32) -> i32;
    fn wellfed_mdns_open() -> i32;
    fn wellfed_mdns_recv(
        socket: i32,
        buffer: *mut u8,
        length: u32,
        ip: *mut u32,
        port: *mut u32,
    ) -> i32;
    fn wellfed_mdns_send(socket: i32, data: *const u8, length: u32, ip: u32, port: u32) -> i32;
    fn wellfed_ipv4() -> u32;
}

/// FW47: the device API's non-blocking listener on `port`, or `None`.
pub fn api_listen(port: u16) -> Option<i32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let socket = unsafe { wellfed_api_listen(u32::from(port)) };
    (socket >= 0).then_some(socket)
}

/// A waiting connection, never blocking; close it with [`tcp_close`].
pub fn api_accept(listener: i32) -> Accepted {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    match unsafe { wellfed_api_accept(listener) } {
        -1 => Accepted::None,
        socket if socket >= 0 => Accepted::Socket(socket),
        _ => Accepted::Error,
    }
}

/// What is waiting on `socket` now, never blocking.
pub fn api_recv(socket: i32, buffer: &mut [u8]) -> ApiRecv {
    let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    match unsafe { wellfed_api_recv(socket, buffer.as_mut_ptr(), length) } {
        0 => ApiRecv::Empty,
        n if n > 0 => ApiRecv::Data(n as usize),
        _ => ApiRecv::Closed,
    }
}

/// How much of `data` the stack took now (0 while it is full); `None` on
/// an error.
pub fn api_send(socket: i32, data: &[u8]) -> Option<usize> {
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let sent = unsafe { wellfed_api_send(socket, data.as_ptr(), length) };
    usize::try_from(sent).ok()
}

/// FW47: the mDNS socket (UDP 5353, joined to 224.0.0.251), or `None`.
pub fn mdns_open() -> Option<i32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let socket = unsafe { wellfed_mdns_open() };
    (socket >= 0).then_some(socket)
}

pub fn mdns_recv(socket: i32, buffer: &mut [u8]) -> Datagram {
    let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    let (mut ip, mut port) = (0, 0);
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    match unsafe { wellfed_mdns_recv(socket, buffer.as_mut_ptr(), length, &mut ip, &mut port) } {
        0 => Datagram::None,
        n if n > 0 => Datagram::Got {
            len: n as usize,
            ip,
            port: u16::try_from(port).unwrap_or(0),
        },
        _ => Datagram::Error,
    }
}

#[must_use]
pub fn mdns_send(socket: i32, data: &[u8], ip: u32, port: u16) -> bool {
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    unsafe { wellfed_mdns_send(socket, data.as_ptr(), length, ip, u32::from(port)) == 0 }
}

/// The station's IPv4 address (host order), once DHCP gave one.
pub fn ipv4() -> Option<u32> {
    // SAFETY: the C shim in `example_entry.c`; any buffer is live for the call.
    let ip = unsafe { wellfed_ipv4() };
    (ip != 0).then_some(ip)
}

/// Milliseconds since the scheduler started (configTICK_RATE_HZ is 1000).
pub fn uptime_ms() -> u32 {
    // SAFETY: the pinned SDK routine, with plain-value arguments.
    unsafe { raw::xTaskGetTickCount() }
}

/// The running task's identity, nonzero and stable for the task's life; zero
/// from an interrupt or before the scheduler starts. A task created after
/// another ended may reuse its handle.
pub fn current_task() -> usize {
    if ipsr() != 0
        // SAFETY: the pinned SDK routine, with plain-value arguments.
        || unsafe { raw::xTaskGetSchedulerState() } != k::WELLFED_SCHEDULER_RUNNING as _
    {
        return 0;
    }
    // SAFETY: the pinned SDK routine; it reads the current TCB pointer.
    unsafe { raw::xTaskGetCurrentTaskHandle() as usize }
}

/// One line, not split by another task's print: from a task, the scheduler
/// is paused for the print (interrupts keep running). Before the scheduler
/// starts, or from an interrupt, it prints directly.
pub fn log(message: &CStr) {
    // SAFETY: a NUL-terminated format and string.
    unscheduled(|| unsafe { raw::_rtl_printf(c"%s".as_ptr(), message.as_ptr()) });
}

/// Run `f` with no other task scheduled: from a task, the scheduler is
/// paused for the call (interrupts keep running); before the scheduler
/// starts, or from an interrupt, it runs directly. `f` must not block.
pub fn unscheduled<R>(f: impl FnOnce() -> R) -> R {
    let task = ipsr() == 0
        // SAFETY: the pinned SDK routine, with plain-value arguments.
        && unsafe { raw::xTaskGetSchedulerState() } == k::WELLFED_SCHEDULER_RUNNING as _;
    if task {
        // SAFETY: the pinned SDK routine, with plain-value arguments.
        unsafe { raw::vTaskSuspendAll() };
    }
    let result = f();
    if task {
        // SAFETY: the pinned SDK routine, with plain-value arguments.
        unsafe { raw::xTaskResumeAll() };
    }
    result
}

/// Read the live KM0 image mapping. No flash/status/register writes are made
/// by this observation.
pub fn flash_mmu_entry0() -> FlashMmuEntry {
    let rsip = k::WELLFED_RSIP_BASE as usize as *const raw::RSIP_REG_TypeDef;
    // SAFETY: fixed pinned SDK MMIO layout; these registers are read-only here.
    unsafe {
        FlashMmuEntry {
            control: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_CTRL)),
            start: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_STRADDR)),
            offset: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_OFFSET)),
        }
    }
}

/// What `wellfed_ble.c` copies at start.
#[cfg(feature = "bluetooth")]
#[repr(C)]
struct RawBleConfig {
    name: *const u8,
    name_len: u32,
    service: [u8; 16],
    control: [u8; 16],
}

#[cfg(feature = "bluetooth")]
unsafe extern "C" {
    fn wellfed_ble_start(config: *const RawBleConfig) -> i32;
    fn wellfed_ble_event_read(seq: u32, out: *mut BleEvent) -> i32;
    fn wellfed_ble_control_read(buffer: *mut u8, length: u32) -> u32;
    fn wellfed_ble_notify(data: *const u8, length: u32) -> i32;
    fn wellfed_ble_disconnect() -> i32;
    fn wellfed_ble_advertise(on: i32) -> i32;
    fn wellfed_ble_stats(out: *mut BleStats);
}

/// FW34: start the BLE peripheral's task (`wellfed_ble.c`), which waits for
/// the Wi-Fi radio, then serves `service` (advertising only when asked); true
/// once, when the task was created.
#[cfg(feature = "bluetooth")]
pub fn ble_start(service: &BleService) -> bool {
    let config = RawBleConfig {
        name: service.name.as_ptr(),
        name_len: u32::try_from(service.name.len()).unwrap_or(0),
        service: service.service,
        control: service.control,
    };
    // SAFETY: the C shim copies the config during the call; the name is static.
    unsafe { wellfed_ble_start(&config) != 0 }
}

#[cfg(feature = "bluetooth")]
pub fn ble_event(seq: u32) -> BlePoll {
    let mut event = BleEvent::default();
    // SAFETY: the C shim in `wellfed_ble.c`; any buffer is live for the call.
    match unsafe { wellfed_ble_event_read(seq, &mut event) } {
        1 => BlePoll::Event(event),
        0 => BlePoll::Pending,
        _ => BlePoll::Lost,
    }
}

/// Control bytes written since the connection began.
#[cfg(feature = "bluetooth")]
pub fn ble_control_read(buffer: &mut [u8]) -> usize {
    let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: the C shim copies at most `length` bytes into the live buffer.
    let n = unsafe { wellfed_ble_control_read(buffer.as_mut_ptr(), length) };
    usize::try_from(n).unwrap_or(0)
}

/// Queue `data` as control notifications; false when the queue is full.
#[cfg(feature = "bluetooth")]
#[must_use]
pub fn ble_notify(data: &[u8]) -> bool {
    let Ok(length) = u32::try_from(data.len()) else {
        return false;
    };
    // SAFETY: the C shim copies the bytes during the call.
    unsafe { wellfed_ble_notify(data.as_ptr(), length) != 0 }
}

/// Advertise, or stop, whenever no host is connected; false when the BLE
/// task is not running yet.
#[cfg(feature = "bluetooth")]
pub fn ble_advertise(advertising: Advertising) -> bool {
    let on = i32::from(advertising == Advertising::On);
    // SAFETY: the C shim in `wellfed_ble.c`, with a plain value.
    unsafe { wellfed_ble_advertise(on) != 0 }
}

/// Ask the BLE task to drop the connection.
#[cfg(feature = "bluetooth")]
pub fn ble_disconnect() -> bool {
    // SAFETY: the C shim in `wellfed_ble.c`, with no arguments.
    unsafe { wellfed_ble_disconnect() != 0 }
}

#[cfg(feature = "bluetooth")]
pub fn ble_stats() -> BleStats {
    let mut stats = BleStats::default();
    // SAFETY: the C shim in `wellfed_ble.c`; any buffer is live for the call.
    unsafe { wellfed_ble_stats(&mut stats) };
    stats
}
