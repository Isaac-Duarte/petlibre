# Relay bench control

`wellfed-bench` ([crates/wellfed-bench](../../crates/wellfed-bench)) powers the feeder, enters the ROM loader and chains the [Rust UART flash client](../development/rust-flash.md). It adds power and strap control only. Serial reads, inspections, plans and writes run as separate `wellfed-flash` processes, so their pinned checks, plan hashes and no-retry stops apply unchanged. Running it is a physical bench operation; build and check targets never call it.

Build both binaries together; the bench runs the `wellfed-flash` beside itself unless `--flash-bin` names another:

```sh
cargo build --locked --release -p wellfed-bench -p wellfed-flash
target/release/wellfed-bench status
```

Run it from the repository root, or pass `--root`. A plan binds the SHA-256 of the `wellfed-flash` binary that prepared it, so do not rebuild between `flash-prepare` and `flash-execute`. The bench never links the flash client in, so rebuilding the bench alone does not affect a plan.

## Wiring

A DSD TECH SH-UR04A USB relay board (CP2102 serial, 9600 8N1) adds two switches to the [UART wiring](uart.md):

| Relay | Contacts | Connection | Role |
| --- | --- | --- | --- |
| 1 | COM1, NO1 | In series with USB 5V between two screw-terminal USB-A adapters; GND, D+ and D− pass straight through | Feeder power |
| 2 | COM2, NO2 | Feeder TX side of the 4.7 kΩ strap resistor, and GND | Replaces the loader-entry button |
| 3, 4 | — | Unused | Spare |

Normally-open contacts leave the feeder off and the strap released when the relay board loses USB. The board answers `AT` with `OK`; `AT+CHn=1`, `AT+CHn=0` and `AT+CHn=?` set and query channel *n*. Its front buttons toggle the same state, which queries report.

The relay board also enumerates as a USB serial port and can take the name the UART adapter used to have. Use `/dev/serial/by-id/` paths. They live in the ignored `private/bench.json` with the pinned baseline and ROM report:

```json
{
  "relay_port": "/dev/serial/by-id/<relay board>",
  "uart_port": "/dev/serial/by-id/<UART adapter>",
  "baseline": "private/<pinned full read>/flash-read.bin",
  "rom_report": "private/<ROM inspection>/report.json"
}
```

## Commands

