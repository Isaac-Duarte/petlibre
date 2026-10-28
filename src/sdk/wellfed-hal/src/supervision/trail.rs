//! The call trail a silent hang leaves in RAM a watchdog reset keeps.
//!
//! The [crumb](super::crumb) names the task a hang left running, but not
//! where in it the CPU stalled. A stall with interrupts masked inside the
//! vendor Wi-Fi driver gives no fault and no backtrace, so the KM4 image
//! wraps a few vendor primitives at link time (`-Wl,--wrap`; the
//! builder's `TRAIL_WRAPS`), and each wrapper in `example_entry.c` notes its
//! call in a ring of [`ENTRIES`] entries in `wellfed_status_trail`, just
//! below the wall clock's time carry. The shim's Wi-Fi task also notes when
//! `wifi_connect` starts and returns. The driver masks interrupts with
//! `save_and_cli` around every packet buffer it allocates or frees, so the
//! ring turns over many times during a join; the rarer steps therefore
//! also go to one sticky [`Mark`] slot per kind, which that traffic never
//! evicts. The driver's `rtw_enter_critical`, which guards every register
//! access, is deliberately not wrapped: a note there stretched the first
//! WPA3 join past the watchdog feeder's 1 s supervision gap. The next boot reads the trail once, at
//! startup, prints it after a hang the crumb names (`masked` or `spin`), and
//! starts an empty one.
//!
//! **Layout** (words, shared with `example_entry.c`): the magic
//! [`sdk::TRAIL_MAGIC`], the KM4 boot count the start was given, the check
//! `!(magic ^ boot ^ version)`, the notes since the start, the wrapped critical
//! sections open now (one counter for the whole CPU) and three reserved
//! words; then [`MARKS`] marks of four words (the whole note number, then a
//! note's three words) and [`ENTRIES`] entries of three. A note's first
//! word holds the [`Step`] in bits 0–7, the open critical sections (this one
//! included, at most 127) from [`DEPTH_SHIFT`], an interrupt context at
//! [`ISR_SHIFT`] and the note's number's low 16 bits from [`SEQ_SHIFT`];
//! then the wrapped call's return address and its first argument (or the
//! step's result). Note `n` goes to slot `n % ENTRIES`.
//!
//! **Validity.** A power-on leaves whatever the RAM holds, and a build
//! without the trail uses the same RAM as heap: both are refused by the
//! magic and check, which folds in [`sdk::TRAIL_VERSION`] so a trail of
//! another layout is refused too. The boot count ties the trail to the boot
//! that wrote it: KM4 counts its boots in backup register 3 when it arms
//! the flash hooks, after the trail started, so only a trail whose count is
//! one less than this boot's is the last boot's ([`Refusal::OtherBoot`]
//! otherwise). Each entry's number must be the one its slot holds for the
//! count. A note claims its slot by incrementing the count, then stores
//! `lr`, `arg` and the numbered word last, so a reset in the middle of a
//! note leaves the slot with the number it held a ring earlier, which is
//! dropped and counted as `skipped`. A mark is rewritten with interrupts
//! masked, so no other note can mix with it: its first note word is cleared
//! first and its whole number stored last, so a reset in the middle leaves
//! a mark that is dropped. The count wraps at 2^32 notes, after which the
//! window covers only the notes since the wrap until it fills again.
//!
//! **In place.** The startup reads the trail where it lies, word by word
//! ([`Words`]), before it starts the boot's own: nothing notes until then,
//! and the startup's stack (the main stack, before the scheduler) holds no
//! copy of it.

use core::fmt;

use crate::flash_lock::provider::BOOT_COUNT_REGISTER;
use crate::sdk;

/// Entries in the ring.
pub const ENTRIES: usize = 64;
/// [`ENTRIES`] as the note numbers count.
const RING: u32 = 64;
/// Sticky marks, one per [`Mark`].
pub const MARKS: usize = 5;
const HEADER_WORDS: usize = 8;
const MARK_WORDS: usize = 4;
const ENTRY_WORDS: usize = 3;
const MARKS_AT: usize = HEADER_WORDS;
const ENTRIES_AT: usize = MARKS_AT + MARKS * MARK_WORDS;
const _: () = assert!(ENTRIES_AT + ENTRIES * ENTRY_WORDS == sdk::TRAIL_WORDS);

const STEP_MASK: u32 = 0xff;
/// Where a note's first word keeps the open critical sections, its
/// interrupt flag and its number (`WELLFED_TRAIL_*_SHIFT`).
pub const DEPTH_SHIFT: u32 = 8;
pub const ISR_SHIFT: u32 = 15;
pub const SEQ_SHIFT: u32 = 16;
const DEPTH_MASK: u32 = 0x7f;
const SEQ_MASK: u32 = 0xffff;

