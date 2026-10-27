# Relay bench control

`wellfed-bench` ([src/tools/wellfed-bench](../../../src/tools/wellfed-bench)) powers the feeder, enters the ROM loader and chains the [Rust UART flash client](../../development/rust-flash.md). It adds power and strap control only. Serial reads, inspections, plans and writes run as separate `wellfed-flash` processes, so their pinned checks, plan hashes and no-retry stops apply unchanged. Running it is a physical bench operation; build and check targets never call it.

Build the binaries together; the bench runs the `wellfed-flash` beside itself unless `--flash-bin` names another, and its `itest` and `stock-ota` run the stock server, the `wellfed-stock` beside it unless `--stock-bin` names another ([stock server](../../development/stock-server.md)):

```sh
cargo build --locked --release -p wellfed-bench -p wellfed-flash -p wellfed-stock
target/release/wellfed-bench status
```

The bench is five crates with one-way dependencies: `wellfed-relay` (the relay board driver), `wellfed-rig` (the operator sequences and the steps the suite builds on), `wellfed-itest` (the integration suite, on the rig), `wellfed-dashboard` (the read-only page, which links neither the relay driver nor the flash client) and `wellfed-bench` (the command line over all of them, one binary). The files they share (the bench config, the progress document, the run records and the suite's catalog) are typed in `wellfed-formats`. The rig signs offers through `wellfed-signing`; every build runs as a `wellfed-build` process and every flash read and write as a `wellfed-flash` process.

Run it from the repository root, or pass `--root`. A plan binds the SHA-256 of the `wellfed-flash` binary that prepared it, so do not rebuild between `flash-prepare` and `flash-execute`. The bench never links the flash client in, so rebuilding the bench alone does not affect a plan.

## Wiring

A DSD TECH SH-UR04A USB relay board (CP2102 serial, 9600 8N1) adds four switches to the [UART wiring](uart.md) ([diagram](../../../src/tools/wellfed-bench/wiring.svg), with reference designators and net names for a bench PCB):

| Relay | Contacts | Connection | Role |
| --- | --- | --- | --- |
| 1 | COM1, NO1 | In series with USB 5V between two screw-terminal USB-A adapters; GND, D+ and D− pass straight through | Feeder power |
| 2 | COM2, NO2 | Feeder TX side of the 4.7 kΩ strap resistor, and GND | Replaces the loader-entry button |
| 3 | COM3, NO3 | In series with adapter TXD → feeder RX | UART into the feeder |
| 4 | COM4, NO4 | In series with feeder TX → adapter RXD, on the adapter side of the strap | UART out of the feeder |

Normally-open contacts leave the feeder off and the strap released when the relay board loses USB. A powered adapter holds its TXD and RXD lines high, which can back-power a feeder that is off through its UART pins, so relays 3 and 4 isolate them: every power cut opens them after the supply, and every power-on closes them just before it. `capture` connects them only on a powered feeder. `--uart-when-off connected` leaves them closed through the cuts, as the bench was before they were wired, for comparison. GND stays connected. `ota`, and `itest` without `--power`, leave the relay board alone, so they need a feeder the bench powered (which connected the UART). The board answers `AT` with `OK`; `AT+CHn=1`, `AT+CHn=0` and `AT+CHn=?` set and query channel *n*. Its front buttons toggle the same state, which queries report.

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
| `status` | The [bench lease](#bench-lease), relay states and port presence; `--break-lease` first frees a lease a killed run left |
| `on`, `off` | Power the feeder with the strap released, or release the strap and cut power |
| `capture NAME` | Receive-only recording of the running feeder; power is untouched |
| `boot NAME` | Power off, start a receive-only capture, confirm it holds the port, then power on |
| `loader NAME` | Same, with the strap held for 2 s across power-on; requires the download banner and a NAK run, otherwise powers off |
| `transients NAME` | Repeated logic-analyzer captures (`sigrok-cli`, fx2lafw) of relay power-ons or, with `--mode loader`, loader entries; each cycle arms the analyzer before power-on and ends powered off. No UART, no flash |
| `dump NAME` | `loader`, then `read small` and `read full` in one helper session |
| `flash-prepare NAME` | `loader`, `read small`, `update inspect` and `update prepare`; prints the plan summary and the exact `flash-execute` line, then stops |
| `flash-execute NAME` | Checks relay 1 stayed on and the strap is released, runs `update execute` with the reviewed hash, then `boot` |
| `give-way-prepare NAME`, `give-way-execute NAME` | The same pair for `give-way` (`--fallback` names bank 0's frozen review) |
| `ota NAME --review DIR`, `ota NAME --build` | With `--build`, first runs `wellfed-build` and the review freeze. Signs the review in process with `wellfed-build`'s signing library and `private/ota-key.json` (verifying the review as `wellfed-flash` does: a directory with no `review.json`, an image or manifest that differs from it, a non-pinned "stock" image or an unattended motion policy is refused), signs a version 2 manifest and pushes it over the device API as `--keys` (the offer, bank 1's give-way, the offer again to bank 0, the chunks, and a resume after any drop), takes receive-only UART captures until the new build confirms, falls back or `--minutes` (20) pass, then polls the device API until its status shows the build on bank 1. Stock gets a downgrade statement for the feeder's identity and floor; `--downgrade` signs one for a version at or below the floor. `--fault corrupt@N`, `unwanted@N`, `stall@N` or `drop@N` spoils the bench's own transfer, and `--fault give-way-only` stops once bank 1 gave way (to offer bank 0 another manifest next). `ota NAME --cancel` sends only a `CancelUpdate`: it forgets the candidate a give-way or a stopped install left recorded, without which bank 0 refuses any other manifest (`Pending`); run it between failure drills. Prints a timing summary. Never touches power or opens the relay board |
| `itest NAME` | Real-device integration scenarios ([below](#integration-suite)): runs its own hotspot, `serve` and receive-only UART capture, installs over the air, and restores the bench at the end. Opens the relay board only with `--power`, for cold boots |

`wellfed-bench <command> --help` lists the flags. `flash-prepare`/`flash-execute` take the updater's `--bank`, `--source`, `--original-read`, `--target`, `--interrupted` and `--restore-marker`, and `--attended-motion` for a target whose manifest names a motion policy.

Session evidence goes to new directories under `private/captures/`, `private/flash-backups/` and `private/verification/`, named after `NAME`, which must be a single plain name. Existing paths are refused before any relay changes.

`flash-prepare` and `flash-execute` are separate on purpose: review the plan between them. A failed step stops without retry and leaves the feeder powered for inspection. A stopped update is never followed by a boot; finish it with a fresh `flash-prepare … --interrupted` session as the [development updater](../../development/uart-development.md) describes. Relay state shows that the supply was not switched off between the two commands. It does not rule out a brownout.

Ctrl+C stops the running sequence: the `wellfed-flash` child stops itself terminally and records it, and the bench runs the same cleanup as for any stop (the loader and transient sequences power off, and the demo turns the spares off).

Stock recovery keeps its own commands. Use `loader` to enter the ROM, then run `wellfed-flash stock-recovery` directly.

## Bench lease

One tool drives the bench at a time. Every `wellfed-bench` command except `status` (and `itest --list`), every `wellfed-flash` command that opens the serial port (`read`, `update`, `give-way`, `stock-recovery`), `wellfed-stock setup-send`, and `wellfed-stock serve --enable-ota-trial` for as long as it runs take the lease `private/bench.lease` before they touch anything, and remove it when they end, on a stop or Ctrl+C as on success. It names the holder: its pid, its start time and boot (so a reused pid is not mistaken for it), its host, its command line and when it started. A second tool is refused with that holder named, before it opens a port or switches a relay:

```
Stopped: The bench is held by pid 81234 (`wellfed-bench dump fw-01`, since 2026-10-26 12:00:00 UTC on bench); wait for it to finish or stop it
```

**Chained tools.** A process descended from the holder runs on the holder's lease and leaves it in place, so the children the bench starts (`wellfed-flash` captures, reads and updates; `itest` and `stock-ota`'s `serve --enable-ota-trial`) never wait on their own parent. That is checked once, when the child starts; the bench waits for or stops its children before it releases. Run by hand, `wellfed-flash` takes the lease itself, so a hand-run read cannot open the UART under a running bench command. The progress page, alone or as `--ui`, only reads and takes none; with a command, the command holds it. The offline `wellfed-flash` commands (`plan-stock-recovery`, `decode-status`, `inspect-journal`) and an emulated helper (`WELLFED_FLASH_EMULATOR`, the host tests) take none.

**The stock server.** `wellfed-stock` keeps the lease beside its `--config`, which on the bench is `private/`. A trial-enabled `serve` holds the bench for its whole life, because the install it dispatches runs for minutes after the request: `wellfed-bench off` or `dump` from another terminal is refused until the server stops. `ota-trial` takes no lease of its own: it only asks that server, which already holds the bench, so it works from any shell. A plain `serve`, `broker` and `ota-images` take none: they only answer the feeder, beside the bench command that drives it (`ota` beside an operator's `serve`). `network apply` and `network remove` take none either: they change the host's routing, not the bench, and they run under sudo, where a lease would leave a root-owned `private/bench.lease.lock` that every later run as the operator could not open. If one ever does (`Permission denied` on that file), remove it with sudo; it holds nothing.

**A killed run.** A run killed outright (SIGKILL, a closed terminal) cannot remove its lease, and may have left relays on, a capture running or an update half done. The next tool refuses rather than take the bench over:

```
Stopped: The bench lease of pid 81234 (…) outlived its process: that run was killed and may have left relays on or an update half done. Check `wellfed-bench status`, then free the bench with `wellfed-bench status --break-lease`
```

`status` shows the lease first (free, held, or left by a process that is gone) and then the relays, without taking it. While a holder keeps the relay port open (a `capture` does for its whole run), `status` shows the lease and then stops with "Serial port: Device or resource busy" instead of the relays (physical, October 26). `status --break-lease` removes a lease only when its holder is gone, then shows the relays as usual; a live holder's lease, one written on another host, and a file that is not a lease are never broken (remove the last two by hand once the bench is idle). Against a file that is not a lease, `--break-lease` stops before the relays; plain `status` still shows them. A child the killed bench had started (`wellfed-flash`, `wellfed-stock serve`) may still be running on its own; it no longer counts as the bench's, so look for it (`pgrep -a wellfed-`) before breaking.

Taking, breaking and releasing run under an advisory lock on `private/bench.lease.lock`, which the kernel drops with the process, so two tools racing for a free or stale lease cannot both win. The lease itself is replaced atomically, so `status` never reads it torn. The lease and its rules are `wellfed_formats::bench::lease`.

## Over-the-air iteration (FW12)

`ota` runs one [over-the-air update](../../firmware/ota-design.md#updates-over-the-device-api) of the feeder that is already running: build → checks → push over the device API → reboot → build identity and status. The hotspot must already be up: the bench never starts it, and a push that cannot start stops before any capture. A bank-0 change still needs the UART updater.

```sh
target/release/wellfed-bench ota fw12-unit-01 --build                     # build, review, offer, verify
target/release/wellfed-bench ota fw12-unit-02 --review private/verification/<review>
target/release/wellfed-bench ota stock-01 --review private/verification/<stock review>   # back to stock (FW41)
```

**Back to stock over the air (FW41, host-tested).** A review whose `review.json` says `image_kind: stock` installs through the same signed update: the bench signs the stock payload with its `payload_sha256` as the build ID, the running replacement gives way, and bank 0's updater writes and verifies bank 1 before the marker, as for any build. The run passes on stock's `app_main:version:` banner after activation; there is no health step, since stock does not report to this server. Stock has no trial, so a stock image that failed to boot would need the UART [stock recovery](recovery.md). The integration suite refuses stock reviews. The UART route reads the whole flash three times and took about 65 minutes on October 23; this route writes only bank 1 and should take a few minutes.

- **Build (`--build`):** runs two children from the root, their output streamed to the progress log. First `wellfed-build build --profile firstboot` (beside the bench binary, or `--build-bin`), then the review `wellfed-build review build --build private/firmware-build/plaf103/firstboot --output private/verification/NAME-review`. The run then continues as with `--review` on that directory.
  - `--firmware qa|optics|buttons|adc|speaker|blink|trial|feeder` picks the build (default `qa`), and `--variant` picks a variant of that firmware: `stall|fail` with `--firmware trial`, `entropy|owner-stall|held-stall|gap-probe|feed-stall|stop-fail|owner-fail|twin` with `--firmware feeder` (`feed-stall` and `stop-fail` move the motor only for a feed request, like the feeder, and inject a fault into it: [motor pulse](../../firmware/motor-pulse.md#a-stalled-loop-and-a-failed-stop-during-a-feed); `owner-fail` never opens its feeding owner, so its trial must fall back, and `owner-stall` resets about 3 minutes into every boot once confirmed, a reset storm: [OTA design](../../firmware/ota-design.md#reset-storms)). The bring-up areas and the trial images are variants of one `bringup` firmware, so `--firmware optics` builds `wellfed-build --firmware bringup --variant optics` (likewise `buttons`, `adc` and `speaker`), `--firmware trial --variant stall` builds `--firmware bringup --variant stall`, and `--firmware trial` alone its normal image; `qa`, `blink` and `feeder` pass through unchanged. The motor pulse (`--firmware motor`, `--variant pulse`) is refused: motion builds stay manual, then go through `--review` with `--attended-motion`.
  - A nonzero exit from either child stops without signing or offering.
  - Refused before anything runs: an existing `NAME-review`, `--firmware` or `--variant` without `--build`, `--variant` without the firmware that declares it, and `--build` with `--review`.

- **Version:** `--version`, or one more than the highest version among the signed manifests in `private/verification/`. The manifest is written to `private/verification/NAME-vN.manifest`.
- **Build ID:** the review's `startup_identity`. A review whose `startup_motion_policy` is set needs `--attended-motion`, as for `flash-prepare`.
- **Tools:** no server runs: the bench signs by linking `wellfed-build` and offers over the device API. `--build` runs the `wellfed-build` beside it unless `--build-bin` names another; its output goes to the progress log.
- **Captures:** 60 s segments `private/captures/NAME-01`, `-02`, … A name with any numbered segment is refused before building or signing.

The `PL…` lines are read as each segment ends. Progress shows on stdout and the page: the give-way, the manifest, written sectors as a bar, activation, the new build's start and trial boots. The outcomes:

| Outcome | Lines | Exit |
| --- | --- | --- |
| Passed | `PLFW1 start build=<ID> bank=1`, then `PLCNF1 confirmed=1`, then the health check below | 0 |
| Health missing | confirmed on the UART, but the server's health never showed the build within 90 s | 1 |
| Fell back | `PLTRL1 give_way=1` after activation or the new build's start; reports the last `PLTRL1 confirmed=0 boot=N` | 1 |
| Update error | `PLOTA1 error stage=…` | 1 |
| Refused | the push ended before any UART outcome: the feeder refused the offer (a downgrade without a statement, a manifest bank 0 does not install, another signer) or reported a stop over the API | 1 |
| Timeout | none of these within `--minutes` | 1 |

**Status check.** After the confirmation, the device API's `Status` is read as `--keys` every 5 s for up to 90 s; the check requires the offered build on bank 1. It reports the uptime, the boot count, the Wi-Fi source and whether the clock is valid. A changed reason for waiting (an older build, another bank, the request failing) is logged once.

**Timing.** Once any child has run, the command ends with a timing summary: build, review, sign + offer, give-way to activation (the download), activation to confirmation, the health check, and the total. This is the FW12 duration record. Build, review and offer times are measured by the bench. The two feeder phases come from each capture's `serial-events.jsonl` chunk times. Without that file they fall back to the end of the 60 s segment and say so.

After a fallback, bank 0 refuses the manifest that failed its trial: sign a new version. Ctrl+C stops the child, the capture or the poll, and the command. The classifier, the health check and the sequence are host-tested with real log lines, status documents and fake tools (October 22, 2026); neither the command nor `--build` has yet run on hardware.

## Integration suite

`itest` runs named real-device scenarios against the feeder on the bench. It is a bench command, not a test target: `cargo test` and `make check` never reach it. Builds go in over the air. The UART stays attached, receive-only, for the whole run. It is what the scenarios assert on, it is the evidence, and it is the way back when the feeder stops answering.

The suite is the `wellfed-itest` crate, which `wellfed-bench` links: `itest` and `stock-ota` run the scenarios on the rig in the bench's own process, so the run, its interrupt and its exit status are the command's. `--list` answers before anything opens. The scenario catalog the page and `--list` show is typed in `wellfed-formats`.

```sh
cargo build --release --bins --examples
target/release/wellfed-bench itest --list
target/release/wellfed-bench itest itest-20261022-01 --build --tier smoke    # a few minutes
target/release/wellfed-bench itest itest-20261022-02 --build --power         # standard (default tier)
target/release/wellfed-bench itest itest-20261022-03 --build --power --destructive --tier soak --soak-minutes 120
target/release/wellfed-bench itest itest-20261022-04 --review private/verification/<review> --only requests,request-auth
target/release/wellfed-bench itest itest-20261025-01 --review private/verification/<review> --only feed-writes --feeds
target/release/wellfed-bench itest itest-20261027-01 --build --power --destructive --feeds --only sustained   # the 8-hour run
```

**Tiers.** `--tier smoke` runs `ota-update`, `requests` and `request-auth`, which takes minutes once the build under test is running. `standard` (the default) runs the whole functional suite. `soak` adds the long runs, `soak` and `sustained`. `--only` picks scenarios whatever their tier; the [sustained run](#sustained-run) is meant to run alone.

**Builds.** `--build` first asks `wellfed-build source-id --image` for the identity a build of these sources would embed. That identity covers every source input, the firmware, the variant and the update key. If a complete review with that `startup_identity` is already in `private/verification`, the suite signs it rather than building again. `wellfed-bench ota --build` does the same, and `--rebuild` builds anyway. So after the first run on unchanged sources, neither the build under test nor the trial firmware's `fail` image is rebuilt.

| Scenario | Checks | Needs |
| --- | --- | --- |
| `ota-update` | The build under test (`--build`, which builds the `feeder` firmware, or `--review`) installs over the air and confirms, unless the device API already shows it on bank 1. It always runs first, then syncs the feeder's clock, and every other scenario needs it. | |
| `requests` | Over the device API (FW47): status, with `PLAPI1 request … op=status`; a clock sync, after which the feeder's clock is within 5 s of the host's; a lights change against the generation just read saves a newer one (`PLCFG1 saved`), reads back, and is restored. | |
| `request-auth` | A session with an unenrolled key is refused (`OpenRefusal::Unknown`, `reason=unknown-key`); a handshake with its last byte flipped is dropped (`reason=noise`); the owner's session still works. | |
| `settings-persist` | Lights, sound and volume changed against the base generation, then a write from the stale generation must be refused `Stale` and change nothing; a cold boot, then read back; the originals are restored. | `--power` |
| `wifi-fallback` | Credentials stored for an absent SSID (`PLCFG1 net saved stored=1`). After a cold boot, `Status.wifi` shows `Fallback`. A clear (against the Wi-Fi generation) and another cold boot return it to `Build`. It refuses to start while credentials are stored. | `--power` |
| `network-bounce` | The hotspot goes down for 20 s and comes back. The device API answers again with the same boot count and more Wi-Fi losses (association, lease or gateway). | |
| `feed-writes` | Two owner sessions: the suite's own and a second held open from before the feeds, since the feeder admits no handshake while a feed holds its outputs. A 2-portion feed on the first; once the motor drives (`PLOWN1 state=4`), the held session sends a lights change against the current generation (it must be refused `Busy`), a lock change and a clock sync, and their answers are logged. No settings-writer save (`PLCFG1 saved`, `net saved`, `keys change`) or confirmation may appear between `PLOWN1 feed id=N` and `PLOWN1 fed id=N`; the feed must end `outcome=1` (Done) with `completed` = `requested`, and the refused change must have changed nothing. After the feed the same change saves, and is restored with the lock. A second feed is cancelled from the held session 2 s into its drive: the cancel answers `Done`, the owner logs its outputs stopped, and the feed ends `outcome=2 reason=2` (Cancelled). The lock change and the clock sync must answer `Done`, the lock must read the new value after the feed, and when the feeder logs a `PLOWN1 interlock` record after the feed began its `writes_deferred` must have grown. No `PLFW1 start` or panic and no boot count change throughout: the event ring logs nothing per append, so a ring write during the feed shows as the reset its failed flash lease causes. Feeds and cancels are sent once, never resent on a fresh session (the feeder remembers a write's outcome only once its answer resolves), and a failed one is treated as uncertain. After any failure the suite waits for a feed it started to end (`fed id=N`, then `state=1`) before restoring anything; if it does not end, nothing is restored and the run stops, whatever `--keep-going` says. The feeds run unattended, so the hopper may be empty. | `--feeds` |
| `ota-recovery` | The trial firmware's `fail` image installs and fails its trial, and KM0 gives way to bank 0, which answers the device API. The suite then pushes the re-signed baseline to it with one fault each, in its own transfer: a flipped byte → `stage=hash`, a chunk ahead of the wanted ones → `order`, 75 s of silence → `timeout`, none of them activating. Bank 0 keeps answering after each stop. A clean push then confirms and restores the baseline, so the fallback needs no restore install of its own. | `--build --power` |
| `ota-power-cuts` | Installs the feeder's `twin` variant (the same firmware under another build identity, so it still serves the device API). It has to be a build the feeder has not confirmed, because a confirmed one skips the trial. Two cuts follow. **Mid-download** (48 sectors in), the next boot must be bank 0, and when the push offers again its first progress record must show the download starting again. **10 s into trial boot 1**, before the 30 s confirmation, KM0 must count boot 1 again (backup registers clear on power loss), and the build must still confirm. A cut during activation itself is not attempted, since the marker write and reset take milliseconds. The re-signed baseline then goes back on. | `--build --power --destructive` |
| `soak` | A device API status every 30 s for `--soak-minutes` (default 60). It fails on any restart, any Wi-Fi loss or any missed answer, and reports the slowest. | soak tier |
| `sustained` | `--sustained-cycles` (default 16) half-hour cycles of feeds every 10 minutes, each cycle opening with one fault from a fixed rotation; any failed check cuts relay 1 and halts. Defined [below](#sustained-run). | soak tier, `--build --power --destructive --feeds` |

A scenario whose flag is missing is reported as **skipped**, never as passed. `--destructive` requires `--power`. The design recovers from both power cuts over the air (FW10, FW11); if it ever does not, recovery is the UART flash the suite prints.

**Device state.** Once the baseline confirms, the suite reads the settings, whether Wi-Fi credentials are stored, and the build and bank. After every later scenario it reads them again. A scenario that leaves any of them changed fails, even if its own checks passed. The suite puts back what a request can: settings at once, stored credentials from the next boot.

**Bench state.** Before changing anything, the suite refuses if any of these is wrong: the UART port is missing, `wellfed-stock` is not built, an evidence name is in use, or anything already answers on the server's `http_listen` (stop that `serve`; the suite runs its own). It then:
- records whether the hotspot connection named in `private/hotspot.json` is active, and brings it up;
- with `--power`, records relay 1 and switches the supply on;
- starts the UART capture;
- starts `wellfed-stock serve --config private/server.json` as a child.

Updates go over the device API, pushed on a thread of their own while the scenario reads the UART; `serve` stays up for stock's broker. The suite talks to the feeder over the device API as the key in `--keys` (default `private/wellfed-owner.json`), found over mDNS or, while the bench host's firewall drops UDP 5353, through the hotspot's neighbour table and `target_mac`. All server output goes to `private/verification/NAME-servers.log`. At the end, or on an error or Ctrl+C, the suite stops its children and puts the hotspot and the supply back as it found them, except after a `sustained` halt, which leaves the supply off. Without `--power` the relay board is never opened. The suite never flashes over UART and never changes the firewall or routing. Startup-motion builds are not offered. Only `feed-writes` and `sustained`, under `--feeds`, ask the feeder to feed (the owner approved unattended feeds on October 25, 2026, and set their limits on October 26: hopper empty, D cells out, at most 3 portions a feed); `sustained` also installs the `feed-jam`, `sensor-fault` and `feed-stall` variants, which move the motor only for a feed request.

**Failures.** A failed scenario stops the rest (they are recorded as skipped), unless `--keep-going` is given. Scenarios that change device state restore it on failure too, while the feeder still answers. If there is no `PL…` record, no device API answer and no fresh bank-0 health for 180 s, the feeder counts as silent; every wait checks this, so an unpowered feeder stops an install watch within 3 minutes rather than at its 15-minute deadline. Only the wrong-credential wait in `wifi-fallback`, where the feeder is unreachable on purpose, is exempt. The suite then stops whatever `--keep-going` says, prints the last confirmed review and the `flash-prepare` command to recover it over UART, and exits 2. A scenario failure exits 1.

**Evidence.**
- **Captures:** receive-only, in back-to-back 180 s segments `private/captures/NAME-uart-01`, `-02`, … (the flash client's limit for one observe). A thread starts each segment as the last exits, so the port stays held while the suite blocks on a build or a child. Segments are read while they record, so a record arrives within about a second.
- **Reviews and manifests:** `NAME-good`, `NAME-fail`, `NAME-restore`, `NAME-cut-speaker` and `NAME-cut-restore`, in `private/verification/`. Each signs the next version, so a run uses up several. A reused review keeps its original name.
- **Record:** `private/verification/NAME-itest.json` holds each scenario's verdict, reason and duration, any silence, and the capture list, marked `"evidence": "physical"`.

**Cold boots.** After a supply cut the server keeps reporting the old session's health as online and fresh until the broker's keepalive lapses, and a request sent then is lost. So after a cold boot the suite waits for `PLMQT1 ready` and accepts only health received after it.

**Status (October 22, 2026).** **Physical, faster suite:** `trial-fallback` and `ota-faults` are now one scenario, `ota-recovery`, and bank 0's recovery role pipelines its download. `itest-20261022-18 --build --power` passed the standard tier in 13.6 minutes (`-14`: 27.8):
- `ota-update` took 164 s;
- `ota-recovery` took 336 s, against 961 s for the two scenarios it replaces;
- each download took 18–19 s.

The runs below used the two separate scenarios.

**Physical, scaled suite:**
- `itest-20261022-09 --build --power --destructive` passed the eight functional scenarios. It showed the build cache, the continuous capture and the device-state check after every scenario working on hardware.
- `ota-power-cuts` then failed: a reinstalled confirmed build gets no trial. It was rebuilt around the speaker variant, and `itest-20261022-10` passed it in 10 minutes (OTA v42–v44):
  - a cut at 48/171 sectors booted bank 0, and the download began again at 16/171;
  - a cut 10 s into trial boot 1 was counted as boot 1 again, and the build confirmed.
- Further harness fixes on hardware: refusal records trail their replies by about half a second, so the suite waits for them. A feeder the suite has just powered on is heard before it is offered its own build (`same=1`).

**Physical, first suite:** `itest-20261022-06 --build --power` passed all eight scenarios in 26 minutes (OTA v28–v32). The good build (`877b2c9b…`) confirmed. `request-auth` saw three refusals on the UART. The Wi-Fi fallback took about 2 minutes. `trial-fallback` counted trial boots 1–3, and KM0 gave way. `ota-faults` saw `stage=hash`, `range` and `timeout`, then a clean install. The bench state came back as found. Runs 01–05 found the harness bugs fixed above: no silence check during waits, the capture gap and offset during the forger, stale health after a cold boot. They also found that the `fail` variant no longer passed the link audit, because its constant failure let LTO drop the image's writers. **Host:** the scenarios, the UART log and the HTTP parsing run against a scripted feeder that emits the firmware's record formats and models the boot window.

### Sustained run

FW27's defined sustained run is the `sustained` scenario (`src/tools/wellfed-itest/src/sustained.rs`). This definition is static (the code and this text); the scenario passes against the scripted feeder (simulation, October 26), and has not yet run on hardware.

```sh
target/release/wellfed-bench itest itest-YYYYMMDD-NN --build --power --destructive --feeds --only sustained \
  | tee private/verification/itest-YYYYMMDD-NN-console.log
# a shakedown of every fault once (4 h): add --sustained-cycles 8
```

**Before it starts (the owner's unattended motion rules, October 26):** hopper empty, the D cells out so relay 1 is the kill switch, nobody at the bench, and the bench lease free. The build under test is this checkout's `feeder` (`--build`); the run also builds the `feed-jam`, `sensor-fault`, `feed-stall` and `twin` variants once and re-signs them later. Each install and each restore signs one manifest version: 8 per rotation of eight cycles, about 17 for the full run with the baseline's. The logic analyzer may stay clipped on; it plays no part.

**Length and cadence.** 16 cycles of at least 30 minutes, about 8 hours: a step that outlasts its half hour (the update cuts can) stretches its cycle, and each cycle's note gives its real length. Each cycle first runs its fault step, then feeds the baseline at once and again every 10 minutes while the cycle lasts (at least one feed a cycle), with a device API status every 30 s in between. Baseline feeds ask for 1, 2, 3, 1, … portions; no feed in the run asks for more than 3. By the simulation's timings that is about 70 feeds (36 in eight cycles), fewer on hardware, where an install takes about 4 minutes rather than 2.5; at about 9 s a portion that is under 25 minutes of drive in 8 hours.

**The rotation.** Cycle n takes step n mod 8, so the full run takes each twice:

| Cycle | Step | What it injects | How it must end |
| --- | --- | --- | --- |
| 1, 9 | `feed-writes` | The [scenario](#integration-suite) as it stands: settings, lock and clock writes from a held session during a 2-portion feed, then a cancel 2 s into the next | Its own checks; its feeds join the run's journal |
| 2, 10 | Reboot | A `Reboot` request at rest | One restart into the baseline; reset cause `watchdog` (the firmware restarts by letting its watchdog lapse; FW54 read it so on October 26, physical); the clock still valid (FW46's warm carry); no drive after the boot |
| 3, 11 | Cut while reserved | A 3-portion feed, relay 1 cut as soon as the `Feeding` answer arrives, which the feeder sends only once the owner has reserved the feed in its journal (static), before the drive starts | The next boot logs `fed id=N outcome=5` (uncertain) once; reset cause `power_on`; no drive after the boot; the run syncs the clock again |
| 4, 12 | `feed-jam` | Installs the variant (a trial, then confirmation), feeds 1, 2 and 3 portions, then puts the baseline back | 1: Done with 1 portion (one reversal); 2: Failed, `Overcurrent`; 3: Failed, `Sensor`; the owner Idle after each |
| 5, 13 | Cut mid-drive | A 3-portion feed, relay 1 cut 4 s into its drive | As the reserved cut |
| 6, 14 | `sensor-fault` | Installs the variant, feeds 1, 2 and 3 portions, asks for one more, reboots, puts the baseline back | 1: Failed, `FeedbackTimeout`; 2: Failed, `Outlet`, no drive; 3: Failed, `Sensor`, the owner then in `Fault`; the extra feed refused `Fault`; the reboot clears the hold |
| 7, 15 | Outage mid-feed | The hotspot down for 60 s from 4 s into a 3-portion drive | The feed ends Done with 3 portions on the UART; no restart; the link losses grew; the device API answers again within 180 s |
| 8, 16 | `feed-stall`, then `ota-power-cuts` | Installs the variant: a 1-portion feed (600 ms loop stall) and a 2-portion feed (8 s stall); then the two update power cuts, mid-download and early in the twin's trial; the baseline back | 1: Failed, `Output`, then one restart; 2: one watchdog restart and `outcome=5` at the next boot; then `ota-power-cuts`' own checks |

**Pass criteria.** The run passes only if all 16 cycles pass every check. Each check reads evidence an existing tool or record already produces:

| Criterion | Evidence | Check |
| --- | --- | --- |
| No feed sent twice, none run twice | The suite's own requests (`Ask::Feed` is never resent) and the `PLOWN1 feed id=` and `fed id=` records | Each feed id the run was given has exactly one owner record and exactly one result, before or after a restart; no result names an id the run never asked for; ids only grow |
| Every feed ends as its step says | `PLOWN1 fed id=N … outcome= reason= completed=` | The outcomes in the table above; baseline feeds Done with every portion |
| Uncertain feeds are resolved, not retried | `fed id=N outcome=5` after the restart, and no `state=4` | A cut or a stall's reset leaves exactly one uncertain result and no drive in the boot that follows |
| Counters agree with the journal | `Stats { Counters }` (FW54) | This boot's `feeds`, `feeds_failed` and `portions` equal the sum of this boot's `fed` records on the UART; FW62's lifetime counts never go back |
| No unexpected restart, panic or fault | `PLFW1 start`, `PLPNC1`, `PLTRL1 storm=` and `give_way=1` on the UART; the status `boot_count` | None at rest or during a feed; each planned restart shows exactly one start record, and a warm one moves the boot count by one (it lives in a backup register a power cut clears); `Stats { Counters }.reset` names the planned cause; no `PLPNC1` anywhere in the run |
| No motion at rest | `PLOWN1 state=4` | None outside a feed the run asked for |
| Memory | `Stats { Memory }` (FW54), after each baseline feed | Least free heap at least 64 KiB (provisional: half the 141,792 B FW44 measured, static); free heap within 8 KiB of the first reading in the same boot of the same build |
| Clock | `GetTime` (FW46), after each feed | When valid: error bound at most 120 s, and the host's time within that bound plus 2 s; valid after a reboot; an invalid clock (after a cut, or an expiry) is synced and counted |
| Updates install and confirm during the run | The install watch (`PLOTA1`, `PLTRL1`, `PLCNF1`) and the status build | Each variant and each restore of the baseline ends `Passed`; `ota-power-cuts` passes |
| The feeder stays reachable and as it was | Status every 30 s; the device state check | Every status answered, no Wi-Fi loss at rest, and after every step the baseline on bank 1 with the run's settings and Wi-Fi source |

Each cycle ends with a note of the figures so far (feeds, polls and the slowest answer, least free heap, the largest clock skew and bound, clock syncs); with the UART captures and `NAME-itest.json` they are the run's evidence.

**Stop conditions.** Any failed check, a feed whose result does not arrive within 100 s, a silent feeder (180 s), an install that does not confirm, and the operator's Ctrl+C all halt the run the same way: the suite cuts relay 1 and leaves it off, runs no further cycle or scenario, puts the hotspot back as it found it, and records why. A host panic does the same: until the run ends cleanly, tearing the rig down cuts relay 1. Nothing is restored on the feeder, so a halt during a `feed-jam`, `sensor-fault` or `feed-stall` step may leave that variant on bank 1 (the halt note says so): install the baseline before any other run. A person looks before power returns. The [UART recovery](#integration-suite) applies if the feeder then does not boot.

**Kept out of the unattended run (the owner's).** Real food: portion weights, food through the outlet beam and FW18's weighing session. Real jams with food in the rotor, taped or blocked sensors, a full hopper. Batteries: the D cells, adapter changes, brownout and the low-voltage floor (FW26), so the run has no low-power step. Button presses (Dispense during a feed) and LED observation (FW23). The `stop-fail` variant's 8 s held stop and `owner-stall` stay out too: the first leaves the motor driving until the watchdog, and both ran on October 26 (FW16, physical). DHCP-server loss without an access-point loss, and lease expiry, need a host network change, which the authorization does not cover.

## Stock and back

Three bench routes cover stock 1.1.15:
- to stock over the air (FW41);
- to stock over UART (FW38, as `stock-prepare` and `stock-execute`);
- from stock to a replacement over stock's own updater (FW40, `stock-ota`).

`itest`'s `stock-roundtrip` scenario chains the over-the-air ones. The design for doing all of this without a bench is in [consumer installation](../../firmware/consumer-installation.md).

### To stock over the air

```sh
target/release/wellfed-bench ota NAME --review <frozen stock review>
```

The bench signs the pinned stock payload with a downgrade statement for the feeder's identity and floor and pushes it: the running build gives way, and bank 0 writes stock into bank 1. The command passes on stock's `app_main:version:` banner and skips the health step, since stock does not report to this server. Stock's own KM0 then runs, so our trial counter never counts it. If stock failed to boot from bank 1, the only way back would be UART. Status: physical (FW41, October 23, 2026): the first step of `fw41-roundtrip-01` (336 sectors written by the recovery role, then stock 1.1.15 on bank 1).

### To stock over UART

The FW38 procedure as two reviewed commands, shaped like `flash-prepare` and `flash-execute`:

```sh
# Bank 1 still holds a valid image: give way to bank 0 first.
target/release/wellfed-bench stock-prepare NAME --fallback <bank-0 review>
target/release/wellfed-bench stock-execute NAME --fallback <bank-0 review> --expected-plan-sha256 <sha>
# Bank 1 invalid (a give-way, or just after a stock-ota install): recover.
target/release/wellfed-bench stock-prepare NAME
target/release/wellfed-bench stock-execute NAME --expected-plan-sha256 <sha>
```

**`stock-prepare NAME`:**
1. loader;
2. `read small`, then `read full`, in one helper session;
3. a refusal if bank 1's marker is still valid, since `stock-recovery` refuses a valid target marker;
4. `wellfed-flash stock-recovery prepare`, which writes the plan from the pinned baseline backup.

It prints the next command with the plan's hash. With `--fallback`, both commands instead run the give-way to that bank-0 review, under the name `NAME-giveway`, and point back at `stock-prepare`.

**`stock-execute NAME`:**
- single-use, and it requires unbroken power since the prepare;
- runs `stock-recovery execute` with the plan hash; it reads all 8 MiB three times, about an hour;
- boots, and must see stock's banner.

The boot capture holds the unit's serial and cloud keys, so keep it private. The bench reads the plan through `wellfed-formats`' `StockRecoveryReviewed`. Status: host-tested; the same `wellfed-flash` steps ran physically in FW38.

### Stock to a replacement (`stock-ota`)

`stock-ota` has a feeder running stock 1.1.15 install a replacement build through stock's own MQTT-triggered HTTP updater, with no UART write ([stock OTA requirements](stock/ota-requirements.md)). It assumes stock is installed, and proves it first: the server must see stock report 1.1.15 on its adapter, which is also the server's own gate for the request.

```sh
cargo build --release --bins --examples          # from a clean commit: --enable-ota-trial refuses a dirty build
target/release/wellfed-bench stock-ota NAME --power                     # default firmware (qa)
sudo target/release/wellfed-stock network apply --config private/server.json   # once it reaches stock-ready
sudo target/release/wellfed-stock network remove                               # afterwards
```

**Routing is the operator's.** Stock connects to its vendor broker on TCP 1883. Only `network apply`, a host firewall change, redirects the feeder's traffic to this server, and only the operator runs it and removes it.
- **Image port:** `network apply` also accepts the image server's TCP 8088 from the feeder's MAC. Without that rule, stock connects for about 25 s and then reports `OTA_INFORM` error 2003 (`fw40-stock-ota-02`).
- **Timing:** `network apply` refuses until a broker listens, so run it once the run reaches `stock-ready`. Without it stock never reports, and the first stage fails after 5 minutes with nothing sent.

**Stages:**
1. **Build:** before the bench is touched, the firmware is staged as `ota --build` does: build, review, sign the next version. Stock ignores the manifest; the promotion uses it.
2. **`stock-ready`:** the bench comes up as in `itest`, with `serve --enable-ota-trial`. The stage then waits for fresh stock telemetry with `softwareVersion` 1.1.15 and `powerType` 1.
3. **`stock-settle`:** an earlier request that failed or is uncertain blocks a new one. The run asks the server every 30 s to resolve it, for up to 25 minutes.
   - **The server's evidence:** 20 minutes since that request, 10 quiet connected minutes, and a fresh 1.1.15 version report.
   - **Version report:** idle stock reports its version only around a connect, so a refused resolve makes the server send stock a read-only `ATTR_GET_SERVICE`.
   - **Heartbeat:** stock heartbeats about every 61 s, which the server's 90 s telemetry bound tolerates (`fw40-stock-ota-03`/`-04`).
4. **`stock-request`:** `wellfed-stock ota-images` serves the review's `image.bin`, bound to its `ota_sha256`, to the feeder found by its MAC. Then one `POST /api/ota/trial`. The server persists the request, then sends stock one `OTA_UPGRADE` with:
   - target version `2.0.0`;
   - the image's SHA-256;
   - a URL of about 80 characters, under stock's 128-byte parser buffer.

   It is never resent.
5. **`stock-install`:** stock's `OTA_PROGRESS` and `OTA_UPGRADE` reports are printed as they arrive, and a reported error fails the stage. Stock writes bank 0 and invalidates bank 1, so the stage passes only when the first `PLFW1 start` afterwards is the build, from **bank 0**.
6. **`bank0-health`:** the build answers the device API from bank 0 while it waits for an update. Stock reports nothing after activating it and the build speaks no MQTT, so the run then tells the server what it saw (`POST /api/ota/trial/answered`: the build identity and bank 0 the device API reported). The server checks that against the build it served and closes the request as `replacement_healthy` once stock's telemetry has gone stale (90 s), so the run asks every 10 s for 120 s; the next request then needs no 20-minute resolution first. A server that will not close it (stock still reporting, or its version reported after the request) is noted, not failed: the next request then waits for the resolution instead. Unknown on hardware: whether stock posts its `softwareVersion` between taking the command and rebooting into the build. If it does, every install fails this fast path closed and waits for the resolution.
7. **`promote`:** the same build is pushed, signed, over the device API. Bank 0 installs it into bank 1, and the trial boot confirms. The feeder ends in the normal two-bank state, with bank 0 holding the same build. `--no-promote` stops after `bank0-health`.

**After it:**
- The run restores the hotspot and the supply as it found them.
- It writes `private/verification/NAME-itest.json` with the stage verdicts, so the page's run history shows it. Server output goes to `NAME-servers.log`.
- If anything failed after the request was sent, the run prints the UART recovery pointer: bank 1 may be invalid.

**Status:**
- **Physical (October 23, 2026):**
  - `fw40-stock-ota-05`: stock installed a `qa` build in about 40 s, and it booted from bank 0.
  - `fw40-stock-ota-06-promote`: an `itest --only ota-update` offer moved that build into bank 1, where it confirmed.
  - Those runs found the image-port rule, the heartbeat bound and the stale version report fixed above. They also showed the old `bank0-health` could never pass: the recovery role reported no health.
  - `fw40-stock-ota-07`: the whole sequence with a fresh `qa` build. Stock installed it in 40 s, the recovery role reported health from bank 0, and the promotion (v84) confirmed with optics passing on the trial boot.

### `itest` scenario `stock-roundtrip`

```sh
target/release/wellfed-bench itest NAME --review <baseline review> --only stock-roundtrip --power --stock <frozen stock review>
```

From the confirmed baseline:
1. **To stock:** the stock review, offered over the air, installs into bank 1, and its banner must appear.
2. **On stock:** the server switches to `serve --enable-ota-trial`. The scenario waits for stock to report, so run `network apply` then, and resolves any earlier request.
3. **Back:** stock is asked to install the baseline, which starts on bank 0 and answers the device API there (closing the server's request as `replacement_healthy`), then is promoted into bank 1. It confirms at once, since it was confirmed before.
4. **Restore:** back to plain `serve`, with the baseline's device-API status fresh.

`--stock` requires `--power`. A failure on stock stops the suite with a stock-aware recovery hint. Status: physical (October 23, 2026): `fw41-roundtrip-01` passed in 6 min 31 s from `qa` v84; the promotion (v86) confirmed at once. With the owner's `network apply` rules already in place, nobody had to touch the bench.

## Progress page

Each command except `status` records its steps, the flash client's progress lines (bytes read, sectors verified), relay changes and the active capture in `private/bench-status/`:
- `current.json`
- `current.log`: the command's full output
- one `history.jsonl` line per command, including the reviewed next command after a prepare
- `relays.json`: the last state each relay was set to or read as

The fields match what the Python bench wrote, so older history still shows.

`--ui` serves a read-only page on `http://127.0.0.1:8780/`:
- `wellfed-bench --ui` alone runs until Ctrl+C, as does `wellfed-dashboard` (what `src/scripts/bench-dashboard` builds and runs, with the same `--listen` and `--ui-port`).
- `wellfed-bench --ui flash-execute …` serves it while that command runs, `itest` included.

**Building it.** The page is optional. It is a Svelte app in `src/web/apps/bench`, which the binary embeds only when built with the `ui` feature:

```
make ui                                   # pnpm in src/web/: builds src/web/apps/bench/dist
cargo build --locked --release -p wellfed-bench -p wellfed-dashboard --features ui
```

A binary built without the feature refuses `--ui` before any bench command runs. Node and pnpm are needed only for `make ui` and `make check-web`, never for `make check`. The page's TypeScript types (`src/web/packages/api/src/bench.gen.ts`) are generated from the Rust views, and a test in `make check` fails when they are stale (`UPDATE_BINDINGS=1 cargo test -p wellfed-dashboard bindings` rewrites them).

**What it reads.** The page reads only the status files, the evidence directories and the stock server's status. It links no relay or flash code and never opens a serial port, so it can run beside any command, and nothing it does writes. The feeder panel asks `wellfed-stock serve` for `GET /api/status` on the address and token in `private/server.json`, every 5 seconds; that read changes nothing on the server or the feeder. When the server is down or not configured, the panel says so.

**Network.** The page binds to loopback by default because it shows private UART output. `--listen ADDRESS` serves another interface, for example the Tailscale address to reach it from a phone. There is no access token: anyone who can reach that address can read the page, so prefer a private interface such as Tailscale. `--listen 0.0.0.0` (or `::`) serves every interface, including the feeder hotspot and any container bridges; the owner chose to allow it (October 25). Allowing that port through the host firewall is the owner's change. `--ui-port` changes the port.

The page has four views (keys `g o`, `g t`, `g s`, `g l`; `/` filters the log):
- **Strip** (every view): relay and UART port presence, the last recorded relay states (not queried), the `wellfed-flash` hash, the build and bank from the newest replacement-firmware start banner in a capture, and the running command.
- **Overview:**
  - **Guided QA** (when the latest capture holds `PLQA1` records): coaches the newest [QA](../../firmware/qa.md) run from its records. It shows what to do now in plain words ("Look at the Wi-Fi light. Lit: press Dispense. Dark: press Lock."), the time left on a button prompt, each check's state with the answers recorded so far, and the result. **Enable sound** beeps at each new prompt. It follows the records only; `wellfed-audit qa` still grades the run. The records are parsed by `wellfed_formats::qa`, which the audit shares.
  - **Now:** the command, its outcome and elapsed time. Steps show their own durations. While the command runs, bars show straight-line ETAs for the tool's own counts, an update's estimated transfer and the active capture. The next command to run after review is shown with a copy button. The tab title carries the state.
  - **Feeder:** stock as `wellfed-stock` last heard it: online or not, its reported version, and whether the server holds its broker session. The replacement firmware speaks no MQTT, so it shows nowhere here.
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

The Rust bench is host-tested only: fake relay board and flash client, the page's handlers in process, and the real binary against synthetic status files (October 22, 2026). The web page is host-tested too (October 22, 2026, simulation): unit tests for its log reading, run builder and formatting, and Playwright against the real binary serving a synthetic `private/` tree, on desktop and phone sizes in both themes. Its feeder panel has not yet read a live `wellfed-stock serve`. Its sequences port the Python bench line for line. The physical evidence below was gathered with the Python `src/tools/bench/bench.py`, which it replaces; repeat `status`, `on`/`off`, `loader` and a no-op `flash-prepare`/`flash-execute` under a reviewed session before relying on it.

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
