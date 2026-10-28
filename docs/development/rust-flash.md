# Rust flash client

`wellfed-flash` ([src/tools/wellfed-flash](../../src/tools/wellfed-flash)) reads and writes the feeder's flash over UART. It does bounded reads, region-scoped [development updates](uart-development.md), give-ways and original-stock recovery. This page is for developers at the [relay bench](../devices/plaf103/bench.md).

It is the only flash client. It keeps the pinned helper, ROM and baseline checks, marker-last sequencing and terminal no-retry stops. Its reports, plans and journals keep the fields the former Python clients wrote, so evidence those clients saved still reads.

A passing check here does not authorize a device session. Any session needs a reviewed bench procedure.

## Build it

1. Run `cargo build --locked -p wellfed-flash`.

   You should see `target/debug/wellfed-flash`. Add `--release` for `target/release/`.

2. Run it from the repository root, or pass `--root` to name the checkout whose ignored `private/` holds the evidence. All inputs and outputs must resolve inside that checkout.

3. Run `wellfed-flash <command> --help` to list a command's arguments.

## Commands

| Command | Does |
| --- | --- |
| `wellfed-flash read observe\|inspect-rom\|small\|full\|status` | Restricted reads that never program or erase flash |
| `wellfed-flash update inspect\|prepare\|execute` | A region-scoped [development update](uart-development.md) of bank 1 (default) or bank 0. Flags: `--bank`, `--interrupted`, `--restore-marker`, `--original-read` |
| `wellfed-flash give-way inspect\|prepare\|execute` | Clears bank 1's first marker byte so the loader falls back to bank 0 |
| `wellfed-flash stock-recovery prepare\|execute` | Exact original bank-1 stock recovery from a full read |
| `wellfed-flash plan-stock-recovery` | Offline: plans stock recovery sectors from a baseline and a current read |
| `wellfed-flash decode-status` | Offline: decodes GD25Q64E status registers and assesses a write range |
| `wellfed-flash inspect-journal` | Offline: checks a transaction journal's hash chain; never resumes it |

A bounded `read small` uploads the pinned helper to a loader you have already entered. Later sessions continue that helper session.

The relay bench, [`wellfed-bench`](../devices/plaf103/bench.md), runs this client as a separate process. That keeps every plan bound to the `wellfed-flash` binary itself.

## How a write works

Every write goes through the same three steps.

1. Run the command's `inspect`. It reads the device and writes nothing. Stock recovery has no `inspect`: it prepares from a full read.

2. Run its `prepare`. It opens no port. It writes the plan and prints the plan's SHA-256.

3. Check the plan: the bank, the reviews it names, and the sectors it touches. A plan binds the SHA-256 of the binary that prepared it, so a rebuilt binary must prepare afresh.

> [!WARNING]
> `execute` erases and programs flash. Make a full backup first, and run it only against the unit and plan you just checked.

4. Run its `execute` with exactly that plan and hash.

### Frozen reviews

Frozen image reviews come from `wellfed-build review` ([development updates](uart-development.md)). The reviewer re-runs the build's linked-code audits and the factory-loader emulation. This client only checks that a frozen review is complete and still matches its image. `wellfed-formats` types the review file for both.

The reviewer compared the build's SDK revision and toolchain digest with the lock of the checkout that froze it. The review binds them through the manifest's hash. Using a review reads nothing from the current checkout's sources or locks. So an SDK, toolchain or `Cargo.lock` bump never strands an older review, bank 0's recovery image among them. The image hashes and every writer check are unchanged.

## Output for scripts and agents

Progress goes to stderr. Stdout carries exactly one JSON object: `status`, the output path, and the result or the error.

| Exit code | Meaning |
| --- | --- |
| 0 | Complete: inspected, prepared, verified, or a clean offline result |
| 1 | Stopped after the port opened. Inspect the report and journal; nothing was retried |
| 2 | Refused before any serial access. No port opened, and no output directory was created unless the error says otherwise |

A command that opens the serial port first takes the [bench lease](../devices/plaf103/bench.md#bench-lease) under `--root`. When `wellfed-bench` started it, it runs on its parent's lease instead. Another holder, or a lease a killed run left behind, is a refusal (exit 2) that names the holder. The offline commands take no lease.

Ctrl-C stops a session terminally and records the stop. It is checked before every transmission, so no frame follows the interrupt. A final report that cannot be saved after the port opened is a stop (exit 1, `report_error`), never a refusal.

## Common mistakes

- **Running from a worktree whose `private/` is a symlink.** The evidence paths don't resolve inside the checkout there. Run the bench from a checkout whose `private/` is a real directory. The firmware build refuses there too, because the shared SDK tree records the main checkout's paths.

## Verification

- `cargo test -p wellfed-flash` runs every transaction against a modeled helper with a synthetic baseline, which only test builds accept. It covers wire order checked against an independently restated policy, sector scope, lost ACKs, the interrupted mode, both banks, give-way with restore, stock recovery and refusals.
- `tests/emulated.rs` runs in `make check-fixtures`. It serves the pinned vendor helper, executed under Unicorn by `wellfed-emu`, on a Unix socket (`WELLFED_FLASH_EMULATOR`). It drives the built binary with the real baseline, frozen reviews and session reports. It covers:
  - update and give-way with restore: exactly the target is written, and the journal verifies;
  - a status read, and a refused serial-evidence continuation;
  - stock recovery: a full 8 MiB read, a reviewed plan and its execution, after which bank 1 is the baseline's and every other byte is unchanged;
  - a refused second planned erase inside the emulated part. The session stops (exit 1), resends nothing, and its journal never claims verification.
- When `WELLFED_FLASH_EMULATOR` is set, reports record `"transport": "emulator"`. A session continues only evidence from the same transport. Reports from serial sessions, including the Python clients', count as serial.

Emulation runs the helper's command parser and packet checks as ARM code. It models the ROM UART and flash calls, so it does not establish physical write acceptance, timing or power-cut behavior.

## Next steps

- [Development updates](uart-development.md): the update path in full.
- [Recovery](../devices/plaf103/recovery.md): the pinned recovery profile.
- [Optional fixtures](fixtures.md): the emulated tests' inputs.

## Evidence

- **Observed, October 22, 2026, research unit, relay bench driving power and the loader:** `read small` and `read status` matched a Python session byte for byte (data, bytes sent and bytes received). A no-op `update` erased nothing and verified the region. `give-way`, then `update --restore-marker`, booted bank 0 and then bank 1. `update --bank 0` installed a changed image: 140 sectors, verified. Until then the fixtures also required the Rust plan and wire to equal the Python updater's, and those passed.
- **Observed, October 23, 2026:** stock recovery ran on hardware in Rust. Bank 1 was restored to stock, and stock 1.1.15 booted. That session skipped the final full read at the owner's request; the next update inspection matched bank 1 against the frozen stock review instead.
- **Modeled, October 23, 2026:** the emulated stock recovery and fault cases passed on the research baseline. This is simulation only, not a hardware restore.
