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
| `ota NAME --review DIR`, `ota NAME --build` | With `--build`, first runs `wellfed-build` and the review freeze. Signs the review with `wellfed-local ota-sign`, offers it with `ota-offer` to the server already running, takes receive-only UART captures until the new build confirms, falls back or `--minutes` (20) pass, then polls `wellfed-local status` until the server's health shows the build. Prints a timing summary. Never touches power or opens the relay board |
| `itest NAME` | Real-device integration scenarios ([below](#integration-suite)): runs its own hotspot, `serve` and receive-only UART capture, installs over the air, and restores the bench at the end. Opens the relay board only with `--power`, for cold boots |
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

- **Build (`--build`):** runs two children from the root, their output streamed to the progress log. First `wellfed-build plaf103-firstboot` (beside the bench binary, or `--build-bin`), then the Python review `tools/flash/review-development-image.py build --build private/firmware-build/plaf103-firstboot --output private/verification/NAME-review`. The run then continues as with `--review` on that directory.
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

## Integration suite

`itest` runs named real-device scenarios against the feeder on the bench. It is a bench command, not a test target: `cargo test` and `make check` never reach it. Builds go in over the air. The UART stays attached, receive-only, for the whole run. It is what the scenarios assert on, it is the evidence, and it is the way back when the feeder stops answering.

```sh
cargo build --release --bins --examples
target/release/wellfed-bench itest --list
target/release/wellfed-bench itest itest-20261022-01 --build --tier smoke    # a few minutes
target/release/wellfed-bench itest itest-20261022-02 --build --power         # standard (default tier)
target/release/wellfed-bench itest itest-20261022-03 --build --power --destructive --tier soak --soak-minutes 120
target/release/wellfed-bench itest itest-20261022-04 --review private/verification/<review> --only requests,request-auth
```

**Tiers.** `--tier smoke` runs `ota-update`, `requests` and `request-auth`, which takes minutes once the build under test is running. `standard` (the default) runs the whole functional suite. `soak` adds long runs. `--only` picks scenarios whatever their tier.

**Builds.** `--build` first asks `wellfed-build source-id --image` for the identity a build of these sources would embed. That identity covers every source input, the variant and the update key. If a complete review with that `startup_identity` is already in `private/verification`, the suite signs it rather than building again. `wellfed-bench ota --build` does the same, and `--rebuild` builds anyway. So after the first run on unchanged sources, neither the build under test nor the `fw11-fail` variant is rebuilt.

| Scenario | Checks | Needs |
| --- | --- | --- |
| `ota-update` | The build under test (`--build` or `--review`) installs over the air and confirms, unless health already shows it. It always runs first, and every other scenario needs it. | |
| `requests` | `ping` and `health` replies, with `PLMQT1 request op=ping ok=1`. The FW21 clock has synced and is within 5 s of the server. A lights change saves a newer generation (`PLCFG1 saved`) and reads back, then is restored. | |
| `request-auth` | Beside the bare broker, `request_forger` sends four requests. The unsigned `wifi` clear, the wrongly keyed request and the replay are all refused with `auth`. The valid one is answered. The UART shows at least three `op=unauthenticated ok=0`. `serve` comes back afterwards. | |
| `settings-persist` | Lights, sound and volume changed, then a cold boot, then read back; the originals are restored. | `--power` |
| `wifi-fallback` | Credentials stored for an absent SSID (`PLCFG1 net saved stored=1`). After a cold boot, health shows `wifi.source` `fallback`. A signed clear and another cold boot return it to `build`. It refuses to start while credentials are stored. | `--power` |
| `network-bounce` | The hotspot goes down for 20 s after `PLMQT1 closed`. It comes back up, then `PLMQT1 ready` and fresh health with more `transport.connects`. | |
| `trial-fallback` | The `fw11-fail` variant installs and fails its trial, and KM0 gives way. Bank 0's recovery role then installs the baseline, re-signed and offered once the variant activated. | `--build` |
| `ota-faults` | The variant's offer makes the confirmed build give way. `ota_fault_server` then serves the re-signed baseline with one fault per boot: corrupt → `stage=hash`, truncated → `range`, stalled → `timeout`, none of them activating. A clean download then confirms. | `--build --power` |
| `ota-power-cuts` | Installs the speaker variant (no motion). It has to be a build the feeder has not confirmed, because a confirmed one skips the trial. Two cuts follow. **Mid-download** (48 sectors in), the next boot must be bank 0 and its first progress record must show the download starting again. **10 s into trial boot 1**, before the 30 s confirmation, KM0 must count boot 1 again (backup registers clear on power loss), and the build must still confirm. A cut during activation itself is not attempted, since the marker write and reset take milliseconds. The re-signed baseline then goes back on. | `--build --power --destructive` |
| `soak` | Pings every 30 s for `--soak-minutes` (default 60). It fails on any restart (`PLFW1 start`), any reconnect (`transport.connects`) or any missed reply, and reports the slowest reply. | soak tier |

A scenario whose flag is missing is reported as **skipped**, never as passed. `--destructive` requires `--power`. The design recovers from both power cuts over the air (FW10, FW11); if it ever does not, recovery is the UART flash the suite prints.

**Device state.** Once the baseline confirms, the suite reads the settings, whether Wi-Fi credentials are stored, and the build and bank. After every later scenario it reads them again. A scenario that leaves any of them changed fails, even if its own checks passed. The suite puts back what a request can: settings at once, stored credentials from the next boot.

**Bench state.** Before changing anything, the suite refuses if any of these is wrong: the UART port is missing, `wellfed-local` or its examples are not built, an evidence name is in use, or anything already answers on the server's `http_listen` (stop that `serve`; the suite runs its own). It then:
- records whether the hotspot connection named in `private/hotspot.json` is active, and brings it up;
- with `--power`, records relay 1 and switches the supply on;
- starts the UART capture;
- starts `wellfed-local serve --config private/server.json` as a child.

Scenarios swap `serve` for `wellfed-local broker` plus `ota_fault_server` or `request_forger` and back again. All server output goes to `private/verification/NAME-servers.log`. At the end, or on an error or Ctrl+C, the suite stops its children and puts the hotspot and the supply back as it found them. Without `--power` the relay board is never opened. The suite never flashes over UART and never changes the firewall or routing. Motion builds are not offered.

**Failures.** A failed scenario stops the rest (they are recorded as skipped), unless `--keep-going` is given. Scenarios that change device state restore it on failure too, while the feeder still answers. If there is no `PL…` record and no fresh health for 180 s, the feeder counts as silent; every wait checks this, so an unpowered feeder stops an install watch within 3 minutes rather than at its 15-minute deadline. Only the wrong-credential wait in `wifi-fallback`, where the feeder is unreachable on purpose, is exempt. The suite then stops whatever `--keep-going` says, prints the last confirmed review and the `flash-prepare` command to recover it over UART, and exits 2. A scenario failure exits 1.

**Evidence.**
- **Captures:** receive-only, in back-to-back 180 s segments `private/captures/NAME-uart-01`, `-02`, … (the flash client's limit for one observe). A thread starts each segment as the last exits, so the port stays held while the suite blocks on a build or a child. Segments are read while they record, so a record arrives within about a second.
- **Reviews and manifests:** `NAME-good`, `NAME-fail`, `NAME-restore`, `NAME-trigger`, `NAME-fault`, `NAME-cut-speaker` and `NAME-cut-restore`, in `private/verification/`. Each signs the next version, so a run uses up several. A reused review keeps its original name.
- **Record:** `private/verification/NAME-itest.json` holds each scenario's verdict, reason and duration, any silence, and the capture list, marked `"evidence": "physical"`.

**Cold boots.** After a supply cut the server keeps reporting the old session's health as online and fresh until the broker's keepalive lapses, and a request sent then is lost. So after a cold boot the suite waits for `PLMQT1 ready` and accepts only health received after it.

**Status (October 22, 2026).** **Physical, scaled suite:**
- `itest-20261022-09 --build --power --destructive` passed the eight functional scenarios. It showed the build cache, the continuous capture and the device-state check after every scenario working on hardware.
- `ota-power-cuts` then failed: a reinstalled confirmed build gets no trial. It was rebuilt around the speaker variant, and `itest-20261022-10` passed it in 10 minutes (OTA v42–v44):
  - a cut at 48/171 sectors booted bank 0, and the download began again at 16/171;
  - a cut 10 s into trial boot 1 was counted as boot 1 again, and the build confirmed.
- Further harness fixes on hardware: refusal records trail their replies by about half a second, so the suite waits for them. A feeder the suite has just powered on is heard before it is offered its own build (`same=1`).

**Physical, first suite:** `itest-20261022-06 --build --power` passed all eight scenarios in 26 minutes (OTA v28–v32). The good build (`877b2c9b…`) confirmed. `request-auth` saw three refusals on the UART. The Wi-Fi fallback took about 2 minutes. `trial-fallback` counted trial boots 1–3, and KM0 gave way. `ota-faults` saw `stage=hash`, `range` and `timeout`, then a clean install. The bench state came back as found. Runs 01–05 found the harness bugs fixed above: no silence check during waits, the capture gap and offset during the forger, stale health after a cold boot. They also found that the `fw11-fail` variant no longer passed the link audit, because its constant failure let LTO drop the image's writers. **Host:** the scenarios, the UART log and the HTTP parsing run against a scripted feeder that emits the firmware's record formats and models the boot window.

## Progress page

Each command except `status` records its steps, the flash client's progress lines (bytes read, sectors verified), relay changes and the active capture in `private/bench-status/`:
- `current.json`
- `current.log`: the command's full output
- one `history.jsonl` line per command, including the reviewed next command after a prepare
- `relays.json`: the last state each relay was set to or read as

The fields match what the Python bench wrote, so older history still shows.

`--ui` serves a read-only page on `http://127.0.0.1:8780/`:
- `wellfed-bench --ui` alone runs until Ctrl+C.
- `wellfed-bench --ui flash-execute …` serves it while that command runs.

**Building it.** The page is optional. It is a Svelte app in `web/apps/bench`, which the binary embeds only when built with the `ui` feature:

```
make ui                                   # pnpm in web/: builds web/apps/bench/dist
cargo build --locked --release -p wellfed-bench --features ui
```

A binary built without the feature refuses `--ui` before any bench command runs. Node and pnpm are needed only for `make ui` and `make check-web`, never for `make check`. The page's TypeScript types (`web/packages/api/src/bench.gen.ts`) are generated from the Rust views, and a test in `make check` fails when they are stale (`UPDATE_BINDINGS=1 cargo test -p wellfed-bench bindings` rewrites them).

**What it reads.** The page reads only the status files, the evidence directories and the local server's status. It never opens a serial port, so it can run beside any command, and nothing it does writes. The feeder panel asks `wellfed-local serve` for `GET /api/status` on the address and token in `private/server.json`, every 5 seconds; that read changes nothing on the server or the feeder. When the server is down or not configured, the panel says so.

**Network.** The page binds to loopback by default because it shows private UART output. `--listen ADDRESS` serves another interface, for example the Tailscale address to reach it from a phone. There is no access token: anyone who can reach that address can read the page, so name only a private interface such as Tailscale. Wildcard addresses are refused. Allowing that port through the host firewall is the owner's change. `--ui-port` changes the port.

The page has four views (keys `g o`, `g t`, `g s`, `g l`; `/` filters the log):
- **Strip** (every view): relay and UART port presence, the last recorded relay states (not queried), the `wellfed-flash` hash, the build and bank from the newest replacement-firmware start banner in a capture, and the running command.
- **Overview:**
  - **Now:** the command, its outcome and elapsed time. Steps show their own durations. While the command runs, bars show straight-line ETAs for the tool's own counts, an update's estimated transfer and the active capture. The next command to run after review is shown with a copy button. The tab title carries the state.
  - **Feeder:** online, stale or offline; health age, build, bank, faults, Wi-Fi credential source, clock and transport reconnects; recent request replies.
  - **Latest integration run:** each scenario's verdict, or the live run's progress.
  - **Recent commands.**
- **Tests:**
  - **Run in progress:** each scenario with what it checks and its time. The running one shows its current wait and its notes from the log.
  - **Run history:** every `NAME-itest.json` against every scenario, newest on the left. A live run fills the first column. Each row has the pass rate over runs that reached a verdict, the median passing time and the last failure. **Flaky** marks two or more pass/fail changes in the last 20 runs. Selecting a run opens its record: verdicts and reasons, the UART recovery commands if the feeder went silent, and the tails of its captures and `NAME-servers.log`. A run's link (`#/tests/NAME`) can be shared.
  - **Start a run:** composes a `wellfed-bench itest` command from a tier or chosen scenarios and the allowed flags, and warns which scenarios would be skipped. The page only writes the command out; running it is a physical bench operation.
- **Sessions:** session names with evidence parts, report state and age. Selecting one shows each part's report or plan (bank, sectors, erases, image hashes, plan SHA-256) and a copyable execute line. A report still marked `running` whose command is no longer live reads as `ended`: the process stopped without recording an outcome.
- **Log:** the command's full output, kept to the newest 20,000 lines. It can be filtered, followed and copied, and only the visible lines are drawn. Stops, refusals, faults and NAKs are highlighted, and scenario starts mark sections. The flash client's JSON summaries are coloured by their `error` and `status` fields. Healthy health lines (`outputs stopped, faults none`) and file names such as `-fault-v52.manifest` are not taken for failures. The latest capture's tail sits beside the log.

Light and dark follow the system; the button in the header cycles auto, light and dark for this browser.

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

The Rust bench is host-tested only: fake relay board and flash client, the page's handlers in process, and the real binary against synthetic status files (October 22, 2026). The web page is host-tested too (October 22, 2026, simulation): unit tests for its log reading, run builder and formatting, and Playwright against the real binary serving a synthetic `private/` tree, on desktop and phone sizes in both themes. Its feeder panel has not yet read a live `wellfed-local serve`. Its sequences port the Python bench line for line. The physical evidence below was gathered with the Python `tools/bench/bench.py`, which it replaces; repeat `status`, `on`/`off`, `loader` and a no-op `flash-prepare`/`flash-execute` under a reviewed session before relying on it.

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
