//! Target builds only: the C entry points of the RAM park and the IPC
//! semaphore over this core's real registers (KM0 or KM4, by the `km0`
//! feature). Like the rest of the park, they stay leaf code in RAM text.

use super::*;

pub struct Hardware;

impl Mmio for Hardware {
    const KM0: bool = cfg!(feature = "km0");

    #[inline(always)]
    fn load(address: u32) -> u32 {
        // SAFETY: fixed IPC/timer/semaphore/SysTick registers.
        unsafe { ptr::read_volatile(address as usize as *const u32) }
    }

    #[inline(always)]
    fn store(address: u32, value: u32) {
        // SAFETY: as for `load`.
        unsafe { ptr::write_volatile(address as usize as *mut u32, value) }
    }

    #[inline(always)]
    fn barrier() {
        // SAFETY: a data synchronization barrier with a memory clobber.
        unsafe { core::arch::asm!("dsb", options(nostack, preserves_flags)) }
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
    fn halt() -> ! {
        loop {
            // SAFETY: a no-op that keeps the RAM spin loop observable.
            unsafe { core::arch::asm!("nop", options(nostack, preserves_flags)) }
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub unsafe extern "C" fn petlibre_amebad_flash_park_publish(
    token: *const ParkToken,
    command: u32,
) -> bool {
    // SAFETY: the C caller passes null or a readable token.
    unsafe { publish::<Hardware>(token, command) }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub unsafe extern "C" fn petlibre_amebad_flash_park_wait(
    token: *const ParkToken,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: as for `petlibre_amebad_flash_park_publish`.
    unsafe { wait::<Hardware>(token, tick_limit, poll_limit) }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub extern "C" fn petlibre_amebad_flash_sem_try_claim(index: u32) -> bool {
    sem_try_claim::<Hardware>(index)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub extern "C" fn petlibre_amebad_flash_sem_release(index: u32) -> bool {
    sem_release::<Hardware>(index)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub unsafe extern "C" fn petlibre_amebad_flash_owner_begin(
    token: *const ParkToken,
    channel: u32,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: as for `petlibre_amebad_flash_park_publish`.
    unsafe { owner_begin::<Hardware>(token, channel, tick_limit, poll_limit) }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".image2.ram.text.petlibre_park")]
#[inline(never)]
pub unsafe extern "C" fn petlibre_amebad_flash_owner_end(
    token: *const ParkToken,
    channel: u32,
    tick_limit: u32,
    poll_limit: u32,
) -> bool {
    // SAFETY: as for `petlibre_amebad_flash_park_publish`.
    unsafe { owner_end::<Hardware>(token, channel, tick_limit, poll_limit) }
}
