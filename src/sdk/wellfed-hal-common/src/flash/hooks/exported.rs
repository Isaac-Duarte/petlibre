//! Target builds only: the RAM-resident SDK flash hooks, their C entry points
//! and private state, in whichever core's image links this crate.
//!
//! The public `FLASH_Write_*` symbols here replace the SDK's, which the build
//! overlay renamed to `wellfed_sdk_flash_legacy_*`. The exported names are
//! what the firmware build audits, so they must not change.

use super::*;

const SYSTICK_CTRL: *mut u32 = crate::sdk::SYSTICK_CTRL as usize as *mut u32;

// Private RAM state; names are fixed by the firmware audit.
#[unsafe(no_mangle)]
static mut hook_transport: HookProvider = HookProvider {
    context: ptr::null_mut(),
    ready: None,
    acquire: None,
    release: None,
    peer: None,
    fault: None,
};

#[unsafe(no_mangle)]
static mut hook_ticket: HookTicket = HookTicket {
    id: 0,
    saved_systick: 0,
};

#[unsafe(no_mangle)]
static mut hook_last_id: u64 = 0;
#[unsafe(no_mangle)]
static mut hook_armed: bool = false;
#[unsafe(no_mangle)]
static mut hook_faulted: bool = false;
#[unsafe(no_mangle)]
static mut hook_legacy_locked: bool = false;
#[unsafe(no_mangle)]
static mut hook_lease_held: bool = false;

unsafe extern "C" {
    fn wellfed_sdk_flash_legacy_lock();
    fn wellfed_sdk_flash_legacy_unlock();
    fn wellfed_sdk_flash_legacy_ipc(data: *mut c_void, status: u32, channel: u32);
}

pub struct Hardware;

impl HookCpu for Hardware {
    #[inline(always)]
    fn cells() -> HookCells {
        // SAFETY: the private statics above, which nothing but the
        // interposer names.
        unsafe {
            HookCells {
                transport: RamCell::new(&raw mut hook_transport),
                ticket: RamCell::new(&raw mut hook_ticket),
                last_id: RamCell::new(&raw mut hook_last_id),
                armed: RamCell::new(&raw mut hook_armed),
                faulted: RamCell::new(&raw mut hook_faulted),
                legacy_locked: RamCell::new(&raw mut hook_legacy_locked),
                lease_held: RamCell::new(&raw mut hook_lease_held),
            }
        }
    }

    #[inline(always)]
    fn mask() -> u32 {
        let value: u32;
        // SAFETY: reads PRIMASK only.
        unsafe {
            core::arch::asm!("mrs {}, PRIMASK", out(reg) value, options(nostack, preserves_flags))
        };
        value
    }

    #[inline(always)]
    fn ipsr() -> u32 {
        let value: u32;
        // SAFETY: reads IPSR only.
        unsafe {
            core::arch::asm!("mrs {}, IPSR", out(reg) value, options(nomem, nostack, preserves_flags))
        };
        value
    }

    #[inline(always)]
    fn disable() {
        // SAFETY: masks interrupts; acts as a compiler barrier.
        unsafe { core::arch::asm!("cpsid i", options(nostack, preserves_flags)) }
    }

    #[inline(always)]
    fn restore(value: u32) {
        // SAFETY: restores a previously read PRIMASK value.
        unsafe {
            core::arch::asm!("msr PRIMASK, {}", in(reg) value, options(nostack, preserves_flags))
        }
    }

    #[inline(always)]
    fn tick_read() -> u32 {
        // SAFETY: the architectural SysTick control register.
        unsafe { ptr::read_volatile(SYSTICK_CTRL) }
    }

    #[inline(always)]
    fn tick_write(value: u32) {
        // SAFETY: as for `tick_read`.
        unsafe { ptr::write_volatile(SYSTICK_CTRL, value) }
    }

    #[inline(always)]
    fn fail(reason: HookFault) -> ! {
        hook_fail(reason)
    }

    #[inline(always)]
    fn spin() {
        // SAFETY: a no-op in the RAM halt loop.
        unsafe { core::arch::asm!("nop", options(nostack, preserves_flags)) }
    }

    #[inline(always)]
    fn legacy_lock() {
        // SAFETY: the renamed pinned SDK routine.
        unsafe { wellfed_sdk_flash_legacy_lock() }
    }

    #[inline(always)]
    fn legacy_unlock() {
        // SAFETY: as for `legacy_lock`.
        unsafe { wellfed_sdk_flash_legacy_unlock() }
    }

    // `data` is the SDK's opaque IPC message pointer, handed back unchanged
    // to the legacy handler it was meant for; nothing here dereferences it.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    #[inline(always)]
    fn legacy_ipc(data: *mut c_void, status: u32, channel: u32) {
        // SAFETY: as for `legacy_lock`.
        unsafe { wellfed_sdk_flash_legacy_ipc(data, status, channel) }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub extern "C" fn hook_fail(reason: HookFault) -> ! {
    fail_body::<Hardware>(reason)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub unsafe extern "C" fn wellfed_amebad_flash_hooks_install(
    provider: *const HookProvider,
) -> bool {
    // SAFETY: the C caller passes null or a readable provider.
    unsafe { install::<Hardware>(provider) }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub extern "C" fn wellfed_amebad_flash_hooks_armed() -> bool {
    armed::<Hardware>()
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub extern "C" fn FLASH_Write_Lock() {
    write_lock::<Hardware>()
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub extern "C" fn FLASH_Write_Unlock() {
    write_unlock::<Hardware>()
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_hooks")]
#[inline(never)]
pub unsafe extern "C" fn FLASH_Write_IPC_Int(data: *mut c_void, status: u32, channel: u32) {
    write_ipc::<Hardware>(data, status, channel)
}

/// Lease provider fault callback: runs from RAM inside `hook_fail`, so it is
/// leaf code with no call or flash literal (audited). It requests motor STOP
/// and infrared OFF; the caller then halts in RAM for the watchdog.
#[cfg(feature = "km4")]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.wellfed_park")]
#[inline(never)]
pub unsafe extern "C" fn wellfed_amebad_flash_provider_fault(
    _context: *mut c_void,
    _reason: HookFault,
) {
    crate::sdk::ram_outputs_safe();
}
