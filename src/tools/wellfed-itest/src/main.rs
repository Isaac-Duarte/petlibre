//! `wellfed-itest`: the integration suite's commands (`itest`, `stock-ota`)
//! on `wellfed-bench`'s command line. `wellfed-bench` hands those commands
//! here with the arguments it was given; this binary runs nothing else.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    wellfed_bench::cli::main(Some(&wellfed_itest::Linked))
}
