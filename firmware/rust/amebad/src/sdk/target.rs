//! KM4 calls into the pinned AmebaD SDK.
#![allow(unsafe_code)]

use core::ffi::{CStr, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{GpioClock, GpioRouting, Irq, Isr, TaskStep};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod raw {
    include!(concat!(env!("OUT_DIR"), "/sdk.rs"));
}
use raw::petlibro_sdk_constant as k;

pub const GPIO_PORT_A: u32 = k::PETLIBRO_GPIO_PORT_A;
pub const PMU_USER_BASE: u32 = k::PETLIBRO_PMU_DEV_USER_BASE;
pub const PMU_MAX: u32 = k::PETLIBRO_PMU_MAX;
pub const TIM_CR_CNT_START: u32 = k::PETLIBRO_TIM_CR_CNT_START;
pub const TIM_CR_CNT_RUN: u32 = k::PETLIBRO_TIM_CR_CNT_RUN;
pub const TIM_CR_CNT_STS: u32 = k::PETLIBRO_TIM_CR_CNT_STS;
pub const TIM_IT_UPDATE: u32 = k::PETLIBRO_TIM_IT_UPDATE;
pub const WDG_BIT_ENABLE: u32 = k::PETLIBRO_WDG_BIT_ENABLE;
pub const WDG_BIT_RST_MODE: u32 = k::PETLIBRO_WDG_BIT_RST_MODE;
pub const WDG_BIT_ISR_CLEAR: u32 = k::PETLIBRO_WDG_BIT_ISR_CLEAR;
pub const HS_WDTRST_OPT: u32 = k::PETLIBRO_BIT_HSYS_PLFM_WDTRST_OPT;
pub const LP_WDT_RESET_HS_ALL: u32 = k::PETLIBRO_BIT_LSYS_WDT_RESET_HS_ALL;
pub const BACKUP_USER: u32 = k::PETLIBRO_BKUP_REG1;
pub const BOOT_KM4_WATCHDOG: u32 = k::PETLIBRO_BOOT_KM4WDG_RESET;
pub const BOOT_KM4_SYSTEM: u32 = k::PETLIBRO_BOOT_KM4SYS_RESET;

const _: () = assert!(
    k::PETLIBRO_WDG_BASE == 0x4000_2800,
    "requires the KM4 watchdog mapping"
);
const _: () = assert!(
    k::PETLIBRO_TICK_RATE_HZ == 1000,
    "owner polling requires the pinned 1 kHz SDK tick"
);

fn state(enabled: bool) -> u32 {
    if enabled {
        k::PETLIBRO_ENABLE
    } else {
        k::PETLIBRO_DISABLE
    }
}

fn reg(address: u32) -> *mut u32 {
    address as usize as *mut u32
}

const _: () = assert!(
    k::PETLIBRO_GPIO_FUNC_BIT == super::GPIO_ENABLE_BIT
        && k::PETLIBRO_GPIO_CLOCK_BIT == super::GPIO_ENABLE_BIT
);

pub fn gpio_clock_state() -> GpioClock {
    let function =
        unsafe { ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_GPIO_FUNC)) };
    let clock =
        unsafe { ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_GPIO_CLOCK)) };
    GpioClock {
        function_before: function,
        clock_before: clock,
        function_after: function,
        clock_after: clock,
    }
}

pub fn gpio_clock_enable() -> GpioClock {
    let _guard = super::IrqGuard::lock();
    let before = gpio_clock_state();
    // The KM4 RCC entry controls HS registers. GPIO belongs to LP SYSCTRL;
    // use its SDK register constants directly. Match the LP ROM enable order:
    // clock first, then function, preserving every other bit (no reset).
    unsafe {
        ptr::write_volatile(
            reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_GPIO_CLOCK),
            before.clock_before | k::PETLIBRO_GPIO_CLOCK_BIT,
        );
        ptr::write_volatile(
            reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_GPIO_FUNC),
            before.function_before | k::PETLIBRO_GPIO_FUNC_BIT,
        );
    }
    cortex_m::asm::dsb();
    GpioClock {
        function_before: before.function_before,
        clock_before: before.clock_before,
        ..gpio_clock_state()
    }
}

