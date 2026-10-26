//! A breadcrumb that survives a silent watchdog reset.
//!
//! A hang that stops the watchdog feeder task itself (a vendor call that
//! spins with interrupts masked, or a task above the feeder's priority that
//! never yields) resets the platform without a word: nothing runs to say
//! why the feeding stopped. So the FreeRTOS tick hook leaves, every tick,
//! the task the tick interrupted and how long it has been since the feeder
//! task last ran, in a backup register, which survives the reset. The next
//! boot reports it once, before anything else runs, and clears it.
//!
//! **Spun or masked.** While interrupts run the tick keeps counting, so
//! `since` climbs past the feeder's 250 ms period to the watchdog's expiry
//! (at least 2.9 s): a task spun, or held the scheduler suspended
//! (`suspended`), and the task named is the one that kept the CPU. Once
//! interrupts are masked the tick stops too, and the register keeps the
//! value from just before: `since` inside a feed period or so, and the task
//! named is the one that masked them. A boot where the feeder stopped on
//! purpose (a restart, or a supervision decision it printed) says so
//! (`stopping`).
//!
//! **Register 2.** The SDK leaves backup registers 1–5 to the application,
//! and the rest are taken: 1 holds the flash attempt and update bits with
//! the last lease fault, 3 KM4's boot count, 4 KM0's boot count and the
//! boot trial, 5 KM0's heartbeat. Register 2 held KM0's park-handler
//! diagnostics, which no core or tool ever read back; KM0 no longer writes
//! it, so the tick hook is its one writer and the boot's [`take`] its one
//! reader. Backup registers clear on power loss, and a cold boot's zero is
//! no crumb.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::sdk;

/// The backup register the crumb lives in; see the module documentation.
pub const REGISTER: u32 = 2;

/// The FreeRTOS tick period (`configTICK_RATE_HZ` 1000).
const TICK_MS: u32 = 1;
/// `since` is kept in steps this long: six bits reach past the watchdog's
/// longest expiry (5.3 s).
pub const SINCE_STEP_MS: u32 = 128;
const SINCE_MAX: u32 = 0x3f;
/// The feeder unseen this long (about 2 s, a whole number of steps) while
/// the tick still ran is a spin: well past its 250 ms period and any
/// routine stall (the 0.45–0.72 s association stall), and below the
/// watchdog's shortest expiry less a feed period (2.65 s), so a spin that
/// lasts until the reset always reads as one.
pub const SPIN_MS: u32 = 16 * SINCE_STEP_MS;

/// The task-name prefix every runtime and firmware task shares; the crumb
/// keeps the characters after it.
const PREFIX: &[u8] = b"wellfed";
/// Characters of the name the crumb keeps.
pub const NAME_LEN: usize = 3;

const TAG: u32 = 0b101;
const TAG_SHIFT: u32 = 29;
const PREFIXED_BIT: u32 = 1 << 28;
/// Where each kept name character sits, first character highest.
const CHAR_SHIFTS: [u32; NAME_LEN] = [22, 16, 10];
const CHAR_MASK: u32 = 0x3f;
const SUSPENDED_BIT: u32 = 1 << 9;
const STOPPING_BIT: u32 = 1 << 8;
const SINCE_SHIFT: u32 = 2;
const CHECK_MASK: u32 = 0b11;

/// Tick-hook entries since the feeder task last ran.
static SINCE_TICKS: AtomicU32 = AtomicU32::new(0);
/// The feeder stopped feeding on purpose; set once, cleared by the reset.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// The FreeRTOS scheduler at the tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scheduler {
    #[default]
    Running,
    /// Suspended (`vTaskSuspendAll`): the tick hook still runs, no task
    /// switches.
    Suspended,
}

/// The watchdog feeder at the tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Feeder {
    #[default]
    Feeding,
    /// Stopped feeding on purpose; the reset that follows is its own.
    Stopping,
}

/// What the last tick before a reset saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Crumb {
    /// Up to [`NAME_LEN`] characters of the interrupted task's name, after
    /// [`PREFIX`] when `prefixed`; lowercase ASCII, zero-padded.
    pub name: [u8; NAME_LEN],
    pub prefixed: bool,
    pub scheduler: Scheduler,
    pub feeder: Feeder,
    /// Since the feeder task last ran, in [`SINCE_STEP_MS`] steps; the
    /// largest step means that long or longer.
    pub since_ms: u32,
}

/// How the boot before ended, as the crumb tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stall {
    /// The feeder stopped feeding on purpose, and said why then.
    Stopped,
    /// The tick ran on without the feeder: a task kept the CPU.
    Spin,
    /// The tick stopped with the feeder still running: interrupts masked.
    Masked,
}

impl Stall {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Spin => "spin",
            Self::Masked => "masked",
        }
    }
}

/// A name character in six bits: printable ASCII folded to upper case,
/// less 0x20, so zero (a space, or the name's end) ends the name.
fn char_code(byte: u8) -> u32 {
    let upper = byte.to_ascii_uppercase();
    if upper > b' ' && upper < 0x60 {
        u32::from(upper - b' ')
    } else {
        0
    }
}

/// Two bits folded from the other thirty, so a stray value in the register
/// (an older KM0's diagnostics) is rarely taken for a crumb.
const fn check(word: u32) -> u32 {
    let mut fold = 0;
    let mut rest = word >> SINCE_SHIFT;
    while rest != 0 {
        fold ^= rest & CHECK_MASK;
        rest >>= 2;
    }
    fold
}

