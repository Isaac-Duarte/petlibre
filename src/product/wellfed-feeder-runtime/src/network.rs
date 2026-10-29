//! Wi-Fi credentials without a rebuild.
//!
//! - **Request half:** credentials from the device API ([`change`]) or
//!   from BLE setup ([`save`]), checked by the store's own rules, are
//!   queued for the settings writer, which saves them to NOR (storage
//!   sectors 5 and 6).
//! - **Join half:** before each join attempt the C shim asks for credentials
//!   (`wellfed_wifi_select`) and then reports the outcome
//!   (`wellfed_wifi_outcome`). Stored credentials are used until
//!   [`FALLBACK_AFTER`] attempts in a row fail without one success this boot;
//!   then the build's credentials take over, so a bad save cannot strand the
//!   feeder. A save during the boot (a device API write, or BLE setup)
//!   takes effect at the next join: the new credentials are read again and
//!   get their own run of attempts before the build's take over.
//! - **No build network:** a consumer image has no build credentials
//!   ([`BUILD_WIFI`]). Stored credentials are then used however often they
//!   fail, and with none stored there is nothing to join: the select hands
//!   the C task empty credentials, which it reports as a failed attempt
//!   without scanning.
//! - **Turn:** the select takes the join's turn in the [`interlock`], waiting
//!   while a feed holds the outputs, and the outcome ends it, so no feed
//!   starts while the attempt scans, associates or waits for DHCP.
//!
//! [`interlock`]: wellfed_hal::interlock
//!
//! Nothing here writes flash.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use wellfed_core::store::slot::SlotIo;
use wellfed_device_api::mailbox::{WifiResult, WifiSource, WifiView};
use wellfed_device_api::wifi::Credentials;
use wellfed_feeder::store::net::NetStore;
use wellfed_feeder::store::reset;

use wellfed_hal::cell::TaskCell;
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::{link, sdk};

/// The credential slots within the storage region (`0x345000`).
pub const NET_BASE: u32 = wellfed_hal::plaf103::storage::NET.first;
/// The factory reset's marker sector.
const RESET_BASE: u32 = wellfed_hal::plaf103::storage::RESET.first;
/// Consecutive failed joins before the build's credentials take over.
pub const FALLBACK_AFTER: u32 = 5;

/// Whether the image was built with Wi-Fi credentials of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildWifi {
    /// A bench image: the C entry prefills the bench hotspot.
    Present,
    /// A consumer image: the only network is the one the owner stores.
    Absent,
}

/// This image's build Wi-Fi. `wellfed-build` sets `WELLFED_BUILD_WIFI`
/// exactly when the generated header the C entry prefills from names a
/// network, so the two always agree; only its presence counts, never a value.
pub const BUILD_WIFI: BuildWifi = match option_env!("WELLFED_BUILD_WIFI") {
    Some(_) => BuildWifi::Present,
    None => BuildWifi::Absent,
};

/// Which credentials to use for the next join; `None` when there is
/// nothing to join (no build network and none stored).
pub fn choose(
    build: BuildWifi,
    stored: bool,
    failures: u32,
    joined_once: bool,
) -> Option<WifiSource> {
    match (build, stored, joined_once) {
        (BuildWifi::Absent, false, _) => None,
        (BuildWifi::Absent, true, _) => Some(WifiSource::Stored),
        (BuildWifi::Present, false, _) => Some(WifiSource::Build),
        (BuildWifi::Present, true, true) => Some(WifiSource::Stored),
        (BuildWifi::Present, true, false) if failures >= FALLBACK_AFTER => {
            Some(WifiSource::Fallback)
        }
        (BuildWifi::Present, true, false) => Some(WifiSource::Stored),
    }
}

