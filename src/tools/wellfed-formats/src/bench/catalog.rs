//! The integration suite's catalog: every scenario, in run order, with what
//! it checks, its tier and the session flags it needs.
//!
//! The suite pairs each entry with the code that runs it, `--list` prints it,
//! and the page lists it beside the run records, so the three cannot drift
//! apart. The page links no suite code, which is why the table lives here.

use serde::Serialize;

/// How much of the suite a run takes on; each tier includes the cheaper ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Minutes, no cold boots or installs beyond the build under test.
    Smoke,
    /// The whole functional suite (about half an hour).
    Standard,
    /// Standard, then long runs (`--soak-minutes`).
    Soak,
}

/// A session flag a scenario cannot run without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Power,
    Build,
    Destructive,
    Stock,
    Feeds,
}

impl Need {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Power => "--power",
            Self::Build => "--build",
            Self::Destructive => "--destructive",
            Self::Stock => "--stock",
            Self::Feeds => "--feeds",
        }
    }

    pub fn why(self) -> &'static str {
        match self {
            Self::Power => "it cold-boots the feeder",
            Self::Build => "it builds a variant",
            Self::Destructive => "it cuts power during an update",
            Self::Stock => {
                "it leaves the replacement for stock and back, through the feeder routing"
            }
            Self::Feeds => "it moves the motor; the hopper may be empty",
        }
    }
}

/// One scenario as the suite selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    pub name: &'static str,
    pub about: &'static str,
    /// The cheapest tier that runs it (`--tier`).
    pub tier: Tier,
    /// Session flags it cannot run without; it is skipped, never passed.
    pub needs: &'static [Need],
    /// Installs the build under test; every other scenario needs it.
    pub baseline: bool,
}

/// The whole suite, in run order.
pub const SCENARIOS: [Scenario; 11] = [
    Scenario {
        name: "ota-update",
        about: "install the build under test over the air and see it confirm",
        tier: Tier::Smoke,
        needs: &[],
        baseline: true,
    },
    Scenario {
        name: "requests",
        about: "status, settings, a clock sync, and a settings change read back and restored",
        tier: Tier::Smoke,
        needs: &[],
        baseline: false,
    },
    Scenario {
        name: "request-auth",
        about: "an unenrolled key is refused and a tampered handshake dropped; the owner still gets in",
        tier: Tier::Smoke,
        needs: &[],
        baseline: false,
    },
    Scenario {
        name: "settings-persist",
        about: "changed settings survive a cold boot and a stale write is refused, then they are restored",
        tier: Tier::Standard,
        needs: &[Need::Power],
        baseline: false,
    },
    Scenario {
        name: "wifi-fallback",
        about: "wrong stored Wi-Fi credentials fall back to the build's; a clear recovers",
        tier: Tier::Standard,
        needs: &[Need::Power],
        baseline: false,
    },
    Scenario {
        name: "network-bounce",
        about: "the hotspot goes down and up; the feeder rejoins and the device API answers again",
        tier: Tier::Standard,
        needs: &[],
        baseline: false,
    },
    Scenario {
        name: "feed-writes",
        about: "settings, lock and clock writes from a session held across a feed: no store write until it ends Done, a cancel stops the next",
        tier: Tier::Standard,
        needs: &[Need::Feeds],
        baseline: false,
    },
    Scenario {
        name: "ota-recovery",
        about: "a failed trial gives way to bank 0, which stops corrupt, out-of-order and stalled transfers, then restores the baseline",
        tier: Tier::Standard,
        needs: &[Need::Power, Need::Build],
        baseline: false,
    },
    Scenario {
        name: "ota-power-cuts",
        about: "power cut mid-download (bank 0 takes the offer again from the start) and early in a trial (the count restarts)",
        tier: Tier::Standard,
        needs: &[Need::Power, Need::Build, Need::Destructive],
        baseline: false,
    },
    Scenario {
        name: "stock-roundtrip",
        about: "to stock 1.1.15 over our updater, then back over stock's own and the bank-0 promotion",
        tier: Tier::Standard,
        needs: &[Need::Power, Need::Stock],
        baseline: false,
    },
    Scenario {
        name: "soak",
        about: "status every 30 s for --soak-minutes with no restart, link loss or missed answer",
        tier: Tier::Soak,
        needs: &[],
        baseline: false,
    },
];

/// One scenario as the page lists it: what it checks and what it needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CatalogEntry {
    pub name: &'static str,
    pub about: &'static str,
    /// The cheapest tier that runs it.
    pub tier: Tier,
    pub needs: Vec<Requirement>,
    /// Installs the build under test; every other scenario needs it.
    pub baseline: bool,
}

/// A session flag a scenario needs, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Requirement {
    pub flag: &'static str,
    pub why: &'static str,
}

/// The whole suite, in run order, as the page lists it.
pub fn catalog() -> Vec<CatalogEntry> {
    SCENARIOS
        .iter()
        .map(|s| CatalogEntry {
            name: s.name,
            about: s.about,
            tier: s.tier,
            needs: s
                .needs
                .iter()
                .map(|need| Requirement {
                    flag: need.flag(),
                    why: need.why(),
                })
                .collect(),
            baseline: s.baseline,
        })
        .collect()
}

/// `--list`: each scenario with what it needs.
pub fn listing() -> Vec<String> {
    catalog()
        .iter()
        .map(|s| {
            let mut needs: Vec<&str> = s.needs.iter().map(|need| need.flag).collect();
            match s.tier {
                Tier::Smoke => {}
                Tier::Standard => needs.insert(0, "standard"),
                Tier::Soak => needs.insert(0, "soak"),
            }
            let needs = if needs.is_empty() {
                String::new()
            } else {
                format!(" [{}]", needs.join(" "))
            };
            format!("{:<18} {}{needs}", s.name, s.about)
        })
        .collect()
}
