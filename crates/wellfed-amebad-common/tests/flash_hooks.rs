//! Synthetic-provider tests of the SDK flash interposer, using the real authority.

use core::ffi::c_void;
use core::ptr;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use wellfed_amebad_common::flash::hooks::*;
use wellfed_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashPhase, FlashReply,
};

#[derive(Default)]
struct Model {
    irq_mask: u32,
    ipsr: u32,
    systick: u32,
    /// SysTick COUNTFLAG: set by a counter wrap, cleared only by reading CTRL.
    countflag: bool,
    legacy_tick: u32,
    legacy_locks: u32,
    legacy_unlocks: u32,
    legacy_irqs: u32,
    ready_calls: u32,
    acquire_calls: u32,
    release_calls: u32,
    peer_calls: u32,
    fault_calls: u32,
    bad_acquire: u32,
    bad_release: u32,
    ready: bool,
    bad_peer: bool,
    bad_fault: bool,
    preempt_pending: bool,
    recursive_fault: bool,
    now: u32,
    next_id: u64,
    last_fault: HookFault,
}

#[derive(Default)]
struct State {
    transport: HookProvider,
    ticket: HookTicket,
    last_id: u64,
    armed: bool,
    faulted: bool,
    legacy_locked: bool,
    lease_held: bool,
}

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
    static STATE: RefCell<Box<State>> = RefCell::new(Box::default());
    static AUTHORITY: RefCell<Box<FlashCoord>> = RefCell::new(Box::default());
}

fn m<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    MODEL.with(|m| f(&mut m.borrow_mut()))
}

fn authority<R>(f: impl FnOnce(&mut FlashCoord) -> R) -> R {
    AUTHORITY.with(|a| f(&mut a.borrow_mut()))
}

fn authority_ptr() -> *mut c_void {
    AUTHORITY.with(|a| ptr::from_mut(&mut **a.borrow_mut()).cast())
}

struct Halted;

struct Fake;

impl HookCpu for Fake {
    fn cells() -> HookCells {
        STATE.with(|s| {
            let s = ptr::from_mut(&mut **s.borrow_mut());
            // SAFETY: the boxed state lives for the thread.
            unsafe {
                HookCells {
                    transport: &raw mut (*s).transport,
                    ticket: &raw mut (*s).ticket,
                    last_id: &raw mut (*s).last_id,
                    armed: &raw mut (*s).armed,
                    faulted: &raw mut (*s).faulted,
                    legacy_locked: &raw mut (*s).legacy_locked,
                    lease_held: &raw mut (*s).lease_held,
                }
            }
        })
    }
    fn mask() -> u32 {
        m(|m| m.irq_mask)
    }
    fn ipsr() -> u32 {
        m(|m| m.ipsr)
    }
    fn disable() {
        m(|m| m.irq_mask = 1);
    }
    fn restore(value: u32) {
        m(|m| m.irq_mask = value);
    }
    fn tick_read() -> u32 {
        m(|m| {
            m.systick
                | if core::mem::take(&mut m.countflag) {
                    1 << 16
                } else {
                    0
                }
        })
    }
    fn tick_write(value: u32) {
        m(|m| m.systick = value);
    }
    fn fail(reason: HookFault) -> ! {
        fail_body::<Fake>(reason)
    }
    fn spin() {
        std::panic::panic_any(Halted)
    }
    fn legacy_lock() {
        m(|m| {
            m.legacy_locks += 1;
            m.legacy_tick = m.systick;
            m.irq_mask = 1;
            m.systick = 0;
        });
    }
    fn legacy_unlock() {
        m(|m| {
            m.legacy_unlocks += 1;
            m.systick = m.legacy_tick;
            m.irq_mask = 0;
        });
    }
    fn legacy_ipc(data: *mut c_void, status: u32, channel: u32) {
        assert!(data == authority_ptr() && status == 4 && channel == 2);
        m(|m| m.legacy_irqs += 1);
    }
}

unsafe extern "C-unwind" fn provider_ready(context: *mut c_void) -> bool {
    assert!(context == authority_ptr());
    m(|m| {
        assert!(m.irq_mask == 0 && m.ipsr == 0);
        m.ready_calls += 1;
        m.ready
    })
}