// SAFETY (module-wide): every call below is a bounded SDK/ROM routine or a
// volatile access to a fixed peripheral register in the pinned memory map;
// callers provide the serialization the C ports documented.

pub fn primask() -> u32 {
    u32::from(cortex_m::register::primask::read().is_inactive())
}

pub fn ipsr() -> u32 {
    let value: u32;
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
        unsafe { cortex_m::interrupt::enable() };
    } else {
        cortex_m::interrupt::disable();
    }
}

pub fn gpio_port_write(port: u32, mask: u32, value: u32) {
    unsafe { raw::GPIO_PortWrite(port, mask, value) }
}

pub fn gpio_port_read(port: u32, mask: u32) -> u32 {
    // The pinned ROM returns the entire EXT_PORT word, ignoring its mask argument.
    unsafe { raw::GPIO_PortRead(port, mask) & mask }
}

pub fn gpio_routing() -> GpioRouting {
    let _guard = super::IrqGuard::lock();
    let gpio_a = k::PETLIBRO_GPIOA_BASE as usize as *const raw::GPIO_TypeDef;
    let gpio_b = (k::PETLIBRO_GPIOA_BASE + 0x400) as usize as *const raw::GPIO_TypeDef;
    GpioRouting {
        groups: unsafe {
            ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_GPIO_GROUPS))
        },
        aon: unsafe { ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_AON_PAD)) },
        control_a: unsafe { ptr::read_volatile(ptr::addr_of!((*gpio_a).PORT[0].CTRL)) },
        control_b: unsafe { ptr::read_volatile(ptr::addr_of!((*gpio_b).PORT[0].CTRL)) },
        share_pad: unsafe {
            ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_SHARE_PAD))
        },
        ext_a: gpioa_ext_port(),
    }
}

pub fn gpio_direction(port: u32) -> u32 {
    assert!(port < 2);
    let gpio = (k::PETLIBRO_GPIOA_BASE + port * 0x400) as usize as *const raw::GPIO_TypeDef;
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpio).PORT[0].DDR)) }
}

pub fn gpio_output_latch(port: u32) -> u32 {
    assert!(port < 2);
    let gpio = (k::PETLIBRO_GPIOA_BASE + port * 0x400) as usize as *const raw::GPIO_TypeDef;
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpio).PORT[0].DR)) }
}

pub fn pad_control(pin: u32) -> u32 {
    assert!(pin < 64);
    let mux = k::PETLIBRO_PINMUX_BASE as usize as *const raw::PINMUX_TypeDef;
    unsafe { ptr::read_volatile(ptr::addr_of!((*mux).PADCTR[pin as usize])) }
}

/// Remove pad pull resistors explicitly; ROM GPIO_Init skips this for outputs.
pub fn gpio_no_pull(pin: u32) {
    assert!(pin < 64);
    unsafe { raw::PAD_PullCtrl(pin as u8, k::PETLIBRO_GPIO_PUPD_NOPULL as u8) }
}

pub fn gpio_init(pin: u32, output: bool) {
    let mut settings = raw::GPIO_InitTypeDef {
        GPIO_Pin: pin,
        GPIO_Mode: if output {
            k::PETLIBRO_GPIO_MODE_OUT
        } else {
            k::PETLIBRO_GPIO_MODE_IN
        },
        GPIO_PuPd: k::PETLIBRO_GPIO_PUPD_NOPULL,
        ..Default::default()
    };
    unsafe { raw::GPIO_Init(&mut settings) }
}

pub fn gpio_write_bit(pin: u32, high: bool) {
    unsafe { raw::GPIO_WriteBit(pin, u32::from(high)) }
}

pub fn gpio_read_bit(pin: u32) -> bool {
    unsafe { raw::GPIO_ReadDataBit(pin) != 0 }
}

pub fn efuse_read8(address: u32) -> Option<u8> {
    let mut value = 0u8;
    let ok =
        unsafe { raw::EFUSERead8(0, address, &mut value, k::PETLIBRO_L25EOUTVOLTAGE as u8) } != 0;
    ok.then_some(value)
}

