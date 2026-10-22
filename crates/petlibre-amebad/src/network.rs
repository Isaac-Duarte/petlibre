//! FW25: Wi-Fi credentials without a rebuild.
//!
//! - **Transport half:** a signed `wifi` request is checked by the store's own
//!   rules and queued for the settings writer, which saves it to NOR (storage
//!   sectors 5 and 6). It takes effect at the next boot.
//! - **Join half:** before each join attempt the C shim asks for credentials
//!   (`petlibre_wifi_select`) and then reports the outcome
//!   (`petlibre_wifi_outcome`). Stored credentials are used until
//!   [`FALLBACK_AFTER`] attempts in a row fail without one success this boot;
//!   then the build's credentials take over for the rest of the boot, so a bad
//!   save cannot strand the feeder.
//!
//! Nothing here writes flash; the transport may call [`request`].

use core::cell::UnsafeCell;
use core::ffi::{c_uint, c_void};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use petlibre_core::net_store::{Credentials, NetStore};
use petlibre_core::storage::JournalIo;
use petlibre_core::transport::{WifiFields, WifiResult, WifiSource, WifiView};

use crate::sdk;

/// The credential slots within the storage region (`0x345000`).
pub const NET_BASE: u32 = 5;
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

const EMPTY: u8 = 0;
const FILLING: u8 = 1;
const QUEUED: u8 = 2;

struct Shared<T>(UnsafeCell<T>);
// SAFETY: each value is guarded as documented where it is declared.
unsafe impl<T> Sync for Shared<T> {}

/// The queued change; written only by the claimant of `STATE` (EMPTY to
/// FILLING), read only by the writer after QUEUED.
static PENDING: Shared<Option<Credentials>> = Shared(UnsafeCell::new(None));
static STATE: AtomicU8 = AtomicU8::new(EMPTY);

/// What NOR holds, published by the writer.
static LOADED: AtomicBool = AtomicBool::new(false);
static WRITABLE: AtomicBool = AtomicBool::new(false);
static STORED: AtomicBool = AtomicBool::new(false);
static STORED_CRC: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// `WifiSource` of this boot's joins, as a code.
static SOURCE: AtomicU8 = AtomicU8::new(0);

fn decode_hex(hex: &str, out: &mut [u8]) -> Option<usize> {
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) || bytes.len() / 2 > out.len() {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(bytes.len() / 2)
}

pub fn source() -> WifiSource {
    match SOURCE.load(Ordering::Acquire) {
        1 => WifiSource::Stored,
        2 => WifiSource::Fallback,
        _ => WifiSource::Build,
    }
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

/// The transport's half: read, or queue a store or clear for the writer.
pub fn request(fields: WifiFields) -> WifiResult {
    if !LOADED.load(Ordering::Acquire) {
        return WifiResult::Busy;
    }
    if !fields.changes() {
        let pending = STATE.load(Ordering::Acquire) != EMPTY;
        return WifiResult::Current {
            view: view(),
            pending,
        };
    }
    if !WRITABLE.load(Ordering::Acquire) {
        return WifiResult::ReadOnly;
    }

    let credentials = match (fields.ssid, fields.psk) {
        (Some(ssid), Some(psk)) => {
            let (mut s, mut p) = ([0u8; 32], [0u8; 63]);
            let (Some(sn), Some(pn)) = (decode_hex(ssid, &mut s), decode_hex(psk, &mut p)) else {
                return WifiResult::Invalid;
            };
            match Credentials::new(&s[..sn], &p[..pn]) {
                Some(c) => Some(c),
                None => return WifiResult::Invalid,
            }
        }
        _ => None,
    };

    if STATE
        .compare_exchange(EMPTY, FILLING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return WifiResult::Busy;
    }
    // SAFETY: FILLING makes this the only accessor until QUEUED.
    unsafe { *PENDING.0.get() = credentials };
    STATE.store(QUEUED, Ordering::Release);
    WifiResult::Current {
        view: view(),
        pending: true,
    }
}

/// The writer's half: the queued change (`Some(None)` clears), if any.
pub fn pending() -> Option<Option<Credentials>> {
    // SAFETY: QUEUED hands the value to the writer until `finish`.
    (STATE.load(Ordering::Acquire) == QUEUED).then(|| unsafe { *PENDING.0.get() })
}

pub fn finish() {
    STATE.store(EMPTY, Ordering::Release);
}

/// The writer publishes what NOR holds; writable only once it is `armed`.
pub fn publish(store: &NetStore, armed: bool) {
    STORED_CRC.store(
        store
            .credentials
            .map_or(0, |c| petlibre_core::storage::crc32(c.ssid())),
        Ordering::Release,
    );
    STORED.store(store.credentials.is_some(), Ordering::Release);
    GENERATION.store(store.generation as u32, Ordering::Release);
    WRITABLE.store(armed && store.writable(), Ordering::Release);
    LOADED.store(true, Ordering::Release);
}

/// Forget everything (host tests).
pub fn reset() {
    for flag in [&LOADED, &WRITABLE, &STORED] {
        flag.store(false, Ordering::Release);
    }
    GENERATION.store(0, Ordering::Release);
    SOURCE.store(0, Ordering::Release);
    STATE.store(EMPTY, Ordering::Release);
    // SAFETY: tests reset with no other accessor.
    unsafe { *JOIN.0.get() = Join::default() };
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
static JOIN: Shared<Join> = Shared(UnsafeCell::new(Join {
    opened: false,
    stored: None,
    failures: 0,
    joined_once: false,
}));

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
    /// `WifiSource` as 0 build, 1 stored, 2 fallback.
    pub source: u32,
}

/// Before each join attempt: replace the build's credentials with stored
/// ones unless the fallback has taken over.
///
/// # Safety
/// `out` is valid; called only from the Wi-Fi task.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_wifi_select(out: *mut JoinCredentials) {
    // SAFETY: only the Wi-Fi task calls this.
    let join = unsafe { &mut *JOIN.0.get() };
    let stored = stored(join);
    let choice = choose(stored.is_some(), join.failures, join.joined_once);
    let code = match choice {
        WifiSource::Build => 0,
        WifiSource::Stored => 1,
        WifiSource::Fallback => 2,
    };
    SOURCE.store(code, Ordering::Release);
    // SAFETY: the caller's contract; JOIN is static, so the pointers live on.
    let out = unsafe { &mut *out };
    out.source = u32::from(code);
    if let (WifiSource::Stored, Some(c)) = (choice, join.stored.as_ref()) {
        out.ssid = c.ssid().as_ptr();
        out.ssid_len = c.ssid().len() as u32;
        out.psk = c.psk().as_ptr();
        out.psk_len = c.psk().len() as u32;
    }
}

/// After each join attempt: 1 when an address was assigned.
#[unsafe(no_mangle)]
pub extern "C" fn petlibre_wifi_outcome(joined: i32) {
    // SAFETY: only the Wi-Fi task calls this.
    let join = unsafe { &mut *JOIN.0.get() };
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
