# Rust UART flash client

`petlibre-flash` ([crates/petlibre-flash](../../crates/petlibre-flash)) is a Rust port of the UART clients in `tools/flash/`. It ships as a single binary with no Python or pyserial dependency. It keeps the Python clients' pinned helper, ROM and baseline checks, marker-last sequencing and terminal no-retry stops. Its reports, plans and journals use the same fields, so either implementation's later sessions accept the other's saved evidence.

**Evidence: physical (October 22, 2026) and host simulation.** On the research unit, after `tools/bench/bench.py loader`:
- `read small` and `read status` matched a Python session byte for byte: data, bytes sent and bytes received.
- A no-op `update` erased nothing and verified the region.
- `give-way`, then `update --restore-marker`, booted bank 0 and then bank 1.
- `update --bank 0` installed a changed image: 140 sectors, verified.

It is the accepted writer: the Python development updater and give-way client were removed on October 22, 2026, after these checks. The Python reader and stock-recovery client remain, also accepted. A reviewed bench procedure is still required for any device session; a passing check here does not authorize one.

Caution: run the bench from a checkout whose `private/` is a real directory. In a worktree where `private/` is a symlink, the evidence paths do not resolve inside the checkout, and the old Python bench's capture check refused there. The firmware build likewise refuses there, because the shared SDK tree records the main checkout's paths.

## Commands

Build with `cargo build --locked -p petlibre-flash`; the binary is `target/debug/petlibre-flash` (or `target/release/` with `--release`). Run it from the repository root, or pass `--root` to name the checkout whose ignored `private/` holds the evidence. All inputs and outputs must resolve inside it.

| Python client | Rust command |
| --- | --- |
| `uart-flash-read.py observe\|inspect-rom\|read-small\|read-full\|read-status` | `petlibre-flash read observe\|inspect-rom\|small\|full\|status` |
| `uart-development.py inspect\|prepare\|execute` (removed) (`--bank`, `--interrupted`, `--restore-marker`, `--original-read`) | `petlibre-flash update inspect\|prepare\|execute` (the same flags) |
| `uart-give-way.py inspect\|prepare\|execute` (removed) | `petlibre-flash give-way inspect\|prepare\|execute` |
| `uart-stock-recovery.py prepare\|execute` | `petlibre-flash stock-recovery prepare\|execute` |
| `plan-stock-recovery.py` | `petlibre-flash plan-stock-recovery` |
| `flash-status.py` | `petlibre-flash decode-status` |
| `recovery-transaction.py` `inspect_journal` | `petlibre-flash inspect-journal` |

`petlibre-flash <command> --help` lists each command's arguments. The device commands keep the Python sequence. A bounded `read small` uploads the pinned helper to a loader the operator has already entered. Later sessions continue that helper session. Every write is `inspect`, then `prepare` (no serial access, writes the plan and prints its SHA-256), then `execute` with exactly that plan and hash. A plan binds the SHA-256 of the binary that prepared it, so a rebuilt binary must prepare afresh, just as an edited Python client must.

Frozen image reviews still come from `tools/flash/review-development-image.py`, which re-runs the build's linked-code and factory-mapping audits. The Rust client only checks that a frozen review is complete and still matches its image.

Not ported: the stock write-back acceptance tests (`uart-stock-recovery.py prepare-activation-test`, `bank-writeback-test`); the standalone diagnostic installer was removed instead. The relay bench, [`petlibre-bench`](../hardware/bench.md), chains this client as a separate process, so plans stay bound to the `petlibre-flash` binary itself.

## Output for scripts and agents

Progress goes to stderr. Stdout carries exactly one JSON object: `status`, the output path, and the result or the error. Exit status:

| Code | Meaning |
| --- | --- |
| 0 | Complete (inspected, prepared, verified, or a clean offline result) |
| 1 | Stopped after the port opened: inspect the report and journal; nothing was retried |
| 2 | Refused before any serial access: no port opened, and no output directory was created unless the error says otherwise |

Ctrl-C stops a session terminally and records the stop, as the Python clients' `KeyboardInterrupt` handling does: it is checked before every transmission, so no frame follows the interrupt. A final report that cannot be saved after the port opened is a stop (exit 1, `report_error`), never a refusal.

## Verification

- `cargo test -p petlibre-flash` runs every transaction against a modeled helper with a synthetic baseline, which only test builds accept. It covers wire order checked against an independently restated policy, sector scope, lost ACKs, the interrupted mode, both banks, give-way with restore, stock recovery and refusals.
- `tools/tests/test_flash_rust.py` checks status decoding and journal hashing against the Python stock-recovery modules. With fixtures (`make check-fixtures`), it serves the pinned vendor helper under unicorn on a Unix socket (`PETLIBRE_FLASH_EMULATOR`) and drives the binary through update and give-way/restore with the real frozen reviews and session reports: exactly the target is written, and the journal passes the Python inspector. Until October 22, 2026 it also required the Rust plan and wire to equal the Python updater's; those passed, together with the FW32 hardware checks, and the Python updater (`uart-development.py`, `development-bank.py`, `development-wire-policy.py`) was then removed. When `PETLIBRE_FLASH_EMULATOR` is set, reports record `"transport": "emulator"`, and a session continues only evidence from the same transport (Python reports count as serial).

Emulation runs the helper's command parser and packet checks as ARM code. It models the ROM UART and flash calls, so it does not establish physical write acceptance, timing or power-cut behavior. Stock recovery is not exercised under emulation, because each verification reads the full 8 MiB.
