//! The integration suite's command line: `itest` and `stock-ota`, which
//! `wellfed-bench` parses and `wellfed-itest` runs.
//!
//! The suite drives the bench through this crate's rig, so it cannot be
//! linked into this crate's own binary. `wellfed-bench` parses the whole
//! command line (so its help and its refusals are its own), then hands a
//! suite command to `wellfed-itest`, which takes the same arguments and
//! runs it through [`Suite`] (see [`crate::cli`]).

use std::path::PathBuf;

use clap::Args;
use wellfed_formats::bench::catalog::Tier;
use wellfed_formats::stock::SERVER_CONFIG;

use crate::Result;
use crate::commands::Bench;
use crate::commands::ota::{Firmware, Variant, parse_firmware, parse_variant};
use crate::device::OWNER_KEYS;

/// What runs the suite's commands, when it is linked in.
pub trait Suite {
    /// The scenarios a run takes on: its status steps.
    fn itest_steps(&self, args: &ItestArgs) -> Result<Vec<&'static str>>;
    /// The stages a `stock-ota` run takes on: its status steps.
    fn stock_ota_steps(&self, args: &StockOtaArgs) -> Vec<&'static str>;
    /// Run the selected scenarios on the live bench, restore it, then print
    /// and record the verdicts.
    fn itest(&self, bench: &mut Bench, args: &ItestArgs, stock_bin: PathBuf) -> Result<u8>;
    fn stock_ota(&self, bench: &mut Bench, args: &StockOtaArgs, stock_bin: PathBuf) -> Result<u8>;
}

#[derive(Debug, Clone, Args)]
pub struct ItestArgs {
    /// Session name: private/captures/NAME-uart-01, …, private/verification/NAME-*.
    #[arg(required_unless_present = "list")]
    pub name: Option<String>,
    /// Frozen review under test; installed over the air unless already running.
    #[arg(long, conflicts_with = "build")]
    pub review: Option<PathBuf>,
    /// Build the PLAF103 firstboot profile from this checkout and freeze it as NAME-good-review first.
    #[arg(long)]
    pub build: bool,
    /// Only these scenarios, comma-separated, whatever their tier; ota-update
    /// always runs first.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// How much of the suite to run.
    #[arg(long, value_enum, default_value_t = Tier::Standard)]
    pub tier: Tier,
    /// How long `soak` runs.
    #[arg(long, default_value_t = 60.0)]
    pub soak_minutes: f64,
    /// List the scenarios and what each needs, then exit.
    #[arg(long)]
    pub list: bool,
    /// Allow relay power cycles (cold boots), which some scenarios need.
    #[arg(long)]
    pub power: bool,
    /// Allow power cuts in the middle of an update. The design recovers
    /// from them over the air; if it does not, recovery is a UART flash.
    #[arg(long, requires = "power")]
    pub destructive: bool,
    /// Allow feeds through the device API (`feed-writes`): they move the
    /// motor, and run unattended, so the hopper may be empty.
    #[arg(long)]
    pub feeds: bool,
    /// Allow the round trip to stock and back (`stock-roundtrip`), installing
    /// this frozen stock review (the pinned 1.1.15). Stock reaches the server
    /// only through `sudo wellfed-stock network apply`, which the operator
    /// runs; a failure on stock is a UART recovery.
    #[arg(long, value_name = "STOCK_REVIEW", requires = "power")]
    pub stock: Option<PathBuf>,
    /// Run the remaining scenarios after one fails (never after the feeder goes silent).
    #[arg(long)]
    pub keep_going: bool,
    /// With --build: build even when a review of the same sources exists.
    #[arg(long, requires = "build")]
    pub rebuild: bool,
    /// wellfed-stock config the suite serves with (root-relative); its
    /// hotspot interface and feeder MAC also find the feeder when mDNS does not.
    #[arg(long, default_value = SERVER_CONFIG)]
    pub config: PathBuf,
    /// Hotspot description: its NetworkManager `connection` (root-relative).
    #[arg(long, default_value = "private/hotspot.json")]
    pub hotspot: PathBuf,
    /// The owner key file the device API sessions use (root-relative); it
    /// must name exactly one feeder.
    #[arg(long, default_value = OWNER_KEYS)]
    pub keys: PathBuf,
}

impl ItestArgs {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone, Args)]
pub struct StockOtaArgs {
    /// Session name: private/captures/NAME-uart-01, …, private/verification/NAME-*.
    pub name: String,
    /// No-motion firmware to build: qa (the default), optics, buttons, adc, speaker, blink, ble
    /// or trial.
    #[arg(long, value_parser = parse_firmware)]
    pub firmware: Option<Firmware>,
    /// Trial variant for --firmware trial: stall or fail.
    #[arg(long, value_parser = parse_variant)]
    pub variant: Option<Variant>,
    /// Build even when a review of the same sources exists.
    #[arg(long)]
    pub rebuild: bool,
    /// Stop once the build reports from bank 0, instead of offering it as a
    /// signed update that its recovery role installs into bank 1.
    #[arg(long)]
    pub no_promote: bool,
    /// Switch the feeder supply on if it is off (and off again afterwards).
    #[arg(long)]
    pub power: bool,
    /// wellfed-stock config the run serves with (root-relative).
    #[arg(long, default_value = SERVER_CONFIG)]
    pub config: PathBuf,
    /// Hotspot description: its NetworkManager `connection` (root-relative).
    #[arg(long, default_value = "private/hotspot.json")]
    pub hotspot: PathBuf,
    /// The owner key file that confirms the promoted build over the device
    /// API (root-relative).
    #[arg(long, default_value = OWNER_KEYS)]
    pub keys: PathBuf,
}
