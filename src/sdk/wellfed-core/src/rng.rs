//! FW45: the random generator behind every key and nonce.
//!
//! The AmebaD has no hardware RNG the SDK exposes: its `rtw_get_random_bytes`
//! is ROM `Rand()`, a Tausworthe generator seeded with a constant (static
//! analysis of the pinned SDK). So the firmware samples physical noise
//! itself and this module turns it into output, after SP 800-90B and 90A:
//!
//! 1. **Health tests** ([`Health`]) watch every raw sample: the Repetition
//!    Count and Adaptive Proportion tests, with cutoffs derived from the
//!    min-entropy the source claims per sample at a false-alarm rate of
//!    2^-20. The first [`STARTUP_SAMPLES`] samples are tested before any
//!    output. A failure latches until reboot; nothing retries around it.
//! 2. **The pool** ([`Pool`]) hashes tested samples with SHA-256 and gives a
//!    32-byte input only once twice its length in claimed entropy went in.
//! 3. **HMAC-DRBG** ([`Drbg`], SHA-256) stretches that input. It refuses to
//!    run past [`RESEED_INTERVAL`] requests without fresh pool input.
//!
//! The stored seed ([`crate::store::seed`]) joins the first instantiation
//! as its nonce, so even a source weaker than claimed does not repeat a
//! stream across boots. The per-sample claim is the source's to make, from
//! bench measurement; nothing here can check it.

use sha2::{Digest, Sha256};

use crate::mac::hmac_sha256;

/// Raw samples tested before the first output (SP 800-90B 4.3).
pub const STARTUP_SAMPLES: u32 = 1024;
/// The Adaptive Proportion Test's window for non-binary samples.
pub const APT_WINDOW: u32 = 512;
/// Generate requests between reseeds. SP 800-90A allows 2^48; reseeding far
/// sooner keeps a compromised state short-lived and costs one pool fill.
pub const RESEED_INTERVAL: u32 = 1 << 16;
/// The largest single request (SP 800-90A: 2^19 bits).
pub const MAX_REQUEST: usize = 1 << 16;
/// The false-alarm exponent: a healthy source fails a test with
/// probability 2^-20 per window.
const ALPHA_BITS: u32 = 20;
/// 2^(-1/8): claims are in eighths of a bit.
const EIGHTH_BIT: f64 = 0.917_004_043_204_671_2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A health test failed; latched until reboot.
    Health,
    /// The pool has not seen enough tested entropy yet.
    NotReady,
    /// The DRBG needs fresh pool input first.
    ReseedRequired,
    /// A request over [`MAX_REQUEST`] bytes.
    TooLarge,
}

/// The two continuous health tests over one source's raw samples.
#[derive(Clone, Copy, Debug)]
pub struct Health {
    rct_cutoff: u32,
    apt_cutoff: u32,
    last: u8,
    run: u32,
    apt_reference: u8,
    apt_matches: u32,
    apt_seen: u32,
    tested: u32,
    failed: bool,
}

impl Health {
    /// Tests for a source claiming `eighths`/8 bits of min-entropy per
    /// sample, 1 to 64 (an 8-bit sample holds at most 8 bits).
    pub const fn new(eighths: u32) -> Self {
        assert!(eighths >= 1 && eighths <= 64);
        Self {
            rct_cutoff: rct_cutoff(eighths),
            apt_cutoff: apt_cutoff(eighths),
            last: 0,
            run: 0,
            apt_reference: 0,
            apt_matches: 0,
            apt_seen: 0,
            tested: 0,
            failed: false,
        }
    }

    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// The Repetition Count Test's cutoff: a run this long fails.
    pub const fn repetition_cutoff(&self) -> u32 {
        self.rct_cutoff
    }

    /// The Adaptive Proportion Test's cutoff: this many matches in one
    /// window fails.
    pub const fn proportion_cutoff(&self) -> u32 {
        self.apt_cutoff
    }

    /// The start-up test has passed: enough samples tested, none failed.
    pub const fn started(&self) -> bool {
        !self.failed && self.tested >= STARTUP_SAMPLES
    }

    /// Test one raw sample.
    pub fn sample(&mut self, value: u8) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Health);
        }

        // Repetition Count: the same value too many times in a row.
        if self.tested > 0 && value == self.last {
            self.run += 1;
        } else {
            self.last = value;
            self.run = 1;
        }

        // Adaptive Proportion: the window's first value too often within it.
        if self.apt_seen == 0 {
            self.apt_reference = value;
            self.apt_matches = 1;
        } else if value == self.apt_reference {
            self.apt_matches += 1;
        }
        self.apt_seen = (self.apt_seen + 1) % APT_WINDOW;

        self.tested = self.tested.saturating_add(1);
        if self.run >= self.rct_cutoff || self.apt_matches >= self.apt_cutoff {
            self.failed = true;
            return Err(Error::Health);
        }
        Ok(())
    }
}

/// SP 800-90B 4.4.1: C = 1 + ceil(20 / H).
const fn rct_cutoff(eighths: u32) -> u32 {
    1 + (ALPHA_BITS * 8).div_ceil(eighths)
}

