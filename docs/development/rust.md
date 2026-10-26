# Rust standard

The rules for every Rust crate in the workspace, why each exists, and what enforces it. [AGENTS.md](../../AGENTS.md#rust-conventions) keeps the short form; this page is the full one. A rule enforced by a tool is checked by `make check`; a rule marked **review** relies on whoever reads the change.

## Layout

The workspace is `src/sdk/` (the SDK every firmware builds on), `src/product/` (product policy shared by firmware and server), `src/tools/wellfed-*` and `src/tools/xtask` (host tools) and `src/firmware/wellfed/` (one `no_std` staticlib per firmware). Every crate sets `license.workspace = true` and `publish.workspace = true` and inherits `[workspace.lints]`. Every host tool is Rust; `cargo xtask check-repository` refuses Python under `src/`.

## Rules

| Rule | Why | Enforced by |
| --- | --- | --- |
| **No panics on input.** No `unwrap` in library or binary code; `expect` only for an invariant the code just established, with the reason as its message. Use `let … else`, `try_from` and `?`. | A malformed file, packet or reply must become an error a caller can report, never a crash mid-flash or mid-feed. | `PRODUCTION_LINTS` (`clippy::unwrap_used`) in `make check-rust`; review for `expect` |
| **No silent narrowing.** `try_from` instead of a narrowing `as`. | Offsets, lengths and sector numbers that wrap write to the wrong place. | `PRODUCTION_LINTS` (`clippy::cast_possible_truncation`) |
| **Narrow visibility.** Default to `pub(crate)`; `pub` only for what another crate or an integration test uses. | The public surface is what a later change has to keep compatible. | `PRODUCTION_LINTS` (`unreachable_pub`) |
| **Borrow what you read.** No needless by-value parameters. | Clones and moves that only exist to satisfy a signature. | `PRODUCTION_LINTS` (`clippy::needless_pass_by_value`) |
| **No leftovers.** No `dbg!`, `todo!` or `unimplemented!`. | Each is a debugging aid or a placeholder that panics at run time. | `[workspace.lints.clippy]` (`dbg_macro`, `todo`, `unimplemented`) |
| **Deliberate exceptions.** A lint exception is `#[expect(lint, reason = "…")]` at the narrowest scope. | `expect` warns when the exception stops being needed; the reason says why it was. | Review. Older `#[allow]`s without a reason remain; convert them when touching the code. |
| **Typed data.** JSON is `#[derive(Serialize, Deserialize)]` structs and enums; `serde_json::Value` only for JSON that is genuinely opaque (hashed or replayed verbatim, or passed through unread). | A field typo or a missing field fails at the parse, not three calls later. | Review |
| **Shared formats.** A file one crate writes and another reads has its type in `wellfed-formats`; anything hashed goes through `wellfed_formats::canonical`. | Two copies of a type drift; hashed bytes must not depend on field order. | Review |
| **Enums, not strings or flags.** Closed sets are enums (`clap::ValueEnum` for CLI options); prefer a two-variant enum to a `bool` whose meaning isn't clear at the call site. Firmware states and faults are enums with fixed discriminants (`code()`, and `from_byte` where flash bytes are decoded), not integer wrappers. | The compiler checks every match when a variant is added. | Review |
| **Errors.** Library crates expose a `thiserror` enum; binaries and CLI glue use `anyhow` with context. No `Result<_, String>`, no catch-all string variants where a typed variant fits. Checks go through the crate's `require` helper. | Callers can match on what went wrong; the operator reads a chain of context. | Review |
| **One helper, one place.** Search before writing hex, hashing, HMAC, report-writing or serial helpers. | The workspace already has them, and a second copy diverges. | Review |
| **`unsafe` at the edge only.** Host crates forbid `unsafe` at their roots. Firmware permits it only at the C ABI and MMIO boundary, each block with a `// SAFETY:` comment, and `unsafe_op_in_unsafe_fn` is denied. Between Rust crates an interface is a trait, never an `extern "C"` table with a context pointer. | Every unsafe operation is visible and justified where it happens, and a call between Rust crates is direct, so the stack audit follows it (it refuses a Rust indirect call outside `core::fmt`). | `#![forbid(unsafe_code)]` in host crates; `[workspace.lints]` (`unsafe_op_in_unsafe_fn`, `clippy::undocumented_unsafe_blocks`) |
| **Audited binaries.** Changing `repr(C)` types, `#[no_mangle]` symbols, the park/hook RAM code or flash record layouts reruns the link, RAM-park and stack audits and keeps their pins. | Those are what the pinned recovery path depends on. | The image build's audits |
| **Readable code.** Blank lines between a function's logical steps; comments and `//!` docs explain why (invariants, hardware or protocol constraints, ordering), not what the next line does. | The reader is usually debugging a bench run. | `cargo fmt` for layout; review for the rest |

## Tests

`make check-rust` runs the suite with [cargo-nextest](https://nexte.st): each test in its own process, every failure reported in one run, and a test still running after 60 s named, then killed after 180 s ([.config/nextest.toml](../../.config/nextest.toml)). Doctests run separately with `cargo test --doc`, which nextest does not cover.

- Tests may `unwrap`, `expect` and `dbg!` ([clippy.toml](../../clippy.toml)); a panic there is a failed test.
- Hardware and fixture evidence tests are `#[ignore]`d and run only from `make check-fixtures`, which stays on `cargo test` for its `--ignored --exact` selection. An ignored or skipped test is never a pass.
- Network tests bind loopback sockets with synthetic identities.
- The network parsers (the device API server and codec, the Noise handshake reader, BLE setup, the update manifest and writer) carry bounded [proptest](https://proptest-rs.github.io/proptest/) properties: no panic on any input, and encode-then-decode round trips. They run in the ordinary suite, so `make check` covers them; a failure records its seed, which is committed with the fix: for a test under `tests/` in `<name>.proptest-regressions` beside it, for one in a crate's `src/` in `proptest-regressions/<file>.txt` at the crate root (proptest's default persistence). The same drivers back the cargo-fuzz targets ([fuzzing](README.md#fuzzing)).

## Dependencies

`make check-deps` runs [cargo-deny](https://embarkstudios.github.io/cargo-deny/) against [deny.toml](../../deny.toml):

- **Licenses:** permissive licenses only (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0 and a few more), so every binary built from this MIT repository can ship. The one exception is the Unicorn emulator (GPL-2.0), which only the host tool `wellfed-emu` links and which never reaches firmware.
- **Advisories:** vulnerable, unmaintained and yanked crates fail. An ignored advisory carries its reason; today that is only `bare-metal`, archived but still a dependency of cortex-m 0.7.
- **Duplicates** are warned about, not refused: several come from crates partway through an upgrade (digest 0.10 and 0.11, rand_core, syn 2 and 3). A change that adds one says why.
- **Sources:** crates.io only.
- **First-party linkage:** the bench's hardware edge is banned outside the crates that need it, through `wrappers`: `wellfed-relay` only from `wellfed-bench` and `wellfed-itest`, `wellfed-flash` only from `wellfed-audit`, `wellfed-bench` and `wellfed-build`, and `wellfed-build` (which links the flash client) only from `wellfed-bench`. So the read-only page, `wellfed-dashboard`, cannot link either; adding one to it fails `check-deps`.

It then checks the out-of-workspace fuzz crate's lockfile against [src/fuzz/deny.toml](../../src/fuzz/deny.toml), the same rules with the allow list trimmed to that graph. Its one exception is libFuzzer (NCSA, a permissive license), which `libfuzzer-sys` builds into host fuzzing binaries only.

The advisory check reads the RustSec database, which the first run downloads. Offline, `cargo deny check --disable-fetch` uses the cached copy; a check that could not run is not a pass.

A new dependency is justified in the change that adds it (what it replaces or why the workspace can't do without it), uses a version from `[workspace.dependencies]` when one exists, and keeps builds `--locked`.

## Tooling

Rust 1.99.1 through rustup ([rust-toolchain.toml](../../rust-toolchain.toml)), plus `cargo install cargo-nextest cargo-deny` (and `cargo-fuzz` for the optional [fuzz targets](README.md#fuzzing)). Run `cargo fmt --all` after any edit, then `make check`.
