# Rust UART flash client

`wellfed-flash` ([src/tools/wellfed-flash](../../src/tools/wellfed-flash)) is the UART flash client: reads, region-scoped development updates, give-ways and original-stock recovery. It began as a port of the Python clients formerly in `src/tools/flash/` and keeps their pinned helper, ROM and baseline checks, marker-last sequencing and terminal no-retry stops. Its reports, plans and journals keep the same fields, so evidence those clients saved still reads.

**Evidence: physical (October 22, 2026) and host simulation.** On the research unit, after `src/tools/bench/bench.py loader`:
- `read small` and `read status` matched a Python session byte for byte: data, bytes sent and bytes received.
- A no-op `update` erased nothing and verified the region.
- `give-way`, then `update --restore-marker`, booted bank 0 and then bank 1.
- `update --bank 0` installed a changed image: 140 sectors, verified.

It is the only client. The Python development updater and give-way client were removed on October 22, 2026, after these checks, and the Python reader and stock-recovery client on October 23, 2026. **Stock recovery has not run on hardware in Rust.** The physical stock round trip was done with the Python client, and `wellfed-flash stock-recovery` has so far passed only under emulation (below). A reviewed bench procedure is still required for any device session; a passing check here does not authorize one.

Caution: run the bench from a checkout whose `private/` is a real directory. In a worktree where `private/` is a symlink, the evidence paths do not resolve inside the checkout, and the old Python bench's capture check refused there. The firmware build likewise refuses there, because the shared SDK tree records the main checkout's paths.

## Commands

Build with `cargo build --locked -p wellfed-flash`; the binary is `target/debug/wellfed-flash` (or `target/release/` with `--release`). Run it from the repository root, or pass `--root` to name the checkout whose ignored `private/` holds the evidence. All inputs and outputs must resolve inside it.

| Command | Former Python client (removed) |
| --- | --- |
| `wellfed-flash read observe\|inspect-rom\|small\|full\|status` | `uart-flash-read.py observe\|inspect-rom\|read-small\|read-full\|read-status` |
| `wellfed-flash update inspect\|prepare\|execute` (`--bank`, `--interrupted`, `--restore-marker`, `--original-read`) | `uart-development.py` (the same flags) |
| `wellfed-flash give-way inspect\|prepare\|execute` | `uart-give-way.py` |
| `wellfed-flash stock-recovery prepare\|execute` | `uart-stock-recovery.py prepare\|execute` |
| `wellfed-flash plan-stock-recovery` | `plan-stock-recovery.py` |
| `wellfed-flash decode-status` | `flash-status.py` |
| `wellfed-flash inspect-journal` | `inspect-recovery-transaction.py` |

`wellfed-flash <command> --help` lists each command's arguments. A bounded `read small` uploads the pinned helper to a loader the operator has already entered. Later sessions continue that helper session. Every write is `inspect`, then `prepare` (no serial access, writes the plan and prints its SHA-256), then `execute` with exactly that plan and hash. A plan binds the SHA-256 of the binary that prepared it, so a rebuilt binary must prepare afresh.

Frozen image reviews come from `wellfed-build review` ([development updates](uart-development.md)), which re-runs the build's linked-code audits and the factory-loader emulation. This client only checks that a frozen review is complete and still matches its image; `wellfed-formats` types the review file for both.

Not ported, and removed with the Python on October 23, 2026: the finished stock write-back acceptance tests (`uart-stock-recovery.py prepare-activation-test` and `bank-writeback-test`, with `marker-activation-test.py` and `stock-bank-wire-policy.py`). The standalone diagnostic installer was removed earlier. The relay bench, [`wellfed-bench`](../hardware/bench.md), chains this client as a separate process, so plans stay bound to the `wellfed-flash` binary itself.

## Output for scripts and agents

Progress goes to stderr. Stdout carries exactly one JSON object: `status`, the output path, and the result or the error. Exit status:

| Code | Meaning |
| --- | --- |
| 0 | Complete (inspected, prepared, verified, or a clean offline result) |
| 1 | Stopped after the port opened: inspect the report and journal; nothing was retried |
| 2 | Refused before any serial access: no port opened, and no output directory was created unless the error says otherwise |

Ctrl-C stops a session terminally and records the stop: it is checked before every transmission, so no frame follows the interrupt. A final report that cannot be saved after the port opened is a stop (exit 1, `report_error`), never a refusal.

## Verification

- `cargo test -p wellfed-flash` runs every transaction against a modeled helper with a synthetic baseline, which only test builds accept. It covers wire order checked against an independently restated policy, sector scope, lost ACKs, the interrupted mode, both banks, give-way with restore, stock recovery and refusals.
- `tests/emulated.rs` (fixtures, in `make check-fixtures`) serves the pinned vendor helper, executed under Unicorn by `wellfed-emu`, on a Unix socket (`WELLFED_FLASH_EMULATOR`) and drives the built binary with the real baseline, frozen reviews and session reports. It covers update and give-way/restore (exactly the target is written, and the journal verifies), a status read, a refused serial-evidence continuation, and stock recovery: a full 8 MiB read, a reviewed plan, and its execution, after which bank 1 is the baseline's and every other byte is unchanged. A last case refuses the second planned erase inside the emulated part. The session then stops (exit 1), resends nothing, and its journal never claims verification. When `WELLFED_FLASH_EMULATOR` is set, reports record `"transport": "emulator"`, and a session continues only evidence from the same transport (reports from serial sessions, including the Python clients', count as serial).
- Until October 22, 2026 the fixtures also required the Rust plan and wire to equal the Python updater's; those passed, together with the FW32 hardware checks.

Emulation runs the helper's command parser and packet checks as ARM code. It models the ROM UART and flash calls, so it does not establish physical write acceptance, timing or power-cut behavior. **Emulation evidence, October 23, 2026:** the emulated stock recovery and fault cases passed on the research baseline. This is simulation only, not a hardware restore.