/// What a note records, as the shim's `WELLFED_STEP_*` codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    /// `save_and_cli` masked interrupts (noted once masked).
    SaveAndCli = 1,
    /// `restore_flags` is about to unmask them (noted while still masked).
    RestoreFlags = 2,
    /// `rtw_set_timer` is called; the argument is the timer.
    SetTimer = 3,
    /// `rtw_del_timer` is called; the argument is the timer.
    DelTimer = 4,
    /// `rtw_cancel_timer` is called; the argument is the timer.
    CancelTimer = 5,
    /// `wifi_indication` is called; the argument is the event.
    Indication = 6,
    /// `wifi_indication` returned.
    IndicationReturned = 7,
    /// `rtw_os_indicate_disconnect` is called; the argument is the adapter.
    IndicateDisconnect = 8,
    /// `rtw_os_indicate_disconnect` returned.
    IndicateDisconnectReturned = 9,
    /// The Wi-Fi task calls `wifi_connect`; the argument is the attempt.
    Connect = 10,
    /// `wifi_connect` returned; the argument is its result.
    ConnectReturned = 11,
}

impl Step {
    pub const ALL: [Self; 11] = [
        Self::SaveAndCli,
        Self::RestoreFlags,
        Self::SetTimer,
        Self::DelTimer,
        Self::CancelTimer,
        Self::Indication,
        Self::IndicationReturned,
        Self::IndicateDisconnect,
        Self::IndicateDisconnectReturned,
        Self::Connect,
        Self::ConnectReturned,
    ];

    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.code() == byte)
    }

    /// The name a line prints: the wrapped function, `_ret` once it returned.
    pub const fn name(self) -> &'static str {
        match self {
            Self::SaveAndCli => "save_and_cli",
            Self::RestoreFlags => "restore_flags",
            Self::SetTimer => "rtw_set_timer",
            Self::DelTimer => "rtw_del_timer",
            Self::CancelTimer => "rtw_cancel_timer",
            Self::Indication => "wifi_indication",
            Self::IndicationReturned => "wifi_indication_ret",
            Self::IndicateDisconnect => "rtw_os_indicate_disconnect",
            Self::IndicateDisconnectReturned => "rtw_os_indicate_disconnect_ret",
            Self::Connect => "wifi_connect",
            Self::ConnectReturned => "wifi_connect_ret",
        }
    }

    /// The sticky mark the step also goes to, if any.
    pub const fn mark(self) -> Option<Mark> {
        match self {
            Self::SaveAndCli | Self::RestoreFlags => None,
            Self::SetTimer | Self::DelTimer | Self::CancelTimer => Some(Mark::Timer),
            Self::Indication | Self::IndicationReturned => Some(Mark::Indication),
            Self::IndicateDisconnect | Self::IndicateDisconnectReturned => Some(Mark::Disconnect),
            Self::Connect => Some(Mark::Connect),
            Self::ConnectReturned => Some(Mark::ConnectReturned),
        }
    }
}
/// A sticky mark's slot, as the shim's `WELLFED_MARK_*` codes: the last
/// note of its kind, whatever the ring has turned over since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mark {
    /// The last `wifi_connect` call.
    Connect = 0,
    /// The last `wifi_connect` return.
    ConnectReturned = 1,
    /// The last `wifi_indication` call or return.
    Indication = 2,
    /// The last `rtw_os_indicate_disconnect` call or return.
    Disconnect = 3,
    /// The last `rtw_set_timer`, `rtw_del_timer` or `rtw_cancel_timer`.
    Timer = 4,
}

impl Mark {
    pub const ALL: [Self; MARKS] = [
        Self::Connect,
        Self::ConnectReturned,
        Self::Indication,
        Self::Disconnect,
        Self::Timer,
    ];

    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::ConnectReturned => "connect_ret",
            Self::Indication => "indication",
            Self::Disconnect => "disconnect",
            Self::Timer => "timer",
        }
    }
}

/// One note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The note's number since the trail started.
    pub seq: u32,
    pub step: Step,
    /// Wrapped critical sections open, this one included; 127 means that
    /// many or more.
    pub depth: u8,
    /// Noted from an interrupt handler.
    pub isr: bool,
    /// The wrapped call's return address (Thumb bit set), or 0 for the
    /// Wi-Fi task's own steps.
    pub lr: u32,
    pub arg: u32,
}

impl Entry {
    /// The note a slot's three words hold, if its number is `seq` and its
    /// step is known.
    fn read(seq: u32, meta: u32, lr: u32, arg: u32) -> Option<Self> {
        let step = u8::try_from(meta & STEP_MASK)
            .ok()
            .and_then(Step::from_byte)?;
        ((meta >> SEQ_SHIFT) & SEQ_MASK == seq & SEQ_MASK).then_some(Self {
            seq,
            step,
            depth: u8::try_from((meta >> DEPTH_SHIFT) & DEPTH_MASK).unwrap_or(u8::MAX),
            isr: (meta >> ISR_SHIFT) & 1 != 0,
            lr,
            arg,
        })
    }
}