/// Synthetic transport provider using the real authority, not physical IPC.
unsafe extern "C-unwind" fn provider_acquire(
    context: *mut c_void,
    ticket: *mut HookTicket,
) -> bool {
    assert!(context == authority_ptr());
    let preempt = m(|m| {
        assert!(m.irq_mask == 0 && m.ipsr == 0);
        m.acquire_calls += 1;
        core::mem::take(&mut m.preempt_pending)
    });
    if preempt {
        write_lock::<Fake>();
        assert!(m(|m| m.irq_mask == 1 && m.systick == 0));
        write_unlock::<Fake>();
        assert!(m(|m| m.irq_mask == 0 && m.systick == 7));
    }
    let ticket = unsafe { &mut *ticket };
    let (id, now) = m(|m| {
        m.irq_mask = 1;
        ticket.saved_systick = m.systick;
        m.systick = 0;
        m.next_id += 1;
        m.now += 3;
        (m.next_id, m.now - 3)
    });
    let lease = authority(|a| {
        assert_eq!(
            a.request(FlashParty::LOCAL, 17, id, now),
            FlashReply::ACCEPTED
        );
        let t = a.token;
        assert_eq!(a.quiesced(&t, true, now + 1), FlashReply::ACCEPTED);
        assert_eq!(
            a.parked(&t, FlashParty::PEER, now + 2),
            FlashReply::ACCEPTED
        );
        t.lease
    });
    ticket.id = lease;
    m(|m| match m.bad_acquire {
        1 => false,
        2 => {
            m.irq_mask = 0;
            true
        }
        3 => {
            m.systick = 7;
            true
        }
        4 => {
            ticket.id = 0;
            true
        }
        5 => {
            ticket.saved_systick |= 0x10000;
            true
        }
        6 => {
            m.ipsr = 19;
            true
        }
        7 => {
            ticket.id = 1;
            true
        }
        _ => true,
    })
}

unsafe extern "C-unwind" fn provider_release(
    context: *mut c_void,
    ticket: *const HookTicket,
) -> bool {
    assert!(context == authority_ptr());
    assert!(m(|m| m.irq_mask == 1 && m.systick == 0 && m.ipsr == 0));
    assert_eq!(unsafe { (*ticket).id }, authority(|a| a.token.lease));
    let (bad, now) = m(|m| {
        m.release_calls += 1;
        m.now += 2;
        (m.bad_release, m.now - 2)
    });
    if bad == 1 {
        return false;
    }
    authority(|a| {
        let t = a.token;
        assert_eq!(a.complete(&t, FlashParty::LOCAL, now), FlashReply::ACCEPTED);
        assert_eq!(
            a.released(&t, FlashParty::PEER, true, now + 1),
            FlashReply::ACCEPTED
        );
    });
    m(|m| match bad {
        2 => m.irq_mask = 0,
        3 => m.systick = 7,
        4 => m.ipsr = 19,
        _ => {}
    });
    true
}

unsafe extern "C-unwind" fn provider_peer(context: *mut c_void, status: u32, channel: u32) -> bool {
    assert!(context == authority_ptr() && status == 4 && channel == 2);
    m(|m| {
        m.peer_calls += 1;
        !m.bad_peer
    })
}

unsafe extern "C-unwind" fn provider_fault(context: *mut c_void, reason: HookFault) {
    assert!(context == authority_ptr());
    let recursive = m(|m| {
        assert!(m.irq_mask == 1 && m.systick == 0);
        m.fault_calls += 1;
        m.last_fault = reason;
        m.recursive_fault
    });
    authority(|a| a.trip(FlashFault::TRANSPORT));
    if recursive {
        write_lock::<Fake>();
        panic!("recursive fault must halt once");
    }
    m(|m| {
        if m.bad_fault {
            m.irq_mask = 0;
            m.systick = 7;
        }
    });
}

fn provider() -> HookProvider {
    HookProvider {
        context: authority_ptr(),
        ready: Some(provider_ready),
        acquire: Some(provider_acquire),
        release: Some(provider_release),
        peer: Some(provider_peer),
        fault: Some(provider_fault),
    }
}

