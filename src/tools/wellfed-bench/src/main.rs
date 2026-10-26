//! `wellfed-bench`: relay power, the loader strap, and supervised
//! captures, dumps and updates through `wellfed-flash`, and the
//! integration suite's commands; `--ui` serves the read-only progress page.
//! The sequences live in `wellfed-rig` and the suite in `wellfed-itest`;
//! this crate is their command line.

#![forbid(unsafe_code)]

use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    cli::main()
}
