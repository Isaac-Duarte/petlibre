//! FW25: Wi-Fi credentials without a rebuild.
//!
//! - **Request half:** credentials from the device API (FW47, [`change`]) or
//!   from BLE setup (FW45, [`save`]), checked by the store's own rules, are
//!   queued for the settings writer, which saves them to NOR (storage
//!   sectors 5 and 6).
//! - **Join half:** before each join attempt the C shim asks for credentials
//!   (`wellfed_wifi_select`) and then reports the outcome
//!   (`wellfed_wifi_outcome`). Stored credentials are used until
//!   [`FALLBACK_AFTER`] attempts in a row fail without one success this boot;
//!   then the build's credentials take over, so a bad save cannot strand the
//!   feeder. A save during the boot (a device API write, or FW45 BLE setup)
//!   takes effect at the next join: the new credentials are read again and
//!   get their own run of attempts before the build's take over.
//!
//! Nothing here writes flash.

use core::ffi::{c_uint, c_void};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::store::net::{Credentials, NetStore};
use wellfed_core::store::slot::JournalIo;
use wellfed_core::transport::{WifiResult, WifiSource, WifiView};

use crate::cell::TaskCell;
use crate::sdk;

/// The credential slots within the storage region (`0x345000`).
pub const NET_BASE: u32 = sdk::storage::NET.first;
/// Consecutive failed joins before the build's credentials take over.
pub const FALLBACK_AFTER: u32 = 5;

/// Which credentials to use for the next join.
pub fn choose(stored: bool, failures: u32, joined_once: bool) -> WifiSource {
    match (stored, joined_once) {
        (false, _) => WifiSource::Build,
        (true, true) => WifiSource::Stored,
        (true, false) if failures >= FALLBACK_AFTER => WifiSource::Fallback,
        (true, false) => WifiSource::Stored,
    }
}

/// Who holds `PENDING`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    /// Free for the transport to claim.
    Empty = 0,
    /// The claimant is writing the change.
    Filling = 1,
    /// Handed to the writer until `finish`.
    Queued = 2,
}

/// The queued change; written only by the claimant of `STATE` (`Empty` to
/// `Filling`), read only by the writer after `Queued`.
static PENDING: TaskCell<Option<Credentials>> = TaskCell::new(None);
static STATE: AtomicU8 = AtomicU8::new(Hand::Empty as u8);

/// What NOR holds, published by the writer.
static LOADED: AtomicBool = AtomicBool::new(false);
static WRITABLE: AtomicBool = AtomicBool::new(false);
static STORED: AtomicBool = AtomicBool::new(false);
static STORED_CRC: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Set by a save after the first load; the join half then reads again.
static RELOAD: AtomicBool = AtomicBool::new(false);
/// `WifiSource` of this boot's joins, as `WifiSource::code`.
static SOURCE: AtomicU8 = AtomicU8::new(WifiSource::Build.code());

pub fn source() -> WifiSource {
    WifiSource::from_code(SOURCE.load(Ordering::Acquire))
}

fn view() -> WifiView {
    WifiView {
        stored_ssid_crc: STORED
            .load(Ordering::Acquire)
            .then(|| STORED_CRC.load(Ordering::Acquire)),
        generation: GENERATION.load(Ordering::Acquire),
        writable: WRITABLE.load(Ordering::Acquire),
        source: source(),
    }
}

/// The store as the writer last published it, and whether a change waits.
pub fn current() -> WifiResult {
    if !LOADED.load(Ordering::Acquire) {
        return WifiResult::Busy;
    }
    WifiResult::Current {
        view: view(),
        pending: STATE.load(Ordering::Acquire) != Hand::Empty as u8,
    }
}

/// BLE setup's half: queue checked credentials for the writer.
pub fn save(credentials: Credentials) -> WifiResult {
    change(Some(credentials))
}

/// The device API's half: queue checked credentials, or a clear with
/// `None`, for the writer.
pub fn change(credentials: Option<Credentials>) -> WifiResult {
    if !LOADED.load(Ordering::Acquire) {
        return WifiResult::Busy;
    }
    if !WRITABLE.load(Ordering::Acquire) {
        return WifiResult::ReadOnly;
    }
    queue(credentials)
}

/// Whether a queued change still waits for the writer.
pub fn saving() -> bool {
    STATE.load(Ordering::Acquire) != Hand::Empty as u8
}

fn queue(credentials: Option<Credentials>) -> WifiResult {
    if STATE
        .compare_exchange(
            Hand::Empty as u8,
            Hand::Filling as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return WifiResult::Busy;
    }
    // SAFETY: `Filling` makes this the only accessor until `Queued`.
    unsafe { *PENDING.get() = credentials };
    STATE.store(Hand::Queued as u8, Ordering::Release);
    WifiResult::Current {
        view: view(),
        pending: true,
    }
}

/// The writer's half: the queued change (`Some(None)` clears), if any.
pub fn pending() -> Option<Option<Credentials>> {
    // SAFETY: `Queued` hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == Hand::Queued as u8).then(|| unsafe { *PENDING.get() })
}