impl Crumb {
    /// The crumb for a task named `name` (its bytes up to the terminator).
    pub fn new(name: &[u8], scheduler: Scheduler, feeder: Feeder, since_ms: u32) -> Self {
        let (prefixed, rest) = match name.strip_prefix(PREFIX) {
            Some(rest) => (true, rest),
            None => (false, name),
        };
        let mut kept = [0; NAME_LEN];
        for (slot, &byte) in kept.iter_mut().zip(rest) {
            if char_code(byte) == 0 {
                break;
            }
            *slot = byte.to_ascii_lowercase();
        }
        Self {
            name: kept,
            prefixed,
            scheduler,
            feeder,
            since_ms: (since_ms / SINCE_STEP_MS).min(SINCE_MAX) * SINCE_STEP_MS,
        }
    }

    pub fn encode(self) -> u32 {
        let mut word =
            (TAG << TAG_SHIFT) | ((self.since_ms / SINCE_STEP_MS).min(SINCE_MAX) << SINCE_SHIFT);
        for (&byte, shift) in self.name.iter().zip(CHAR_SHIFTS) {
            word |= char_code(byte) << shift;
        }
        for (set, bit) in [
            (self.prefixed, PREFIXED_BIT),
            (self.scheduler == Scheduler::Suspended, SUSPENDED_BIT),
            (self.feeder == Feeder::Stopping, STOPPING_BIT),
        ] {
            if set {
                word |= bit;
            }
        }
        word | check(word)
    }

    /// The crumb a register value holds, or `None` for zero (a cold boot)
    /// or a value no tick hook wrote.
    pub fn decode(word: u32) -> Option<Self> {
        if word >> TAG_SHIFT != TAG || word & CHECK_MASK != check(word & !CHECK_MASK) {
            return None;
        }

        let mut name = [0; NAME_LEN];
        for (slot, shift) in name.iter_mut().zip(CHAR_SHIFTS) {
            let code = (word >> shift) & CHAR_MASK;
            if code == 0 {
                break;
            }
            // Six bits plus 0x20 is printable ASCII, below 0x60.
            *slot = u8::try_from(code + 0x20)
                .unwrap_or(b'?')
                .to_ascii_lowercase();
        }
        Some(Self {
            name,
            prefixed: word & PREFIXED_BIT != 0,
            scheduler: if word & SUSPENDED_BIT != 0 {
                Scheduler::Suspended
            } else {
                Scheduler::Running
            },
            feeder: if word & STOPPING_BIT != 0 {
                Feeder::Stopping
            } else {
                Feeder::Feeding
            },
            since_ms: ((word >> SINCE_SHIFT) & SINCE_MAX) * SINCE_STEP_MS,
        })
    }

    pub fn stall(&self) -> Stall {
        if self.feeder == Feeder::Stopping {
            Stall::Stopped
        } else if self.since_ms >= SPIN_MS {
            Stall::Spin
        } else {
            Stall::Masked
        }
    }
}

impl core::fmt::Display for Crumb {
    /// `kind=… task=… since_ms=… suspended=…`, the task as far as kept.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "kind={} task=", self.stall().name())?;
        if self.prefixed {
            f.write_str("wellfed")?;
        }
        let kept = self.name.iter().take_while(|&&byte| byte != 0);
        for &byte in kept {
            write!(f, "{}", char::from(byte))?;
        }
        if !self.prefixed && self.name[0] == 0 {
            // A tick before any task ran, or a nameless one.
            f.write_str("none")?;
        }
        write!(
            f,
            " since_ms={} suspended={}",
            self.since_ms,
            u8::from(self.scheduler == Scheduler::Suspended)
        )
    }
}

/// The last boot's crumb, cleared so it is reported once. Call at startup,
/// before the scheduler runs the tick hook; `None` after a cold boot.
pub fn take() -> Option<Crumb> {
    let word = sdk::backup_read(REGISTER);
    if word != 0 {
        sdk::backup_write(REGISTER, 0);
    }
    Crumb::decode(word)
}

/// The watchdog feeder task ran: called on each of its passes.
pub fn fed() {
    SINCE_TICKS.store(0, Ordering::Relaxed);
}

/// The feeder stopped feeding on purpose; the reset that follows is its own.
pub fn stopping() {
    STOPPING.store(true, Ordering::Relaxed);
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    SINCE_TICKS.store(0, Ordering::Relaxed);
    STOPPING.store(false, Ordering::Relaxed);
}

/// The FreeRTOS tick hook's body, from the shim's `vApplicationTickHook`:
/// `name` is the interrupted task's name, `suspended` nonzero while the
/// scheduler is suspended. Interrupt context, every tick: a few loads, one
/// register write, nothing that blocks or allocates.
///
/// # Safety
/// `name` is null or points to a NUL-terminated task name (the running
/// task's FreeRTOS control block holds one).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_tick_hook(name: *const u8, suspended: i32) {
    // Only this interrupt adds and only the feeder task clears, so a clear
    // lost to a tick between the two halves is just one tick late.
    let ticks = SINCE_TICKS.load(Ordering::Relaxed).saturating_add(1);
    SINCE_TICKS.store(ticks, Ordering::Relaxed);

    let mut bytes = [0u8; PREFIX.len() + NAME_LEN];
    let mut len = 0;
    if !name.is_null() {
        while len < bytes.len() {
            // SAFETY: the caller's contract: readable up to its terminator,
            // which the loop stops at.
            let byte = unsafe { name.add(len).read() };
            if byte == 0 {
                break;
            }
            bytes[len] = byte;
            len += 1;
        }
    }

    let crumb = Crumb::new(
        &bytes[..len],
        if suspended != 0 {
            Scheduler::Suspended
        } else {
            Scheduler::Running
        },
        if STOPPING.load(Ordering::Relaxed) {
            Feeder::Stopping
        } else {
            Feeder::Feeding
        },
        ticks.saturating_mul(TICK_MS),
    );
    sdk::backup_write(REGISTER, crumb.encode());
}