fn reset() {
    STATE.with(|s| **s.borrow_mut() = State::default());
    m(|m| {
        *m = Model {
            systick: 7,
            legacy_tick: 7,
            ready: true,
            ..Model::default()
        }
    });
    let limits = FlashLimits {
        quiesce_ms: 100,
        park_ms: 100,
        active_ms: 100,
        release_ms: 100,
        total_ms: 300,
    };
    authority(|a| {
        *a = FlashCoord::new(&limits, 17, 0).unwrap();
        assert_eq!(a.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    });
}

fn install_provider(p: &HookProvider) -> bool {
    install::<Fake>(p)
}

/// Run `f`, which must halt in RAM rather than return.
fn halts(f: impl FnOnce()) {
    let outcome = catch_unwind(AssertUnwindSafe(f));
    assert!(
        outcome.err().is_some_and(|e| e.is::<Halted>()),
        "must halt for independent reset"
    );
}

#[test]
fn legacy_until_negotiated_provider() {
    reset();
    assert!(!armed::<Fake>());
    write_lock::<Fake>();
    assert!(m(|m| m.legacy_locks == 1
        && m.irq_mask == 1
        && m.systick == 0));
    m(|m| m.irq_mask = 0); // Exercise in-flight exclusion independently of IRQ refusal.
    assert!(!install_provider(&provider()) && m(|m| m.ready_calls) == 0);
    write_unlock::<Fake>();
    assert!(m(|m| m.legacy_unlocks == 1
        && m.irq_mask == 0
        && m.systick == 7));
    write_ipc::<Fake>(authority_ptr(), 4, 2);
    assert!(m(|m| m.legacy_irqs == 1
        && m.acquire_calls == 0
        && m.peer_calls == 0));
    m(|m| m.ready = false);
    assert!(!install_provider(&provider()));
    m(|m| m.ready = true);
    assert!(install_provider(&provider()));
    let calls = m(|m| m.ready_calls);
    assert!(!install_provider(&provider()) && m(|m| m.ready_calls) == calls);
}

#[test]
fn installation_requires_complete_provider_and_task_context() {
    for missing in 0..8 {
        reset();
        let mut p = provider();
        match missing {
            0 => p.ready = None,
            1 => p.acquire = None,
            2 => p.release = None,
            3 => p.peer = None,
            4 => p.fault = None,
            5 => m(|m| m.ipsr = 18),
            6 => m(|m| m.irq_mask = 1),
            _ => {}
        }
        let target = if missing == 7 {
            ptr::null()
        } else {
            &raw const p
        };
        assert!(!install::<Fake>(target));
        assert!(!armed::<Fake>() && m(|m| m.ready_calls) == 0);
    }
}

#[test]
fn active_lease_and_tick_restore() {
    for initial in 0..8 {
        reset();
        m(|m| m.systick = initial);
        assert!(install_provider(&provider()));
        write_lock::<Fake>();
        assert!(m(|m| m.acquire_calls == 1
            && m.release_calls == 0
            && m.legacy_locks == 0));
        assert!(
            m(|m| m.irq_mask == 1 && m.systick == 0)
                && authority(|a| a.phase) == FlashPhase::ACTIVE
        );
        write_unlock::<Fake>();
        assert!(m(|m| m.release_calls == 1
            && m.legacy_unlocks == 0
            && m.irq_mask == 0
            && m.systick == initial));
        assert_eq!(authority(|a| a.phase), FlashPhase::IDLE);
    }
    reset();
    m(|m| m.preempt_pending = true);
    assert!(install_provider(&provider()));
    write_lock::<Fake>();
    write_unlock::<Fake>();
    assert!(
        m(|m| m.acquire_calls == 2 && m.release_calls == 2) && authority(|a| a.token.lease) == 2
    );
    reset();
    let mut p = provider();
    assert!(install_provider(&p));
    p.acquire = None; // Installed callbacks are an immutable copy.
    p.release = None;
    std::hint::black_box(&p);
    write_lock::<Fake>();
    write_unlock::<Fake>();
    assert!(m(|m| m.acquire_calls == 1 && m.release_calls == 1));
}

#[test]
fn acquire_denial_never_returns_to_sdk_operation() {
    for change in 1..=7 {
        reset();
        m(|m| {
            m.bad_acquire = change;
            m.bad_fault = true;
        });
        assert!(install_provider(&provider()));
        if change == 7 {
            m(|m| m.bad_acquire = 0);
            write_lock::<Fake>();
            write_unlock::<Fake>();
            m(|m| m.bad_acquire = 7);
        }
        halts(write_lock::<Fake>);
        assert!(m(|m| m.last_fault == HookFault::ACQUIRE
            && m.fault_calls == 1
            && m.irq_mask == 1
            && m.systick == 0));
        let releases = if change == 7 { 1 } else { 0 };
        assert!(!armed::<Fake>() && m(|m| m.release_calls == releases && m.legacy_locks == 0));
        let (acquired, checked) = m(|m| (m.acquire_calls, m.ready_calls));
        m(|m| {
            m.irq_mask = 0;
            m.ipsr = 0;
        });
        assert!(!install_provider(&provider()) && m(|m| m.ready_calls) == checked);
        halts(write_lock::<Fake>); // A fault cannot fall back to legacy.
        assert!(m(|m| m.last_fault == HookFault::ACQUIRE
            && m.fault_calls == 1
            && m.acquire_calls == acquired
            && m.legacy_locks == 0));
    }
    reset();
    m(|m| {
        m.bad_acquire = 1;
        m.recursive_fault = true;
    });
    assert!(install_provider(&provider()));
    halts(write_lock::<Fake>);
    assert!(m(|m| m.fault_calls == 1
        && m.last_fault == HookFault::ACQUIRE
        && m.irq_mask == 1
        && m.systick == 0));
}

#[test]
fn release_failure_never_reenables_irqs_or_falls_back() {
    for change in 1..=4 {
        reset();
        m(|m| m.bad_release = change);
        assert!(install_provider(&provider()));
        write_lock::<Fake>();
        halts(write_unlock::<Fake>);
        assert!(m(|m| m.last_fault == HookFault::RELEASE
            && m.irq_mask == 1
            && m.systick == 0
            && m.legacy_unlocks == 0));
    }
}

#[test]
fn bad_context_order_and_peer_failure_halt() {
    for change in 0..5 {
        reset();
        assert!(install_provider(&provider()));
        match change {
            0 => m(|m| m.irq_mask = 1),
            1 => m(|m| m.ipsr = 19),
            2 => write_lock::<Fake>(),
            _ => {}
        }
        halts(|| match change {
            3 => write_unlock::<Fake>(),
            4 => {
                m(|m| m.bad_peer = true);
                write_ipc::<Fake>(authority_ptr(), 4, 2);
            }
            _ => write_lock::<Fake>(),
        });
        let expected = match change {
            0 | 1 => HookFault::CONTEXT,
            4 => HookFault::PEER,
            _ => HookFault::ORDER,
        };
        assert!(m(|m| m.last_fault == expected
            && m.irq_mask == 1
            && m.systick == 0));
        assert!(m(|m| m.legacy_locks == 0 && m.legacy_irqs == 0));
    }
    reset();
    assert!(install_provider(&provider()));
    m(|m| m.ipsr = 19);
    write_ipc::<Fake>(authority_ptr(), 4, 2);
    assert!(m(|m| m.peer_calls == 1
        && m.irq_mask == 0
        && m.systick == 7));
}

#[test]
fn countflag_left_by_a_disabled_systick_is_not_a_context_fault() {
    reset();
    assert!(install_provider(&provider()));
    // The counter wrapped before the provider wrote CTRL=0; the flag survives.
    m(|m| m.countflag = true);
    write_lock::<Fake>();
    assert!(
        m(|m| m.irq_mask == 1 && m.systick == 0) && authority(|a| a.phase) == FlashPhase::ACTIVE
    );
    m(|m| m.countflag = true);
    write_unlock::<Fake>();
    assert!(m(|m| m.release_calls == 1
        && m.irq_mask == 0
        && m.systick == 7));
    assert_eq!(authority(|a| a.phase), FlashPhase::IDLE);
    // An enabled SysTick after acquire still halts.
    reset();
    m(|m| m.bad_acquire = 3);
    assert!(install_provider(&provider()));
    halts(write_lock::<Fake>);
}
