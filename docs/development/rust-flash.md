# Rust UART flash client

`wellfed-flash` ([crates/wellfed-flash](../../crates/wellfed-flash)) is a Rust port of the UART clients in `tools/flash/`. It ships as a single binary with no Python or pyserial dependency. It keeps the Python clients' pinned helper, ROM and baseline checks, marker-last sequencing and terminal no-retry stops. Its reports, plans and journals use the same fields, so either implementation's later sessions accept the other's saved evidence.

**Evidence: host simulation only.** The Rust client has not run on the research unit. Until FW32 closes on the bench, the Python clients remain the accepted writers. A reviewed bench procedure is still required for any device session; a passing check here does not authorize one.

## Commands

Build with `cargo build --locked -p wellfed-flash`; the binary is `target/debug/wellfed-flash` (or `target/release/` with `--release`). Run it from the repository root, or pass `--root` to name the checkout whose ignored `private/` holds the evidence. All inputs and outputs must resolve inside it.

| Python client | Rust command |
| --- | --- |
| `uart-flash-read.py observe\|inspect-rom\|read-small\|read-full\|read-status` | `wellfed-flash read observe\|inspect-rom\|small\|full\|status` |
| `uart-development.py inspect\|prepare\|execute` (`--bank`, `--interrupted`, `--restore-marker`, `--original-read`) | `wellfed-flash update inspect\|prepare\|execute` (the same flags) |
| `uart-give-way.py inspect\|prepare\|execute` | `wellfed-flash give-way inspect\|prepare\|execute` |
| `uart-stock-recovery.py prepare\|execute` | `wellfed-flash stock-recovery prepare\|execute` |
| `plan-stock-recovery.py` | `wellfed-flash plan-stock-recovery` |
| `flash-status.py` | `wellfed-flash decode-status` |
| `recovery-transaction.py` `inspect_journal` | `wellfed-flash inspect-journal` |

`wellfed-flash <command> --help` lists each command's arguments. The device commands keep the Python sequence. A bounded `read small` uploads the pinned helper to a loader the operator has already entered. Later sessions continue that helper session. Every write is `inspect`, then `prepare` (no serial access, writes the plan and prints its SHA-256), then `execute` with exactly that plan and hash. A plan binds the SHA-256 of the binary that prepared it, so a rebuilt binary must prepare afresh, just as an edited Python client must.

Frozen image reviews still come from `tools/flash/review-development-image.py`, which re-runs the build's linked-code and factory-mapping audits. The Rust client only checks that a frozen review is complete and still matches its image.

Not ported: the stock write-back acceptance tests (`uart-stock-recovery.py prepare-activation-test`, `bank-writeback-test`) and the standalone diagnostic installer (`uart-standalone.py`). `tools/bench/bench.py` still chains the Python clients.

## Output for scripts and agents

Progress goes to stderr. Stdout carries exactly one JSON object: `status`, the output path, and the result or the error. Exit status:

| Code | Meaning |
| --- | --- |
| 0 | Complete (inspected, prepared, verified, or a clean offline result) |
| 1 | Stopped after the port opened: inspect the report and journal; nothing was retried |
| 2 | Refused before any serial access: no port opened, and no output directory was created unless the error says otherwise |

Ctrl-C stops a session terminally and records the stop, as the Python clients' `KeyboardInterrupt` handling does.

## Verification

- `cargo test -p wellfed-flash` runs every transaction against a modeled helper with a synthetic baseline, which only test builds accept. It covers wire order checked against an independently restated policy, sector scope, lost ACKs, the interrupted mode, both banks, give-way with restore, stock recovery and refusals.
- `tools/tests/test_flash_rust.py` checks status decoding and journal hashing against the Python modules. With fixtures (`make check-fixtures`), it serves the pinned vendor helper under unicorn on a Unix socket (`WELLFED_FLASH_EMULATOR`) and drives the binary through update and give-way/restore. The test requires the Rust plan to equal the Python planner's, the transmitted bytes to equal `development-wire-policy.py`, and the journal to pass the Python inspector, all using the real frozen reviews and session reports. When `WELLFED_FLASH_EMULATOR` is set, reports record `"transport": "emulator"`.

Emulation runs the helper's command parser and packet checks as ARM code. It models the ROM UART and flash calls, so it does not establish physical write acceptance, timing or power-cut behavior. Stock recovery is not exercised under emulation, because each verification reads the full 8 MiB.