/// Who holds `PENDING`, as `STATE` stores it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Hand {
    /// Free for the device API to claim.
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
pub fn publish<I: SlotIo>(store: &NetStore<I>, armed: bool) {
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
///
/// # Safety
/// No Wi-Fi task is running: the join state is the Wi-Fi task's own.
pub unsafe fn reset() {
    for flag in [&LOADED, &WRITABLE, &STORED, &RELOAD] {
        flag.store(false, Ordering::Release);
    }
    GENERATION.store(0, Ordering::Release);
    SOURCE.store(WifiSource::Build.code(), Ordering::Release);
    STATE.store(Hand::Empty as u8, Ordering::Release);
    // SAFETY: the caller's contract: no Wi-Fi task holds `JOIN`.
    unsafe { *JOIN.get() = Join::default() };
    #[cfg(not(target_os = "none"))]
    INTERLOCK.reset();
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

/// The credential sectors through the flash map: reads need no lease, and
/// the join half never writes.
struct MappedNet;

impl SlotIo for MappedNet {
    fn unit_bytes(&self) -> u32 {
        sdk::SECTOR_BYTES
    }

    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        slot <= 1 && sdk::storage_read(NET_BASE + slot, offset, bytes)
    }

    fn program(&self, _slot: u32, _offset: u32, _bytes: &[u8]) -> bool {
        false
    }

    fn erase(&self, _slot: u32) -> bool {
        false
    }

    fn writable(&self) -> bool {
        false
    }
}

/// The stored credentials, read once through the flash map (no lease).
/// None while a factory reset is unfinished: the Wi-Fi task may ask before
/// the settings writer has erased the credentials the reset clears.
fn stored(join: &mut Join) -> Option<Credentials> {
    if !join.opened {
        join.opened = true;
        let mut marker = [0; reset::MARKER.len()];
        let resetting = sdk::storage_read(RESET_BASE, 0, &mut marker) && reset::marked(&marker);
        let mut store = NetStore::default();
        if !resetting {
            store.open(MappedNet);
        }
        join.stored = store.credentials;
    }
    join.stored
}

/// The C view of one join's credentials; prefilled with the build's, or
/// empty without a build network.
#[repr(C)]
pub struct JoinCredentials {
    pub ssid: *const u8,
    pub ssid_len: u32,
    pub psk: *const u8,
    pub psk_len: u32,
    /// `WifiSource::code`: 0 build, 1 stored, 2 fallback.
    pub source: u32,
}

/// Before each join attempt: take the join's turn (waiting while a feed
/// holds the outputs), then replace the build's credentials with stored
/// ones unless the fallback has taken over.
///
/// # Safety
/// `out` is valid; called only from the Wi-Fi task, which may block here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_wifi_select(out: *mut JoinCredentials) {
    // SAFETY: the caller's contract.
    unsafe { select(BUILD_WIFI, out) }
}

/// [`wellfed_wifi_select`] for an image with or without build credentials.
/// With nothing to join, `out` is emptied (a zero SSID length), which the C
/// task reports as a failed attempt without scanning.
///
/// # Safety
/// As [`wellfed_wifi_select`]'s.
pub unsafe fn select(build: BuildWifi, out: *mut JoinCredentials) {
    INTERLOCK.begin_join(sdk::uptime_ms, sdk::delay_ms);

    // SAFETY: only the Wi-Fi task calls this.
    let join = unsafe { &mut *JOIN.get() };
    if RELOAD.swap(false, Ordering::AcqRel) {
        *join = Join::default();
    }
    let stored = stored(join);
    let choice = choose(build, stored.is_some(), join.failures, join.joined_once);
    // Nothing to join reads as the build's source, as before any join: the
    // device API's view has no value for it.
    let code = choice.unwrap_or(WifiSource::Build).code();
    SOURCE.store(code, Ordering::Release);
    // SAFETY: the caller's contract; JOIN is static, so the pointers live on.
    let out = unsafe { &mut *out };
    out.source = u32::from(code);
    let Some(choice) = choice else {
        out.ssid = core::ptr::null();
        out.ssid_len = 0;
        out.psk = core::ptr::null();
        out.psk_len = 0;
        return;
    };
    if let (WifiSource::Stored, Some(c)) = (choice, join.stored.as_ref()) {
        out.ssid = c.ssid().as_ptr();
        out.ssid_len = u32::try_from(c.ssid().len()).unwrap_or(0);
        out.psk = c.psk().as_ptr();
        out.psk_len = u32::try_from(c.psk().len()).unwrap_or(0);
    }
}

/// After each join attempt: 1 when an address was assigned, which also
/// starts the link's watch (`wellfed_wifi_watch`).
///
/// # Safety
/// Called only from the Wi-Fi task, which owns `JOIN`, and never while a
/// `wellfed_wifi_select` call is running.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_wifi_outcome(joined: i32) {
    INTERLOCK.end_join();
    if joined != 0 {
        // SAFETY: the caller's contract: the Wi-Fi task, between watches.
        unsafe { link::joined() };
    }

    // SAFETY: the caller's contract makes this the only reference to `JOIN`.
    let join = unsafe { &mut *JOIN.get() };
    if source() != WifiSource::Stored {
        return;
    }
    if joined != 0 {
        join.joined_once = true;
        join.failures = 0;
    } else if !link::radio_off() {
        // An attempt battery mode cut short says nothing about the
        // credentials, so it never counts toward the fallback.
        join.failures = join.failures.saturating_add(1);
    }
}
