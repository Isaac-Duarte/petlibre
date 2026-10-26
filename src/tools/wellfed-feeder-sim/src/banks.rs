//! The two banks, the factory loader's choice between them and KM0's boot
//! trial, for updates over the device API on the simulator.
//!
//! The update itself runs the firmware's own update stage
//! (`wellfed_hal::ota::update`) on [`Bus`], with the update store on the
//! simulator's storage image. What this models is the board around it:
//! - **The loader** boots bank 1 while its marker is intact, else bank 0.
//! - **KM0's trial**, the firmware's own decision (`wellfed_hal::trial`)
//!   on a simulated backup register: a bank-1 build the confirmation record
//!   does not name is counted at each boot; at the fourth, with bank 0
//!   valid, KM0 clears bank 1's first marker byte and the loader boots bank
//!   0. A confirmed build that keeps resetting soon after each boot gives
//!   way the same way once the resets make a storm. A power cut clears the
//!   counts, as it clears the backup register; a restart the feeder asked
//!   for, or a boot that ran long enough, ends a storm's count, as the
//!   runtime does.
//! - **A build's identity**: the one bank 0 last activated in bank 1, or
//!   the image each bank started with.
//!
//! The banks live in memory only: a simulator restarted from a flash file
//! starts again from the two images it began with.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_amebad_sys::{BANK0_OFFSET, BANK1_OFFSET, SECTOR_BYTES};
use wellfed_core::ota::{BANK1_BYTES, CONFIRM_LEN, MARKER, image};
use wellfed_core::store::update::Bank0;
use wellfed_hal::ota::update::Flash;
use wellfed_hal::trial::{self, Decision, Storm};
use sha2::{Digest, Sha256};

/// KM0's limit: unconfirmed boots before it gives way.
pub const TRIAL_LIMIT: u32 = trial::LIMIT;
const BANK0_BYTES: usize = (BANK1_OFFSET - BANK0_OFFSET) as usize;

/// The builds each bank starts with.
pub const BANK0_BUILD: [u8; 32] = [0xb0; 32];
pub const BANK1_BUILD: [u8; 32] = [0xb1; 32];

/// How a new build behaves in its trial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trial {
    /// It gets healthy and confirms itself.
    #[default]
    Confirms,
    /// It never confirms: each boot ends in a reset, as the supervisor's
    /// trial deadline makes it, until KM0 gives way.
    NeverConfirms,
    /// It confirms, then every boot from bank 1 ends in a reset nobody
    /// asked for soon after it starts, until KM0 sees a storm and gives way.
    Storms,
}

/// Which bank a boot runs, whether its build is on a trial boot, and what
/// a reset storm left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boot {
    pub bank: u8,
    pub trial: bool,
    pub storm: Storm,
}

#[derive(Debug)]
pub struct Banks {
    bank0: Vec<u8>,
    bank1: Vec<u8>,
    /// The build in bank 1, once bank 0 activated one there.
    pub bank1_build: [u8; 32],
    pub bank0_build: [u8; 32],
    /// KM0's backup register: this power-on's trial and storm counts.
    register: u32,
    /// How a newly activated build behaves.
    pub trial: Trial,
}

/// A bank laid out as the reviewed builds are: five core images, each a
/// header and its body, KM4's from a sector boundary.
fn image(seed: u8) -> Vec<u8> {
    fn part(out: &mut Vec<u8>, address: u32, body: u32, seed: u8) {
        out.extend_from_slice(&MARKER);
        out.extend_from_slice(&body.to_le_bytes());
        out.extend_from_slice(&address.to_le_bytes());
        out.extend_from_slice(&[0xff; 16]);
        out.extend((0..body).map(|i| seed ^ i.to_le_bytes()[0]));
    }
    let sector = SECTOR_BYTES as usize;
    let mut out = Vec::new();
    part(&mut out, 0x0C00_0020, 2_000, seed);
    part(&mut out, 0x0008_3000, 300, seed);
    out.resize(out.len().next_multiple_of(sector), 0xff);
    part(&mut out, 0x0E00_0020, 9_000, seed);
    part(&mut out, 0x1000_5000, 700, seed);
    part(&mut out, 0x0200_0020, 0, seed);
    out.resize(out.len().next_multiple_of(sector), 0xff);
    out
}

impl Banks {
    /// Both banks valid, with their first builds.
    pub fn new() -> Rc<RefCell<Self>> {
        let mut bank0 = image(0x30);
        bank0.resize(BANK0_BYTES, 0xff);
        let mut bank1 = image(0x31);
        bank1.resize(BANK1_BYTES as usize, 0xff);
        Rc::new(RefCell::new(Self {
            bank0,
            bank1,
            bank1_build: BANK1_BUILD,
            bank0_build: BANK0_BUILD,
            register: 0,
            trial: Trial::Confirms,
        }))
    }

    fn valid(bank: &[u8]) -> bool {
        bank[..8] == MARKER
    }

