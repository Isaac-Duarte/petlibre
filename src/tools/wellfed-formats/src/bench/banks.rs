//! `private/bench-status/banks.json`: which build the bench believes each
//! flash bank holds, and what last confirmed or contradicted it.
//!
//! The ledger is written from job results (a UART install, a give-way, an
//! over-the-air install that confirmed or fell back, a stock-OTA install),
//! and seeded once from a full read. Each entry is then checked against
//! what the feeder itself reports: the `PLFW1 start build= bank=` banner on
//! the UART and the build and bank in the device API's `Status`. A report
//! that names another build marks the bank [`Check::Disputed`] rather than
//! overwriting it, because either side may be wrong and the operator
//! decides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Root-relative: the ledger.
pub const BANKS: &str = "private/bench-status/banks.json";

/// A flash bank: 0 is the factory loader's fallback, 1 the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Bank {
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "1")]
    One,
}

/// The ledger: an entry per bank the bench has evidence for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Ledger {
    pub banks: BTreeMap<Bank, Entry>,
}

/// What one bank holds, by whose word, and whether the feeder agrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Entry {
    pub content: Content,
    pub source: Source,
    /// The session name of the job that wrote it.
    pub session: Option<String>,
    pub at: f64,
    pub check: Check,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    /// A replacement-firmware build, by its build identity.
    Build { id: String },
    /// The stock application.
    Stock,
    /// No valid image: bank 1 after a give-way.
    Invalid,
}

/// The job result an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// `banks --seed` from a full read.
    Seed,
    FlashExecute,
    GiveWay,
    /// An over-the-air install that confirmed.
    OtaConfirmed,
    /// An over-the-air install whose trial fell back: bank 1 holds the
    /// candidate, which no longer boots.
    OtaFellBack,
    StockOta,
}

/// Whether the feeder's own report agrees with the entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Check {
    /// Nothing has reported on this bank since the entry was written.
    Unchecked,
    Confirmed {
        by: Witness,
        at: f64,
    },
    /// The feeder reported another build here.
    Disputed {
        by: Witness,
        at: f64,
        seen: Content,
    },
}

/// What reported a bank's build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Witness {
    /// The `PLFW1 start` banner in a capture.
    Banner,
    /// The device API's `Status`.
    Status,
}

impl Ledger {
    /// A job wrote `content` into `bank`: the new entry is unchecked until
    /// the feeder reports.
    pub fn record(
        &mut self,
        bank: Bank,
        content: Content,
        source: Source,
        session: Option<String>,
        at: f64,
    ) {
        let entry = Entry {
            content,
            source,
            session,
            at,
            check: Check::Unchecked,
        };
        self.banks.insert(bank, entry);
    }

    /// The feeder reported running `seen` from `bank`. A bank with no entry
    /// stays unknown: a report alone does not say how the build got there.
    pub fn observe(&mut self, bank: Bank, seen: Content, by: Witness, at: f64) {
        let Some(entry) = self.banks.get_mut(&bank) else {
            return;
        };
        entry.check = if entry.content == seen {
            Check::Confirmed { by, at }
        } else {
            Check::Disputed { by, at, seen }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(id: &str) -> Content {
        Content::Build { id: id.into() }
    }

    #[test]
    fn a_report_confirms_or_disputes_what_a_job_wrote() {
        let mut ledger = Ledger::default();
        ledger.record(
            Bank::One,
            build("aa11"),
            Source::FlashExecute,
            Some("unit".into()),
            1.0,
        );

        ledger.observe(Bank::One, build("aa11"), Witness::Banner, 2.0);
        assert_eq!(
            ledger.banks[&Bank::One].check,
            Check::Confirmed {
                by: Witness::Banner,
                at: 2.0
            }
        );

        ledger.observe(Bank::One, build("bb22"), Witness::Status, 3.0);
        let entry = &ledger.banks[&Bank::One];
        assert_eq!(entry.content, build("aa11"));
        assert!(
            matches!(&entry.check, Check::Disputed { seen, .. } if *seen == build("bb22")),
            "{entry:?}"
        );
    }

    #[test]
    fn a_report_on_an_unknown_bank_records_nothing() {
        let mut ledger = Ledger::default();

        ledger.observe(Bank::Zero, Content::Stock, Witness::Banner, 1.0);

        assert!(ledger.banks.is_empty());
    }

    #[test]
    fn the_ledger_reads_back_keyed_by_bank_number() {
        let mut ledger = Ledger::default();
        ledger.record(Bank::One, Content::Invalid, Source::GiveWay, None, 1.0);
        ledger.record(Bank::Zero, Content::Stock, Source::Seed, None, 1.0);

        let text = serde_json::to_string(&ledger).unwrap();

        assert!(
            text.contains(r#""0":{"content":{"kind":"stock"}"#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Ledger>(&text).unwrap(), ledger);
    }
}