pub fn pad_disable(pin: u8) {
    unsafe { raw::PAD_CMD(pin, k::PETLIBRO_DISABLE as u8) }
}

/// Initialize the ADC with an empty conversion list (manual channel select).
pub fn adc_init_manual() {
    let mut settings = raw::ADC_InitTypeDef::default();
    unsafe { raw::ADC_StructInit(&mut settings) };
    settings.ADC_CvlistLen = 0;
    unsafe { raw::ADC_Init(&mut settings) }
}

pub fn adc_cmd(enabled: bool) {
    unsafe { raw::ADC_Cmd(state(enabled)) }
}

pub fn adc_select(channel: u32) {
    let adc = k::PETLIBRO_ADC_BASE as usize as *mut raw::ADC_TypeDef;
    unsafe { ptr::write_volatile(ptr::addr_of_mut!((*adc).ADC_CHSW_LIST[0]), channel) }
}

pub fn adc_clear_fifo() {
    unsafe { raw::ADC_ClearFIFO() }
}

pub fn adc_sw_trigger(enabled: bool) {
    unsafe { raw::ADC_SWTrigCmd(state(enabled)) }
}

pub fn adc_readable() -> bool {
    unsafe { raw::ADC_Readable() != 0 }
}

pub fn adc_read() -> u16 {
    unsafe { raw::ADC_Read() }
}

pub fn systimer_ticks() -> u32 {
    unsafe { raw::SYSTIMER_TickGet() }
}

pub fn gtimer_clock_enable() {
    unsafe {
        raw::RCC_PeriphClockCmd(
            k::PETLIBRO_APB_GTIMER,
            k::PETLIBRO_APB_GTIMER_CLOCK,
            k::PETLIBRO_ENABLE as u8,
        )
    }
}

pub fn gpio_int_config(pin: u32, enabled: bool) {
    unsafe { raw::GPIO_INTConfig(pin, state(enabled)) }
}

/// Both-edge, active-low, debounced interrupt mode.
pub fn gpio_int_mode(pin: u32, enabled: bool) {
    unsafe {
        raw::GPIO_INTMode(
            pin,
            state(enabled),
            k::PETLIBRO_GPIO_INT_BOTHEDGE,
            k::PETLIBRO_GPIO_INT_ACTIVE_LOW,
            k::PETLIBRO_GPIO_INT_DEBOUNCE,
        )
    }
}

fn gpioa() -> *mut raw::GPIO_TypeDef {
    k::PETLIBRO_GPIOA_BASE as usize as *mut raw::GPIO_TypeDef
}

macro_rules! gpioa_read {
    ($($name:ident => $field:ident),*) => {$(
        pub fn $name() -> u32 {
            unsafe { ptr::read_volatile(ptr::addr_of!((*gpioa()).$field)) }
        }
    )*};
}
gpioa_read!(gpioa_int_status => INT_STATUS, gpioa_int_en => INT_EN, gpioa_int_mask => INT_MASK,
    gpioa_int_bothedge => INT_BOTHEDGE);

pub fn gpioa_ext_port() -> u32 {
    unsafe { ptr::read_volatile(ptr::addr_of!((*gpioa()).EXT_PORT[0])) }
}

pub fn gpioa_eoi(mask: u32) {
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
    let gpio_b = (k::PETLIBRO_GPIOA_BASE + 0x400) as usize as *mut raw::GPIO_TypeDef;
    unsafe {
        let a = ptr::addr_of_mut!((*gpioa()).PORT[0].DR);
        ptr::write_volatile(
            a,
            (ptr::read_volatile(a) | (1 << 13) | (1 << 14)) & !(1 << 15),
        );
        let b = ptr::addr_of_mut!((*gpio_b).PORT[0].DR);
        ptr::write_volatile(b, ptr::read_volatile(b) & !(1 << 3));
    }
    cortex_m::asm::dsb();
}

