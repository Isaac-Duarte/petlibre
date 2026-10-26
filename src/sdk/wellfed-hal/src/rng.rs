//! The firmware's one random generator, for every key and nonce.
//!
//! [`start`] runs once in the startup task, before the transport, the power
//! monitor or the firmware can touch the ADC. It fills a health-tested pool
//! from the internal supply channel's low byte at the claim the bench
//! measured ([`CLAIM_EIGHTHS`], 1 bit against 1.44 measured), mixes in
//! clock-jitter samples uncredited, and instantiates HMAC-DRBG. Every task
//! then draws from that one generator with the scheduler paused for the
//! draw, so two tasks never share an output. The settings writer later mixes
//! in the seed stored across boots and stores the next one
//! (`wellfed_core::store::seed`).
//!
//! A failed start (the ADC not opening, a health test failing, the pool
//! never filling) leaves no generator: [`fill`] refuses, and callers that
//! need secrets go without rather than use weaker bytes.
//!
//! See [pairing](../../../../docs/firmware/pairing.md#measured-noise-physical-october-24).

use wellfed_core::named_enum;
use wellfed_core::rng::{Drbg, Pool};

use crate::entropy::{self, Jitter};
use crate::{adc, plaf103};

/// One bit per ADC sample, in eighths: below the 1.44 bits the bench measured.
pub const CLAIM_EIGHTHS: u32 = 8;
/// ADC samples before the boot fill gives up; it needs 1,024 when healthy.
pub const SAMPLE_LIMIT: u32 = 8_192;
/// Raw samples [`Started`] reports, for the restart check across boots.
pub const FIRST: usize = 16;
const CHANNEL: u32 = plaf103::pins::ADC_SUPPLY;
const POLLS: u32 = 10_000;
/// Jitter samples mixed in as the DRBG's nonce, credited nothing.
const JITTER: usize = 64;
const PERSONALIZATION: &[u8] = b"wellfed-fw45-drbg";

named_enum! {
    pub enum Failure {
        Adc = "adc",
        Health = "health",
        Exhausted = "exhausted",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started {
    /// ADC samples the pool took.
    pub samples: u32,
    /// The first raw samples, for comparing boots.
    pub first: [u8; FIRST],
    /// `Err` leaves the firmware with no generator.
    pub result: Result<(), Failure>,
}

/// Fill the pool and start the generator. Call once, from the startup task,
/// before any other task uses the ADC; the ADC is closed again after.
pub fn start() -> Started {
    let mut started = Started {
        samples: 0,
        first: [0; FIRST],
        result: Err(Failure::Adc),
    };
    if adc::open().calibration.is_none() {
        adc::close();
        return started;
    }

    let mut pool = Pool::new::<CLAIM_EIGHTHS>();
    let input = loop {
        if pool.ready() {
            break pool.take();
        }
        if started.samples >= SAMPLE_LIMIT {
            started.result = Err(Failure::Exhausted);
            break Err(wellfed_core::rng::Error::NotReady);
        }
        let Some(value) = entropy::adc_sample(CHANNEL, POLLS) else {
            started.result = Err(Failure::Adc);
            break Err(wellfed_core::rng::Error::NotReady);
        };
        if let Some(slot) = started.first.get_mut(started.samples as usize) {
            *slot = value;
        }
        started.samples += 1;
        if pool.add(value).is_err() {
            started.result = Err(Failure::Health);
            break Err(wellfed_core::rng::Error::Health);
        }
    };
    adc::close();
    let Ok(input) = input else {
        return started;
    };

    let mut jitter = Jitter::new();
    let mut nonce = [0; JITTER];
    for byte in &mut nonce {
        *byte = jitter.sample().unwrap_or(0);
    }
    install(Drbg::new(&input, &nonce, PERSONALIZATION));
    started.result = Ok(());
    started
}

/// Whether the generator is running.
pub fn ready() -> bool {
    with_generator(|generator| generator.is_some())
}

/// Fill `out` from the generator; false (and `out` untouched) without one,
/// or once it needs a reseed it cannot get.
pub fn fill(out: &mut [u8]) -> bool {
    with_generator(|generator| {
        generator
            .as_mut()
            .is_some_and(|drbg| drbg.generate(out, &[]).is_ok())
    })
}

/// 32 bytes from the generator, if it runs.
pub fn bytes() -> Option<[u8; 32]> {
    let mut out = [0; 32];
    fill(&mut out).then_some(out)
}

/// Mix `input` (the stored seed) into the generator's state; it is not
/// counted as fresh entropy.
pub fn mix(input: &[u8]) -> bool {
    with_generator(|generator| {
        generator
            .as_mut()
            .is_some_and(|drbg| drbg.generate(&mut [], input).is_ok())
    })
}

fn install(drbg: Drbg) {
    with_generator(|generator| *generator = Some(drbg));
}

/// Tasks only: the pause keeps every other task off the generator.
#[cfg(target_os = "none")]
fn with_generator<R>(f: impl FnOnce(&mut Option<Drbg>) -> R) -> R {
    static GENERATOR: crate::cell::TaskCell<Option<Drbg>> = crate::cell::TaskCell::new(None);
    crate::sdk::unscheduled(|| {
        // SAFETY: with the scheduler paused no other task runs, and no
        // interrupt handler reaches the generator.
        f(unsafe { &mut *GENERATOR.get() })
    })
}

/// The host's fake runs per test thread, so the generator does too.
#[cfg(not(target_os = "none"))]
fn with_generator<R>(f: impl FnOnce(&mut Option<Drbg>) -> R) -> R {
    std::thread_local! {
        static GENERATOR: core::cell::RefCell<Option<Drbg>> = const { core::cell::RefCell::new(None) };
    }
    GENERATOR.with_borrow_mut(f)
}

/// Forget the generator (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    with_generator(|generator| *generator = None);
}