/// SP 800-90B 4.4.2: C = 1 + CRITBINOM(W, 2^-H, 1 - 2^-20), the smallest
/// count whose binomial CDF reaches 1 - 2^-20, by the exact recurrence.
const fn apt_cutoff(eighths: u32) -> u32 {
    let mut p = 1.0;
    let mut i = 0;
    while i < eighths {
        p *= EIGHTH_BIT;
        i += 1;
    }
    let target = 1.0 - 1.0 / (1u32 << ALPHA_BITS) as f64;

    // P(X = 0) = (1 - p)^W, then P(X = k + 1) = P(X = k) (W - k) p / ((k + 1) (1 - p)).
    let mut term = 1.0;
    let mut i = 0;
    while i < APT_WINDOW {
        term *= 1.0 - p;
        i += 1;
    }
    let mut cdf = term;
    let mut k = 0;
    while cdf < target && k < APT_WINDOW {
        term *= (APT_WINDOW - k) as f64 * p / ((k + 1) as f64 * (1.0 - p));
        cdf += term;
        k += 1;
    }
    1 + k
}

/// Health-tested samples hashed into 32-byte DRBG inputs.
#[derive(Clone, Debug)]
pub struct Pool {
    health: Health,
    eighths: u32,
    hash: Sha256,
    /// Claimed entropy in the hash so far, in eighths of a bit.
    claimed: u32,
}

impl Pool {
    /// A pool over one source; see [`Health::new`] for `eighths`.
    pub fn new(eighths: u32) -> Self {
        Self {
            health: Health::new(eighths),
            eighths,
            hash: Sha256::new(),
            claimed: 0,
        }
    }

    pub const fn failed(&self) -> bool {
        self.health.failed()
    }

    /// Add one raw sample. A failed test discards everything gathered.
    pub fn add(&mut self, value: u8) -> Result<(), Error> {
        if let Err(error) = self.health.sample(value) {
            self.hash = Sha256::new();
            self.claimed = 0;
            return Err(error);
        }
        self.hash.update([value]);
        self.claimed = self.claimed.saturating_add(self.eighths);
        Ok(())
    }

    /// Enough tested entropy for [`Pool::take`]: twice the output's 256
    /// bits, so the hash's output is full entropy with margin.
    pub const fn ready(&self) -> bool {
        self.health.started() && self.claimed >= 2 * 256 * 8
    }

    /// The conditioned 32 bytes; the pool starts over.
    pub fn take(&mut self) -> Result<[u8; 32], Error> {
        if self.health.failed() {
            return Err(Error::Health);
        }
        if !self.ready() {
            return Err(Error::NotReady);
        }
        self.claimed = 0;
        let hash = core::mem::replace(&mut self.hash, Sha256::new());
        Ok(hash.finalize().into())
    }
}

/// HMAC-DRBG with SHA-256 (SP 800-90A 10.1.2), without prediction resistance:
/// the caller reseeds from the pool when told to, and may reseed sooner.
#[derive(Clone)]
pub struct Drbg {
    key: [u8; 32],
    value: [u8; 32],
    requests: u32,
}

impl core::fmt::Debug for Drbg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Drbg")
            .field("requests", &self.requests)
            .finish_non_exhaustive()
    }
}

impl Drbg {
    /// Instantiate from `entropy`, a `nonce` and a personalization string.
    pub fn new(entropy: &[u8], nonce: &[u8], personalization: &[u8]) -> Self {
        let mut drbg = Self {
            key: [0; 32],
            value: [1; 32],
            requests: 1,
        };
        drbg.update(&[entropy, nonce, personalization]);
        drbg
    }

    /// Mix in fresh `entropy` (and optional `additional` input).
    pub fn reseed(&mut self, entropy: &[u8], additional: &[u8]) {
        self.update(&[entropy, additional]);
        self.requests = 1;
    }

    /// Fill `out`, mixing in `additional` input if any.
    pub fn generate(&mut self, out: &mut [u8], additional: &[u8]) -> Result<(), Error> {
        if out.len() > MAX_REQUEST {
            return Err(Error::TooLarge);
        }
        if self.requests > RESEED_INTERVAL {
            return Err(Error::ReseedRequired);
        }

        if !additional.is_empty() {
            self.update(&[additional]);
        }
        for chunk in out.chunks_mut(32) {
            self.value = hmac_sha256(&self.key, &[&self.value]);
            chunk.copy_from_slice(&self.value[..chunk.len()]);
        }
        self.update(&[additional]);
        self.requests += 1;
        Ok(())
    }

    /// 32 bytes, for a key or seed.
    pub fn bytes(&mut self) -> Result<[u8; 32], Error> {
        let mut out = [0; 32];
        self.generate(&mut out, &[])?;
        Ok(out)
    }

    /// HMAC_DRBG_Update over the concatenated `data`.
    fn update(&mut self, data: &[&[u8]]) {
        debug_assert!(data.len() <= 4, "update takes at most four parts");
        let provided = data.iter().any(|part| !part.is_empty());
        for round in [0u8, 1] {
            if round == 1 && !provided {
                break;
            }
            let mut parts: [&[u8]; 6] = [&[]; 6];
            parts[0] = &self.value;
            let separator = [round];
            parts[1] = &separator;
            for (slot, part) in parts[2..].iter_mut().zip(data) {
                *slot = part;
            }
            self.key = hmac_sha256(&self.key, &parts);
            self.value = hmac_sha256(&self.key, &[&self.value]);
        }
    }
}