    /// The loader's and KM0's choice at a boot, against the confirmation
    /// record as storage sector 4 holds it. A give-way clears bank 1's
    /// first marker byte and boots again, as KM0's own reset does.
    pub fn boot(&mut self, record: &[u8; CONFIRM_LEN]) -> Boot {
        loop {
            let bank = if Self::valid(&self.bank1) { 1 } else { 0 };
            let identity = self.build(bank);
            let first = |bytes: &[u8]| -> [u8; 8] {
                let mut out = [0; 8];
                out.copy_from_slice(&bytes[..8]);
                out
            };
            // KM0's own boot count, in the register's low half.
            let counted = (self.register & !0xffff) | (self.register.wrapping_add(1) & 0xffff);
            let (decision, register) = trial::decide(
                &first(&self.bank1),
                &first(&self.bank0),
                record,
                counted,
                &identity,
            );
            self.register = register;

            let trial = match decision {
                Decision::GiveWay | Decision::StormGiveWay { .. } => {
                    self.bank1[0] = 0;
                    continue;
                }
                Decision::Counted(_) | Decision::NoFallback(_) => true,
                Decision::NotBank1 { .. } | Decision::Confirmed { .. } | Decision::Held { .. } => {
                    false
                }
            };
            let storm = Storm::of(trial::resets(self.register, &identity));
            return Boot { bank, trial, storm };
        }
    }

    /// Power was lost: KM0's counts go with the backup register.
    pub fn power_cut(&mut self) {
        self.register = 0;
    }

    /// The feeder restarts on purpose: not a fault, so no storm.
    pub fn deliberate(&mut self) {
        self.register = trial::deliberate(self.register);
    }

    /// A boot of `bank` ran long enough to end a storm's count.
    pub fn settle(&mut self, bank: u8) {
        self.register = trial::settled(self.register, &self.build(bank));
    }

    /// The build `bank` runs.
    pub fn build(&self, bank: u8) -> [u8; 32] {
        if bank == 1 {
            self.bank1_build
        } else {
            self.bank0_build
        }
    }

    pub fn bank1_valid(&self) -> bool {
        Self::valid(&self.bank1)
    }

    /// Bank 0's image as bank 0 records it at boot: the simulator starts as
    /// a feeder whose bank 0 has run once, built with `key`, as both banks
    /// are.
    pub fn bank0_record(&self, key: Option<[u8; 32]>) -> Option<Bank0> {
        let len = image::extent(
            |at, header| {
                let at = at as usize;
                let Some(bytes) = self.bank0.get(at..at + 32) else {
                    return false;
                };
                header.copy_from_slice(bytes);
                true
            },
            BANK1_OFFSET - BANK0_OFFSET,
        )?;
        Some(Bank0 {
            build_id: self.bank0_build,
            len,
            sha256: Sha256::digest(&self.bank0[..len as usize]).into(),
            key: key.map(|key| wellfed_core::ota::offer::key_id(&key)),
        })
    }

    /// Bank 1's bytes, for a test checking what was installed.
    pub fn bank1(&self) -> &[u8] {
        &self.bank1
    }
}

/// The flash as the update stage running from `bank` reaches it.
pub(crate) struct Bus<'a> {
    pub banks: &'a mut Banks,
    pub bank: u8,
}

impl Bus<'_> {
    fn sector(&mut self, sector: u32, offset: u32, len: usize) -> Option<std::ops::Range<usize>> {
        let end = usize::try_from(offset).ok()?.checked_add(len)?;
        let base = usize::try_from(sector.checked_mul(SECTOR_BYTES)?).ok()?;
        (self.bank == 0 && end <= SECTOR_BYTES as usize && base + end <= self.banks.bank1.len())
            .then(|| base + offset as usize..base + end)
    }
}

impl Flash for Bus<'_> {
    fn armed(&self) -> bool {
        true
    }

    fn erase(&mut self, sector: u32) -> bool {
        let Some(range) = self.sector(sector, 0, SECTOR_BYTES as usize) else {
            return false;
        };
        self.banks.bank1[range].fill(0xff);
        true
    }

    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        let Some(range) = self.sector(sector, offset, data.len()) else {
            return false;
        };
        for (cell, &byte) in self.banks.bank1[range].iter_mut().zip(data) {
            *cell &= byte;
        }
        true
    }

    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        let Some(range) = self.sector(sector, offset, bytes.len()) else {
            return false;
        };
        bytes.copy_from_slice(&self.banks.bank1[range]);
        true
    }

    fn bank0(&mut self, offset: u32, bytes: &mut [u8]) -> bool {
        let start = offset as usize;
        let Some(source) = self.banks.bank0.get(start..start + bytes.len()) else {
            return false;
        };
        bytes.copy_from_slice(source);
        true
    }

    fn give_way(&mut self) -> bool {
        let ok =
            self.bank == 1 && Banks::valid(&self.banks.bank1) && Banks::valid(&self.banks.bank0);
        if ok {
            self.banks.bank1[0] = 0;
        }
        ok
    }
}