fn tim2() -> *mut raw::RTIM_TypeDef {
    k::PETLIBRO_TIM2_BASE as usize as *mut raw::RTIM_TypeDef
}

pub fn tim2_en() -> u32 {
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).EN)) }
}

pub fn tim2_dier() -> u32 {
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).DIER)) }
}

pub fn tim2_arr() -> u32 {
    unsafe { ptr::read_volatile(ptr::addr_of!((*tim2()).ARR)) }
}

/// Overflow-updating TIM2 time base without ARR protection or SDK callback.
pub fn tim2_init(period: u32) {
    let mut settings = raw::RTIM_TimeBaseInitTypeDef::default();
    unsafe { raw::RTIM_TimeBaseStructInit(&mut settings) };
    settings.TIM_Idx = 2;
    settings.TIM_Period = period;
    settings.TIM_UpdateEvent = k::PETLIBRO_ENABLE;
    settings.TIM_UpdateSource = k::PETLIBRO_TIM_UPDATE_OVERFLOW;
    settings.TIM_ARRProtection = k::PETLIBRO_DISABLE;
    unsafe { raw::RTIM_TimeBaseInit(tim2(), &mut settings, raw::TIMER2_IRQ, None, 0) }
}

pub fn tim2_int_config(enabled: bool) {
    unsafe { raw::RTIM_INTConfig(tim2(), k::PETLIBRO_TIM_IT_UPDATE, state(enabled)) }
}

pub fn tim2_cmd(enabled: bool) {
    unsafe { raw::RTIM_Cmd(tim2(), state(enabled)) }
}

pub fn tim2_int_clear() {
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
    let priority = priority.min(k::PETLIBRO_MAX_IRQ_PRIORITY) << k::PETLIBRO_IRQ_PRIORITY_SHIFT;
    unsafe { raw::irq_register(Some(isr), irqn(irq), 0, priority) != 0 }
}

pub fn irq_unregister(irq: Irq) -> bool {
    unsafe { raw::irq_unregister(irqn(irq)) != 0 }
}

/// `InterruptEn(irq, priority)`: the SDK macro ignores the priority.
pub fn irq_enable(irq: Irq, _priority: u32) {
    unsafe { raw::irq_enable(irqn(irq)) }
}

pub fn irq_disable(irq: Irq) {
    unsafe { raw::irq_disable(irqn(irq)) }
}

pub fn wakelock_acquire(id: u32) {
    unsafe { raw::pmu_acquire_wakelock(id) }
}

pub fn wakelock_release(id: u32) {
    unsafe { raw::pmu_release_wakelock(id) }
}

pub fn wdg_vendor() -> u32 {
    let wdg = k::PETLIBRO_WDG_BASE as usize as *const raw::WDG_TypeDef;
    unsafe { ptr::read_volatile(ptr::addr_of!((*wdg).VENDOR)) }
}

/// Reset-mode watchdog that also resets all HS peripherals.
pub fn wdg_init(count_id: u32, divisor: u32) {
    let mut settings = raw::WDG_InitTypeDef {
        CountProcess: count_id,
        DivFacProcess: divisor,
        RstAllPERI: 1,
    };
    unsafe { raw::WDG_Init(&mut settings) }
}

pub fn wdg_cmd(enabled: bool) {
    unsafe { raw::WDG_Cmd(state(enabled)) }
}

pub fn wdg_refresh() {
    unsafe { raw::WDG_Refresh() }
}

/// KM0 backup register; the SDK leaves REG1-REG5 to the user. Expected to
/// survive a watchdog platform reset but not power loss (to be observed).
pub fn backup_read(index: u32) -> u32 {
    unsafe { raw::BKUP_Read(index) }
}

pub fn backup_write(index: u32, value: u32) {
    unsafe { raw::BKUP_Write(index, value) }
}

pub fn boot_reason() -> u32 {
    unsafe { raw::BOOT_Reason() }
}

/// The flash JEDEC ID through the SDK's RAM `FLASH_RxCmdXIP`, whose lock and
/// unlock calls resolve to the hook interposer. Read-only (RDID, 0x9F).
pub fn flash_read_jedec() -> [u8; 3] {
    let mut id = [0u8; 3];
    unsafe { raw::FLASH_RxCmdXIP(0x9f, 3, id.as_mut_ptr()) };
    id
}

