//! The bank ledger the server keeps (`wellfed_formats::bench::banks`):
//! written from job results, checked against what the feeder reports, and
//! seeded by `wellfed-bench banks --seed`.
//!
//! Every change goes to the live state and to `banks.json`, replaced whole
//! and never synced. Nothing here writes while a job holds the UART: job
//! results land after the job, and the capture and the session that report
//! builds are both stopped during UART jobs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use wellfed_formats::bench::banks::{BANKS, Bank, Content, Ledger, Source, Witness, Written};
use wellfed_formats::bench::{Paths, now};

use crate::api::{Seed, SeedError, Seeded, SeededFrom};
use crate::files;
use crate::state::Store;

/// The ledger, shared by the jobs, the capture, the feeder session and the
/// socket.
#[derive(Clone)]
pub(crate) struct Banks {
    ledger: Arc<Mutex<Ledger>>,
    root: PathBuf,
    store: Store,
}

impl Banks {
    /// The ledger in the checkout at `root`, or an empty one when there is
    /// none. One that does not read is refused, never replaced: it is the
    /// operator's to repair.
    pub(crate) fn open(root: &Path, store: Store) -> Result<Self, wellfed_formats::Error> {
        let ledger: Ledger = match wellfed_formats::load(&root.join(BANKS)) {
            Ok(ledger) => ledger,
            Err(wellfed_formats::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Ledger::default()
            }
            Err(error) => return Err(error),
        };
        store.update(|state| state.banks = ledger.clone());
        Ok(Self {
            ledger: Arc::new(Mutex::new(ledger)),
            root: root.to_path_buf(),
            store,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn read(&self) -> Ledger {
        self.lock().clone()
    }

    /// What a finished job's sequence wrote, under its session name.
    pub(crate) fn record(&self, written: &[Written], session: Option<&str>) {
        if written.is_empty() {
            return;
        }
        let mut ledger = self.lock();
        for write in written {
            ledger.record(
                write.bank,
                write.content.clone(),
                write.source,
                session.map(str::to_string),
                now(),
            );
        }
        self.publish(&ledger);
    }

    /// The feeder said it runs `build` from `bank`.
    pub(crate) fn observe(&self, bank: u8, build: &str, by: Witness) {
        let Some(bank) = Bank::of(bank) else {
            return;
        };
        let seen = Content::Build {
            id: build.to_ascii_lowercase(),
        };
        let mut ledger = self.lock();
        if ledger.observe(bank, seen, by, now()) {
            self.publish(&ledger);
        }
    }

    /// `banks --seed`: the dump session's inspection if it names a bank and
    /// a build, else the operator's own.
    pub(crate) fn seed(&self, seed: &Seed) -> Result<Seeded, SeedError> {
        if !wellfed_formats::bench::plain_name(&seed.session) {
            return Err(SeedError::BadName {
                name: seed.session.clone(),
            });
        }
        let report = Paths::new(&self.root, &seed.session)
            .inspect
            .join("report.json");
        let inspection = wellfed_formats::bench::relative(&self.root, &report);

        let (bank, build, from) = match (inspected(&report), seed.bank, &seed.build) {
            (Some((bank, build)), _, _) => (bank, build, SeededFrom::Inspection { inspection }),
            (None, Some(bank), Some(build)) => (
                bank,
                build.to_ascii_lowercase(),
                SeededFrom::Operator { inspection },
            ),
            _ => return Err(SeedError::NoBuild { inspection }),
        };
        let slot = Bank::of(bank).ok_or(SeedError::NoBank { bank })?;

        let mut ledger = self.lock();
        ledger.record(
            slot,
            Content::Build { id: build.clone() },
            Source::Seed,
            Some(seed.session.clone()),
            now(),
        );
        self.publish(&ledger);
        Ok(Seeded {
            bank,
            build,
            from,
            ledger: ledger.clone(),
        })
    }

    fn publish(&self, ledger: &Ledger) {
        self.store.update(|state| state.banks = ledger.clone());
        // A full disk loses the file's copy, never the live one.
        let _ = files::replace_json(&self.root.join(BANKS), ledger);
    }
}

/// The bank and build an inspection report names, when it does: today's
/// flash inspections pin region hashes, so this is `None` until one
/// reports the startup identity it found.
fn inspected(report: &Path) -> Option<(u8, String)> {
    #[derive(Deserialize)]
    struct Report {
        result: Option<Found>,
    }
    #[derive(Deserialize)]
    struct Found {
        bank: Option<u8>,
        startup_identity: Option<String>,
    }

    let report: Report = wellfed_formats::load(report).ok()?;
    let found = report.result?;
    Some((found.bank?, found.startup_identity?.to_ascii_lowercase()))
}
