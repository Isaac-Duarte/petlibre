//! A bounded, foreground NTP responder for the bench's live NTP check
//! (docs/firmware/clock.md#bench-plan):
//!
//! ```text
//! cargo run -p wellfed-rig --example ntp-serve -- 10.42.103.1:123 --seconds 1800
//! ```
//!
//! It answers with this host's clock until the time is up or Ctrl+C, and
//! prints each answer. Port 123 needs the privilege to bind it; any other
//! port the feeder's NTP setting names works unprivileged. Either must be
//! admitted from the hotspot by the host firewall, which this never
//! changes.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// The address and port to answer on.
    listen: SocketAddr,
    /// How long to run (at most 3600).
    #[arg(long)]
    seconds: u64,
    /// The dispersion to claim for this host's own clock, in milliseconds.
    #[arg(long, default_value_t = 10)]
    dispersion_ms: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let run = Duration::from_secs(args.seconds);
    eprintln!("Answering NTP on {} for {} s.", args.listen, args.seconds);
    let count = wellfed_rig::ntp::serve(
        args.listen,
        run,
        args.dispersion_ms,
        || false,
        |answered| println!("answered {} unix_ms={}", answered.from, answered.unix_ms),
    )?;
    eprintln!("Answered {count} requests.");
    Ok(())
}