/// The one flash sector the FW06 stage E probe may erase or program: the
/// last 4 KiB of the inactive bank 0, blank in the research unit's backup.
/// Owner-approved scratch; nothing takes an address argument.
pub const SCRATCH_OFFSET: u32 = 0x18_5000;
pub const SCRATCH_WORDS: u32 = 1024;
pub const SCRATCH_PAGES: u32 = 16;

const _: () = assert!(SCRATCH_OFFSET % 0x1000 == 0 && SCRATCH_OFFSET + 0x1000 <= 0x18_6000);
const _: () = assert!(k::PETLIBRO_SPI_FLASH_BASE == 0x0800_0000 && k::PETLIBRO_ERASE_SECTOR == 2);

/// Sector erase of the scratch sector through the SDK's RAM `FLASH_EraseXIP`
/// (hooked lock, erase, cache flush, unlock).
pub fn scratch_erase() {
    unsafe { raw::FLASH_EraseXIP(k::PETLIBRO_ERASE_SECTOR, SCRATCH_OFFSET) }
}

/// Program one 256-byte page of the scratch sector through the SDK's RAM
/// `FLASH_TxData256BXIP`. Pages outside 0..16 are refused.
pub fn scratch_program(page: u32, data: &[u8; 256]) -> bool {
    if page >= SCRATCH_PAGES {
        return false;
    }
    let mut buffer = *data;
    unsafe { raw::FLASH_TxData256BXIP(SCRATCH_OFFSET + page * 256, 256, buffer.as_mut_ptr()) };
    true
}

/// One word of the scratch sector through the memory-mapped flash window;
/// the SDK wrappers flush the cache after every erase or program.
pub fn scratch_read(word: u32) -> u32 {
    if word >= SCRATCH_WORDS {
        return 0;
    }
    let address = k::PETLIBRO_SPI_FLASH_BASE + SCRATCH_OFFSET + word * 4;
    unsafe { ptr::read_volatile(address as usize as *const u32) }
}

/// Flash-lock provider fault callback: runs from RAM inside `hook_fail`, so
/// it is leaf code with no call or flash literal (audited). Raw latch writes
/// request motor STOP (PA13/PA14 HIGH) and infrared OFF (PA15/PB3 LOW); the
/// caller then halts in RAM for the watchdog.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibro_park")]
#[inline(never)]
pub unsafe extern "C" fn petlibro_amebad_flash_provider_fault(
    _context: *mut c_void,
    _reason: crate::flash_hooks::HookFault,
) {
    let port_a = k::PETLIBRO_GPIOA_BASE as usize as *mut raw::GPIO_TypeDef;
    let port_b = (k::PETLIBRO_GPIOA_BASE + 0x400) as usize as *mut raw::GPIO_TypeDef;
    unsafe {
        let a = ptr::addr_of_mut!((*port_a).PORT[0].DR);
        let b = ptr::addr_of_mut!((*port_b).PORT[0].DR);
        // Separate volatile read-modify-writes (motor STOP first) keep the
        // audited RAM code to plain loads, ORR/BIC and stores.
        ptr::write_volatile(a, ptr::read_volatile(a) | (1 << 13) | (1 << 14));
        ptr::write_volatile(a, ptr::read_volatile(a) & !(1 << 15));
        ptr::write_volatile(b, ptr::read_volatile(b) & !(1 << 3));
        core::arch::asm!("dsb", options(nostack, preserves_flags));
    }
}

/// KM4 IPC block: current interrupt enables (IER_R) and status (ISR).
pub fn ipc_interrupts() -> (u32, u32) {
    unsafe {
        (
            ptr::read_volatile(reg(0x4000_601c)),
            ptr::read_volatile(reg(0x4000_6014)),
        )
    }
}

/// SysTick CTRL; a lease stops it while the peer is parked.
pub fn systick_ctrl() -> u32 {
    unsafe { ptr::read_volatile(reg(0xe000_e010)) }
}

