//! `wellfed-bench`: the bench server (`serve`), which holds the lease and
//! the relay board and runs the rig's sequences and the integration suite
//! one job at a time, and its clients: one command per sequence, `attach`
//! and `status`. `serve --ui` serves the read-only progress page beside it.
//! The sequences live in `wellfed-rig`, the suite in `wellfed-itest` and
//! the server in `wellfed-bench-server`; this crate is their command line.

#![forbid(unsafe_code)]

use std::process::ExitCode;

mod cli;
mod note;
mod orphans;
mod page;
mod tui;

fn main() -> ExitCode {
    cli::main()
}