impl fmt::Display for Entry {
    /// `seq=… step=… depth=… isr=… lr=0x… arg=0x…`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "seq={} step={} depth={} isr={} lr=0x{:08x} arg=0x{:08x}",
            self.seq,
            self.step.name(),
            self.depth,
            u8::from(self.isr),
            self.lr,
            self.arg
        )
    }
}

/// Why the RAM holds no trail of the last boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No boot started a trail of this layout: a power-on, a build without
    /// one, or a build with another layout.
    NotStarted,
    /// A trail another boot started: `boot` is its count, and this boot's
    /// was not one more. The boot in between had no trail, or never armed
    /// its flash hooks.
    OtherBoot { boot: u32, current: u32 },
}

/// The trail's words, read one at a time where they lie.
pub trait Words {
    /// Word `index`, or 0 past the end.
    fn word(&self, index: usize) -> u32;
}

impl Words for [u32; sdk::TRAIL_WORDS] {
    fn word(&self, index: usize) -> u32 {
        self.get(index).copied().unwrap_or(0)
    }
}

/// `wellfed_status_trail` itself, as the last boot left it.
pub struct Ram;

impl Words for Ram {
    fn word(&self, index: usize) -> u32 {
        sdk::trail_word(index)
    }
}

/// A boot's trail, as the next boot reads it from its words.
pub struct Trail<'a, W: Words + ?Sized> {
    words: &'a W,
    /// The KM4 boot count the trail started with.
    pub boot: u32,
    /// Notes since the trail started; the ring keeps the last [`ENTRIES`].
    pub count: u32,
    /// Wrapped critical sections open at the end, CPU-wide.
    pub depth: u32,
}

impl<'a, W: Words + ?Sized> Trail<'a, W> {
    /// The trail `words` hold, if it is the last boot's: this boot's KM4
    /// boot count is `current`.
    pub fn decode(words: &'a W, current: u32) -> Result<Self, Refusal> {
        let [magic, boot, check, count, depth] = [0, 1, 2, 3, 4].map(|i| words.word(i));
        if magic != sdk::TRAIL_MAGIC || check != !(magic ^ boot ^ sdk::TRAIL_VERSION) {
            return Err(Refusal::NotStarted);
        }
        if boot != current.wrapping_sub(1) {
            return Err(Refusal::OtherBoot { boot, current });
        }
        Ok(Self {
            words,
            boot,
            count,
            depth,
        })
    }

    /// The ring's window, oldest first: each slot's note, or `None` for a
    /// slot that held no whole note.
    fn window(&self) -> impl Iterator<Item = Option<Entry>> + '_ {
        let window = self.count.min(RING);
        (0..window).map(move |i| {
            let seq = self.count - window + i;
            let at = ENTRIES_AT + usize::try_from(seq % RING).unwrap_or(0) * ENTRY_WORDS;
            let word = |offset| self.words.word(at + offset);
            Entry::read(seq, word(0), word(1), word(2))
        })
    }

    /// The whole notes in the ring, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        self.window().flatten()
    }

    /// Slots in the window that held no whole note.
    pub fn skipped(&self) -> usize {
        self.window().filter(Option::is_none).count()
    }

    /// The newest whole note in the ring.
    pub fn last(&self) -> Option<Entry> {
        self.entries().last()
    }

    /// The last note of `mark`'s kind, if the boot made one.
    pub fn mark(&self, mark: Mark) -> Option<Entry> {
        let at = MARKS_AT + usize::from(mark.code()) * MARK_WORDS;
        let word = |offset| self.words.word(at + offset);
        let seq = word(0);
        // A torn mark fails its number or its cleared step.
        Entry::read(seq, word(1), word(2), word(3)).filter(|entry| entry.step.mark() == Some(mark))
    }

    /// Every mark with its last note, in [`Mark::ALL`]'s order.
    pub fn marks(&self) -> impl Iterator<Item = (Mark, Option<Entry>)> + '_ {
        Mark::ALL.into_iter().map(|mark| (mark, self.mark(mark)))
    }
}

impl<W: Words + ?Sized> fmt::Display for Trail<'_, W> {
    /// `boot=… count=… n=… skipped=… depth=… last=…`, the last as its step
    /// name or `none`; the marks and entries print on lines of their own
    /// (`mark=… seq=…`, `seq=…`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "boot={} count={} n={} skipped={} depth={} last=",
            self.boot,
            self.count,
            self.entries().count(),
            self.skipped(),
            self.depth
        )?;
        match self.last() {
            Some(entry) => f.write_str(entry.step.name()),
            None => f.write_str("none"),
        }
    }
}

/// Hand the last boot's trail, read in place, to `report`, then start an
/// empty one for this boot, which notes from here on. Call once at
/// startup, before the scheduler starts and before the flash hooks arm
/// (which counts this boot).
pub fn take(report: impl FnOnce(Result<Trail<'_, Ram>, Refusal>)) {
    let current = sdk::backup_read(BOOT_COUNT_REGISTER);
    report(Trail::decode(&Ram, current));
    sdk::trail_start(current);
}