| Command | Effect |
| --- | --- |
| `status` | Relay states and port presence |
| `on`, `off` | Power the feeder with the strap released, or release the strap and cut power |
| `capture NAME` | Receive-only recording of the running feeder; power is untouched |
| `boot NAME` | Power off, start a receive-only capture, confirm it holds the port, then power on |
| `loader NAME` | Same, with the strap held for 2 s across power-on; requires the download banner and a NAK run, otherwise powers off |
| `transients NAME` | Repeated logic-analyzer captures (`sigrok-cli`, fx2lafw) of relay power-ons or, with `--mode loader`, loader entries; each cycle arms the analyzer before power-on and ends powered off. No UART, no flash |
| `dump NAME` | `loader`, then `read small` and `read full` in one helper session |
| `flash-prepare NAME` | `loader`, `read small`, `update inspect` and `update prepare`; prints the plan summary and the exact `flash-execute` line, then stops |
| `flash-execute NAME` | Checks relay 1 stayed on and the strap is released, runs `update execute` with the reviewed hash, then `boot` |
| `give-way-prepare NAME`, `give-way-execute NAME` | The same pair for `give-way` (`--fallback` names bank 0's frozen review) |
| `ota NAME --review DIR`, `ota NAME --build` | With `--build`, first runs `build.py` and the review freeze. Signs the review with `wellfed-local ota-sign`, offers it with `ota-offer` to the server already running, takes receive-only UART captures until the new build confirms, falls back or `--minutes` (20) pass, then polls `wellfed-local status` until the server's health shows the build. Prints a timing summary. Never touches power or opens the relay board |
| `demo [PATTERN…]` | Clicks spare relays 3 and 4 (chase, blink, heartbeat, Morse); reads but never sets power or the strap; refuses while another bench command runs; at most 120 toggles, and both spares end off |

`wellfed-bench <command> --help` lists the flags. `flash-prepare`/`flash-execute` take the updater's `--bank`, `--source`, `--original-read`, `--target`, `--interrupted` and `--restore-marker`, and `--attended-motion` for a target whose manifest names a motion policy.

Session evidence goes to new directories under `private/captures/`, `private/flash-backups/` and `private/verification/`, named after `NAME`, which must be a single plain name. Existing paths are refused before any relay changes.

`flash-prepare` and `flash-execute` are separate on purpose: review the plan between them. A failed step stops without retry and leaves the feeder powered for inspection. A stopped update is never followed by a boot; finish it with a fresh `flash-prepare … --interrupted` session as the [development updater](../development/uart-development.md) describes. Relay state shows that the supply was not switched off between the two commands. It does not rule out a brownout.

Ctrl+C stops the running sequence: the `wellfed-flash` child stops itself terminally and records it, and the bench runs the same cleanup as for any stop (the loader and transient sequences power off, and the demo turns the spares off).

Stock recovery keeps its own commands. Use `loader` to enter the ROM, then run `wellfed-flash stock-recovery` directly.

## Over-the-air iteration (FW12)

`ota` runs one [over-the-air update](../firmware/ota-design.md) of the feeder that is already running: build → checks → upload → reboot → build identity and health. The server and the hotspot must already be up: the bench never starts `wellfed-local serve` or the hotspot, and a failed offer stops before any capture. A bank-0 change still needs the UART updater.

```sh
target/release/wellfed-bench ota fw12-unit-01 --build                     # build, review, offer, verify
target/release/wellfed-bench ota fw12-unit-02 --review private/verification/<review>
```

- **Build (`--build`):** runs two Python children from the root, their output streamed to the progress log. First `firmware/tools/build.py plaf103-firstboot`, then `tools/flash/review-development-image.py build --build private/firmware-build/plaf103-firstboot --output private/verification/NAME-review`. The run then continues as with `--review` on that directory.
  - The interpreter is `.venv/bin/python` unless `--python` names another.
  - `--variant fw11-reset|fw11-fail` adds `--trial-reset` or `--trial-fail`. `fw16-pulse` is refused: motion builds stay manual, then go through `--review` with `--attended-motion`.
  - A nonzero exit from either child stops without signing or offering.
  - Refused before anything runs: an existing `NAME-review`, a missing interpreter, `--variant` or `--python` without `--build`, and `--build` with `--review`.

- **Version:** `--version`, or one more than the highest version among the signed manifests in `private/verification/`. The manifest is written to `private/verification/NAME-vN.manifest`.
- **Build ID:** the review's `startup_identity`. A review whose `startup_motion_policy` is set needs `--attended-motion`, as for `flash-prepare`.
- **Tools:** `wellfed-local` runs beside the bench unless `--local-bin` names another; its output goes to the progress log.
- **Captures:** 60 s segments `private/captures/NAME-01`, `-02`, … A name with any numbered segment is refused before building or signing.

The `PL…` lines are read as each segment ends. Progress shows on stdout and the page: the give-way, the manifest, written sectors as a bar, activation, the new build's start and trial boots. The outcomes:

| Outcome | Lines | Exit |
| --- | --- | --- |
| Passed | `PLFW1 start build=<ID> bank=1`, then `PLCNF1 confirmed=1`, then the health check below | 0 |
| Health missing | confirmed on the UART, but the server's health never showed the build within 90 s | 1 |
| Fell back | `PLTRL1 give_way=1` after activation or the new build's start; reports the last `PLTRL1 confirmed=0 boot=N` | 1 |
| Update error | `PLOTA1 error stage=…` | 1 |
| Timeout | none of these within `--minutes` | 1 |

**Health check.** After the confirmation, `wellfed-local status --config private/server.json` runs every 5 s for up to 90 s; `--config` names another file. The firmware publishes health every 30 s. The check requires all of:
- `replacement.health.build` is the offered build
- `replacement.health.bank` is 1
- `replacement.online` is true
- `replacement.stale` is false

It reports the health's `seq`, `outputs`, `faults` and, when present, whether the `clock` is valid. A changed reason for waiting (an older build, stale health, the status call failing) is logged once.

**Timing.** Once any child has run, the command ends with a timing summary: build, review, sign + offer, give-way to activation (the download), activation to confirmation, the health check, and the total. This is the FW12 duration record. Build, review and offer times are measured by the bench. The two feeder phases come from each capture's `serial-events.jsonl` chunk times. Without that file they fall back to the end of the 60 s segment and say so.

The server serves each offer to completion once, so after a fallback, retry with a new version. Ctrl+C stops the child, the capture or the poll, and the command. The classifier, the health check and the sequence are host-tested with real log lines, status documents and fake tools (October 22, 2026); neither the command nor `--build` has yet run on hardware.

## Progress page

Each command except `status` records its steps, the flash client's progress lines (bytes read, sectors verified), relay changes and the active capture in `private/bench-status/`:
- `current.json`
- `current.log`: the command's full output
- one `history.jsonl` line per command, including the reviewed next command after a prepare
- `relays.json`: the last state each relay was set to or read as

The fields match what the Python bench wrote, so older history still shows.

`--ui` serves a read-only page of those files on `http://127.0.0.1:8780/`:
- `wellfed-bench --ui` alone runs until Ctrl+C.
- `wellfed-bench --ui flash-execute …` serves it while that command runs.

The page only reads the status files and evidence directories. It never opens a serial port, so it can run beside any command. Its assets are embedded in the binary. It binds to loopback by default because the page shows private UART output. `--listen ADDRESS` serves another interface, for example the Tailscale address to reach it from a phone. There is no access token: anyone who can reach that address can read the page, so name only a private interface such as Tailscale. Wildcard addresses are refused. Allowing that port through the host firewall is the owner's change. `--ui-port` changes the port.

The page shows:
- **Header:** relay and UART port presence, the last recorded relay states (not queried), the `wellfed-flash` hash, the build and bank from the newest replacement-firmware start banner in a capture, and the running command. It does not stay pinned while scrolling.
- **Now:** a summary line with the state, elapsed time and step count, then the steps and relays beside bars with straight-line ETAs for the tool's own counts, an update's estimated transfer and the active capture, plus the next command to run after review. The tab title carries the state.
- **Live log** and **Latest RX log** side by side at the same height. The live log streams the command's full output; you can filter it, pause following, or copy it. Stops, refusals, faults and NAKs are highlighted; the flash client's JSON summaries are coloured by their `error` and `status` fields, so a successful summary with `"error":null` reads as success.
- **Sessions:** the ten newest session names (**Show all** for the rest) with evidence parts, report state and age. Selecting one shows the plan (bank, sectors, erases, image hashes, plan SHA-256) and a copyable execute line above the list. A report still marked `running` whose command is no longer live reads as `ended` (grey, italic): the process stopped without recording an outcome.
- **Recent commands:** the newest eight (**Show all** for up to 15) with start time, duration and outcome.

For a timed operator run, an optional `private/bench-status/script.json` lists steps (`{"title", "capture", "anchor", "steps": [{"at", "text"}]}`). The page shows the current step large with a countdown, starting when the anchor bytes (for example `PLOPT1 open`) arrive in that capture. After **Enable sound** (browsers need a click), each step beeps (a double beep to move a probe, a long tone at the end), and **Speak steps** reads it aloud.

## Failure modes

| Failure | Effect | Handling |
| --- | --- | --- |
| Relay board loses USB or the host reboots | Normally-open contacts drop: feeder off, strap released | A running update stops like any other power cut; finish it with `--interrupted` |
| Brownout or supply dip while powered | Relay state still reads on | Not detectable here; the updater's readback and status checks are the guard |
| Strap contact fails at its small current (under 1 mA) | Normal boot instead of the loader | `loader` requires the banner and NAKs, otherwise powers off; nothing is retried |
| UART adapter back-powers the feeder through its TX line | Power-off may not fully reset the chip, so the strap is ignored | Seen intermittently with the manual button (cured by replugging the adapter), not in 32 relay trials; hold a UART break while off or switch adapter TXD with relay 3 if it returns |
| Relay port disappears mid-session | The current command fails; power stays as last set | Cut power by hand if needed, then check `status` |
| Board power also feeds the motor harness | The motor driver is powered whenever the chip is | Only the ROM and flash helper run while flashing; FW30 measures the driver inputs |

## Evidence

The Rust bench is host-tested only: fake relay board and flash client, the page's handlers in process, and the real binary against synthetic status files (October 22, 2026). Its sequences port the Python bench line for line. The physical evidence below was gathered with the Python `tools/bench/bench.py`, which it replaces; repeat `status`, `on`/`off`, `loader` and a no-op `flash-prepare`/`flash-execute` under a reviewed session before relying on it.

**Full live test (physical, October 22, 2026, Python bench).** Every command was exercised on the research unit, except `transients`, which needs the analyzer:
- **Commands:** `status`, `on`, `capture` (power untouched), `off`, `boot`, `loader` and `dump`. The dump was a full 8 MiB read, and comparing it with the previous one showed exactly the expected changes.
- **Updates:**
  - `flash-prepare` and `flash-execute`: a changed-image install of 142 sectors, finished with `--interrupted` after a loader NAK, then a verified boot.
  - `--bank 0`: an install of 145 sectors.
  - `give-way-prepare/give-way-execute` and `--restore-marker`: the FW09 sequence. Each step booted the expected bank.
- **Refusals,** none of which changed a relay or wrote flash:
  - a reused capture name (fixed during the test; it used to power-cycle first)
  - a motion image without `--attended-motion`
  - `flash-execute` without a prepared plan
  - `flash-execute` after the feeder was powered off since prepare
  - a wrong plan hash (refused before any serial write; the feeder stayed powered and was not booted)
- **Dashboard:** it read the live install's steps, sector count and transfer estimate.

Two loader NAKs occurred in this session. One came while `make check` ran beside an install; none came over three installs and four dumps with the host otherwise idle. Keep heavy host work away from updates.


Physical, October 2026 (Python bench): with the UART adapter connected throughout, relay-1 boots reached the application banner and strap-held boots printed the download banner and NAKs in 16 of 16 trials each. Off times were 0.3–2 s, with and without a UART break held while off. A `flash-prepare`/`flash-execute` pair with the installed image as both source and target ran end to end: no-op plan, one verified acquisition, no writes, then a captured application boot. The earlier symptom of loader entry failing until the adapter was replugged did not recur in these trials.