pub fn finish() {
    STATE.store(Hand::Empty as u8, Ordering::Release);
}

/// The writer publishes what NOR holds; writable only once it is `armed`.
pub fn publish(store: &NetStore, armed: bool) {
    STORED_CRC.store(
        store
            .credentials
            .map_or(0, |c| wellfed_core::store::slot::crc32(c.ssid())),
        Ordering::Release,
    );
    STORED.store(store.credentials.is_some(), Ordering::Release);
    // Health reports the low 32 bits; saturate rather than wrap to an old value.
    let generation = u32::try_from(store.generation).unwrap_or(u32::MAX);
    let previous = GENERATION.swap(generation, Ordering::AcqRel);
    WRITABLE.store(armed && store.writable(), Ordering::Release);
    let loaded = LOADED.swap(true, Ordering::AcqRel);

    // A save after the first load: the next join reads the new credentials
    // and gives them a fresh run of attempts before any fallback.
    if loaded && generation != previous {
        RELOAD.store(true, Ordering::Release);
    }
}

/// Forget everything (host tests).
pub fn reset() {
    for flag in [&LOADED, &WRITABLE, &STORED, &RELOAD] {
        flag.store(false, Ordering::Release);
    }
    GENERATION.store(0, Ordering::Release);
    SOURCE.store(WifiSource::Build.code(), Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
    // SAFETY: tests reset with no other accessor.
    unsafe { *JOIN.get() = Join::default() };
}

// ---- The join half, called by the C Wi-Fi task only.

#[derive(Default)]
struct Join {
    opened: bool,
    stored: Option<Credentials>,
    failures: u32,
    joined_once: bool,
}

/// Touched only by the C Wi-Fi task, through the two exports below.
static JOIN: TaskCell<Join> = TaskCell::new(Join {
    opened: false,
    stored: None,
    failures: 0,
    joined_once: false,
});

unsafe extern "C" fn read_only(
    _context: *mut c_void,
    slot: c_uint,
    offset: u32,
    bytes: *mut c_void,
    size: usize,
) -> bool {
    if slot > 1 {
        return false;
    }
    // SAFETY: the store passes a live buffer of `size` bytes.
    let bytes = unsafe { core::slice::from_raw_parts_mut(bytes.cast::<u8>(), size) };
    sdk::storage_read(NET_BASE + slot, offset, bytes)
}

/// The stored credentials, read once through the flash map (no lease).
fn stored(join: &mut Join) -> Option<Credentials> {
    if !join.opened {
        join.opened = true;
        let io = JournalIo {
            context: core::ptr::null_mut(),
            unit_bytes: sdk::SECTOR_BYTES,
            read: Some(read_only),
            program: None,
            erase: None,
        };
        let mut store = NetStore::default();
        // SAFETY: `read_only` is a static function over mapped reads.
        unsafe { store.open(io) };
        join.stored = store.credentials;
    }
    join.stored
}

/// The C view of one join's credentials; prefilled with the build's.
#[repr(C)]
pub struct JoinCredentials {
    pub ssid: *const u8,
    pub ssid_len: u32,
    pub psk: *const u8,
    pub psk_len: u32,
    /// `WifiSource::code`: 0 build, 1 stored, 2 fallback.
    pub source: u32,
}

/// Before each join attempt: replace the build's credentials with stored
/// ones unless the fallback has taken over.
///
/// # Safety
/// `out` is valid; called only from the Wi-Fi task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_wifi_select(out: *mut JoinCredentials) {
    // SAFETY: only the Wi-Fi task calls this.
    let join = unsafe { &mut *JOIN.get() };
    if RELOAD.swap(false, Ordering::AcqRel) {
        *join = Join::default();
    }
    let stored = stored(join);
    let choice = choose(stored.is_some(), join.failures, join.joined_once);
    let code = choice.code();
    SOURCE.store(code, Ordering::Release);
    // SAFETY: the caller's contract; JOIN is static, so the pointers live on.
    let out = unsafe { &mut *out };
    out.source = u32::from(code);
    if let (WifiSource::Stored, Some(c)) = (choice, join.stored.as_ref()) {
        out.ssid = c.ssid().as_ptr();
        out.ssid_len = u32::try_from(c.ssid().len()).unwrap_or(0);
        out.psk = c.psk().as_ptr();
        out.psk_len = u32::try_from(c.psk().len()).unwrap_or(0);
    }
}

/// After each join attempt: 1 when an address was assigned.
#[unsafe(no_mangle)]
pub extern "C" fn wellfed_wifi_outcome(joined: i32) {
    // SAFETY: only the Wi-Fi task calls this.
    let join = unsafe { &mut *JOIN.get() };
    if source() != WifiSource::Stored {
        return;
    }
    if joined != 0 {
        join.joined_once = true;
        join.failures = 0;
    } else {
        join.failures = join.failures.saturating_add(1);
    }
}
