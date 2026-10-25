//! `wellfed-bench`: relay power, the loader strap, and supervised
//! captures, dumps and updates through `wellfed-flash`; `--ui` serves the
//! read-only progress page. The suite's commands run in `wellfed-itest`
//! (see [`wellfed_bench::cli`]).

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    wellfed_bench::cli::main(None)
}