pub fn set_systick_ctrl(value: u32) {
    unsafe { ptr::write_volatile(reg(0xe000_e010), value) }
}

/// One busy-wait iteration with interrupts left as they are.
pub fn spin_once() {
    cortex_m::asm::nop();
}

pub fn hs_platform_para() -> u32 {
    unsafe {
        ptr::read_volatile(reg(
            k::PETLIBRO_SYSCTRL_HP + k::PETLIBRO_REG_HS_PLATFORM_PARA
        ))
    }
}

pub fn set_hs_platform_para(value: u32) {
    unsafe {
        ptr::write_volatile(
            reg(k::PETLIBRO_SYSCTRL_HP + k::PETLIBRO_REG_HS_PLATFORM_PARA),
            value,
        )
    }
}

pub fn lp_km0_ctrl() -> u32 {
    unsafe { ptr::read_volatile(reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_LP_KM0_CTRL)) }
}

pub fn set_lp_km0_ctrl(value: u32) {
    unsafe {
        ptr::write_volatile(
            reg(k::PETLIBRO_SYSCTRL_LP + k::PETLIBRO_REG_LP_KM0_CTRL),
            value,
        )
    }
}

struct TaskSlot {
    claimed: AtomicBool,
    step: core::cell::UnsafeCell<Option<TaskStep>>,
    context: core::cell::UnsafeCell<*mut c_void>,
}

// SAFETY: a slot's cells are written only by the claimant before task creation.
unsafe impl Sync for TaskSlot {}

const EMPTY: TaskSlot = TaskSlot {
    claimed: AtomicBool::new(false),
    step: core::cell::UnsafeCell::new(None),
    context: core::cell::UnsafeCell::new(ptr::null_mut()),
};
static TASKS: [TaskSlot; 2] = [EMPTY, EMPTY];

unsafe extern "C" fn trampoline(argument: *mut c_void) {
    let slot = unsafe { &*argument.cast::<TaskSlot>() };
    let (step, context) = unsafe { ((*slot.step.get()).unwrap_unchecked(), *slot.context.get()) };
    loop {
        let delay = unsafe { step(context) };
        if delay == 0 {
            break;
        }
        // configTICK_RATE_HZ is 1000, so pdMS_TO_TICKS(ms) == ms.
        unsafe { raw::vTaskDelay(delay) };
    }
    slot.claimed.store(false, Ordering::Release);
    unsafe { raw::vTaskDelete(ptr::null_mut()) }
}

/// Create a FreeRTOS task that repeatedly runs `step` until it returns zero.
pub fn task_create(
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
    unsafe {
        *slot.step.get() = Some(step);
        *slot.context.get() = context;
    }
    let created = unsafe {
        raw::xTaskCreate(
            Some(trampoline),
            name.as_ptr(),
            stack_words,
            ptr::from_ref(slot).cast_mut().cast(),
            k::PETLIBRO_IDLE_PRIORITY + priority,
            ptr::null_mut(),
        )
    } == k::PETLIBRO_PD_PASS as raw::BaseType_t;
    if !created {
        slot.claimed.store(false, Ordering::Release);
    }
    created
}

pub fn log(message: &CStr) {
    unsafe { raw::_rtl_printf(c"%s".as_ptr(), message.as_ptr()) };
}

/// Read the live KM0 image mapping and the factory-preserved ROM reset flags.
/// No flash/status/register writes are made by this observation.
pub fn startup_snapshot() -> crate::startup::Snapshot {
    let rsip = k::PETLIBRO_RSIP_BASE as usize as *const raw::RSIP_REG_TypeDef;
    // SAFETY: fixed pinned SDK MMIO layout; these registers are read-only here.
    unsafe {
        crate::startup::Snapshot {
            control: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_CTRL)),
            start: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_STRADDR)),
            offset: ptr::read_volatile(ptr::addr_of!((*rsip).FLASH_MMU[0].MMU_ENTRYx_OFFSET)),
            reset: raw::BOOT_Reason(),
        }
    }
}
