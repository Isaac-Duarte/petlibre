# Resume firmware development

Handoff checkpoint: **October 27, 2026 (morning), Mountain time**, during the coordinated board run (rules at the top of FIRMWARE_BOARD.md). Read this, then [AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md), and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) (stock parity: [FEATURE_BOARD.md](FEATURE_BOARD.md)).

This file is the handoff, not the history. Keep it to the current state, today's work, what is open, and what a new session must know before touching the bench. When a day ends, fold its entries into the boards' evidence columns and the topic docs, and cut them here; the full log up to October 25 is `git show 756f68f:RESUME.md`.

## Checkpoint

Reconfirm with the owner before any bench operation; this is the last recorded state, not a live observation.

- **Bank 1:** v244 `feeder` `807f7283…` (review `fw46-bank0-20261026-01`, the FW46 SysTimer clock), **confirmed** (October 27, physical, `fw62-v244-20261027-01`), installed through bank 0 after the FW61 storm run and a UART give-way (`fw62-giveway-20261027-01`, `recorded=1`), so bank 0's update record names this build. v243 (same build) first carried FW46's activation check (`fw46-v243-20261027-01`: trial clock 121 ms from the host); v242 was refused `update-fallback` before bank 0 had booted and never installed.
- **Bank 0:** `feeder` from review `fw46-bank0-20261026-01` (review `8abe508a…`, startup identity `807f7283…`), the FW46 SysTimer clock with carry record version 2; its clock sources match master's after `81ac4093`. Written over UART: two attempts on the night of October 26 (`fw46-bank0-20261026-01`, `-02`) stopped on loader `0x15` after 38 sectors; `fw46-bank0-20261027-01` (`--interrupted`, plan `d47bdc36…`, analyzer off USB) staged the remaining 208 sectors, verified the region (`468841c1…`) and committed the marker (October 27, physical). Booted by the give-way that followed (`PLOTA1 bank0 bytes=1019904 recorded=1`). The earlier bank 0 (`bfbfaebe…` from `3d028994`, `fw19-bank0-20261026-04`) is the source of that update; its pre-write full-flash backup is `fw19-bank0-20261026-backup` in the local evidence index.
- **Next manifest version:** 245 (242–244 used October 27). **Feeder routing:** `network apply` rules in place (owner, October 25; nft table `wellfed_local`, removed with `sudo target/release/wellfed-stock network remove`). **Feed journal:** high water 58 (feeds 54–58 FW18 phase cases, hopper empty).

- **Client store:** one owner, the bench CLI's key (`private/wellfed-owner.json`).
- **Bench:** feeder **off** (all four relays off, October 27 about 17:17 UTC) so the owner can re-clip the logic analyzer on U7 (the clips were pulled); hotspot up (feeder time setting `gateway`), no server, no capture, lease free. Host firewall `computer-security` stopped by the owner on October 26 (no inbound filter, no forward block); restart it with `sudo systemctl start computer-security`. Hopper empty, D cells out (as recorded October 26; not re-observed).
- **Next:** with the analyzer re-clipped: FW55 traces against the UART and analyzer, FW30's 50 boots of the latch fix, FW46's spoof check; then FW27's sustained run (`--only sustained`, 8 h). FW61 halted needs a variant that resets while held. Owner sessions: FW18 weighing, FW19 real jams, FW23 LEDs, FW42/FW43 stock round trip, FW26 supply, FW30's meter reading.
- **A power cut during a feed is safe since v172 (AR24):** the next boot resolves the feed as uncertain and the owner opens; the feed is never re-run.

## Today (October 25): FW16 feeding through the owner

All physical, owner present, attended motion. Uncommitted unless marked.

1. **Owner inside `feeder` (committed).** The owner is stepped from the loop; the runtime is the only watchdog owner, and the owner publishes a beat. `feeder` opens it once the hooks are armed, on the journal in storage sectors 0–1 (48 portions, single bowl). Five cold boots Idle (`fw16-boot-01`…`05`).
2. **Stall run and the idle gap.** `owner-stall` (`fw16-stall-01`): the 600 ms stall changed nothing; the 3 s stall ended in a watchdog reset as designed. Boots right after a watchdog reset faulted with no stall: the WPA3 SAE handshake held the priority-2 loop past the 1 s gap. The beat now promises 1 s while outputs may move and 10 s (`LIVENESS_MS`) at rest (v158, five cold boots clean).
3. **First feed through the device API (`b151a2e`, v164).** `wellfed feed --portions 1`: drove 3,018 ms, stopped on no feedback, `Fed` event received.
4. **Pin watch and start-up surge (v165, v166).** The motor's start-up surge tripped overcurrent on one poll; the controller now needs two consecutive polls over the limit. Only PA18 changes during drive: it rests HIGH and drops LOW at a mark.
5. **First complete portion (v167, `fw16-measure-02`).** `completed=1`, 9,072 ms of drive, edges `7888L,9054H`. One portion is one of the rotor's three compartments, about 9.05 s of drive with a mark about 1.17 s LOW. Stock (static) credits PA18's rise and disarms it for 5 s.
6. **With food (feeds 6–8, `fw16-food-01`, `-02`).** Each `Done` at 9.79–9.86 s; PA17 LOW (food present); PA16 changed as food fell; the owner saw food come out, a little less than stock.
7. **Limits (v168, `fw16-limits-01`).** Inhibit 5 s, feedback timeout 15 s, 12 s per portion (`portion_ms`), optics up to 578 s. `make check` 994 tests; loop stack bound 4,816 of 8 KiB.
8. **Three portions (feed 9, v168, `fw16-turn-01`).** `completed=3`, 27,241 ms of drive, edges `7840L,9037H,16970L,18156H,26047L,27221H` (9.1 s a portion, mark about 1.2 s), stopped 20 ms after the third rise; only PA18 changed. The owner's tape mark came back to its start: three marks a turn, one compartment a portion.

Code for steps 4–8 is committed as `756f68f`.

## Open

Findings and loose ends no card has closed yet. Each line says where it came from; move it to a card's notes when one takes it.

**Firmware**
- **Silent watchdog reset after a status-30 association reject** (physical plus static, October 25, `ar17-boot-01`, `-03`): after SAE auth succeeds and the access point rejects the association with 30 (it still holds the previous boot's protected association), `wifi_connect` never returns and the whole KM4 stalls past the watchdog (no `PLSUP1 stop`, ROM banner 4.0 s later). Across all captures: 24 of 441 status-30 rejects on our builds, 17 of 35 on October 25 evening builds (0 of about 330 before), and 8 of 8 on stock, so the hang is likely in the vendor stack and its odds rose with recent timing. No feed is at risk (the join interlock holds feeds) and arming is not skipped. Risk: a trial boot right after an OTA restart almost always meets status 30, so a good build could spend its three trial boots and fall back to bank 0. Since AR31 (October 26) every certain restart leaves the network first, and a silent reset prints `PLSUP1 hang`: the first recorded one was `kind=masked task=tmr` (vendor timer task, interrupts masked). Not the cause of the sector-192 install reset, which had no join running.
- **Intermittent bank-1 write fault during bank-0 installs** (physical, October 25): 2 of about 9 installs failed in sectors 176–200 (`0x236000–0x24E000`): one silent watchdog reset (v183, `ar21-itest-20261025-02-uart-02`, the next boot skipped arming as the once-per-power-cycle rule says) and one `stage=write` readback mismatch (v196). Static analysis rules out watchdog starvation; the likeliest cause is an overrun erase or a write that did not take (flash, or supply during Wi-Fi TX). Since then both banks record the failing step, word and erase time (`PLOTA1 error stage=write …`, `PLSUP1 lease_fault …`, an uncached reread). Four installs on that build were clean (`ar21b-rt-01`, `ar21b-pc-01`: masked max 44–45 ms, none long). Next failure tells erase from program from read. Whether the skip boot should re-arm after one more reset is still the owner's call.
- **Loop hard fault after a rejected association** (physical, October 25, `ar9-gap-20261025-cap-04`): UNDEFINSTR on a valid XIP instruction in `wellfed-loop` right after a status-53 association reject and a 720 ms stall; reset, back on bank 1. One in about 14 forced rejoins. Same class as bank 0's vendor Wi-Fi faults. A fault during a feed would interrupt it (see the latch below). [Result](docs/firmware/scheduling-gaps.md#result-october-25-2026-physical).
- **Idle `acquire()` failure reset-loops a confirmed image** (static, October 24): it faults with `init_failed` false, so it never beats again. Holding the fault would strand a transient glitch. FW19's call.
- **Bank-0 recovery role Wi-Fi fault**, parked: about 1 in 40 bank-0 boots hard-fault in the vendor Wi-Fi threads (`cmd_thread`, `rtw_recv`), none in 199 bank-1 boots; plus one silent watchdog reset after a 2 s power cut (`qa-20261024-02`). [Device API](docs/firmware/device-api.md#left).
- **Four firmware tasks, not shown on hardware:** that settings saves and offer checks still work with a firmware using all four, and the service stages' timing in one task.
- The offer check blocks the service task for up to about 6 s a minute.
- `entropy` variant: `adc::close()` runs under the owner's open board.
- `owner-stall` variant: its 3 s stall now sits inside the 10 s idle gap, so its summary is stale.
- `T: Send` bounds on `Slot` and `RamStatic` break the firmware build (raw pointers); host-only impact.
- As of October 22, unverified since: `net_store::save` lacks verify-before-commit; the three record stores' framing could merge.

**Electrical (FW30)**
- The power-on flicker at the motor (0.5–11.5 µs, also in the ROM loader) is unclassified: outputs-only captures can't tell it apart. Probe INA, INB, OUTA and OUTB at U7 when the micro IC hooks arrive (Wednesday, October 28), then 50 boots and 50 loader entries with `--off-seconds 10`.
- PB3 outlet-emitter OFF level with the meter.

**UART installs**
- Installs sometimes stop on a loader `0x15` (an idle NAK read as a reply): `fw16-loop-04`…`09` after 10–49 sectors, earlier `fw10-pipeline-bank0` sessions. Nothing was retried and `--interrupted` finished each; the cause is open. **October 27 (physical, from the serial event timestamps):** both stops on the night of October 26 followed host-side pauses: the bench host read nothing for 656 ms (`fw46-bank0-20261026-01`) and 1,786 ms (`-02`, right after a 770 ms `erase_intent` fsync), then took the queued idle NAKs in one burst, sent the next command and read `0x15`. Those runs had 18 and 3 pauses over 300 ms (the passing `fw19-bank0-20261026-04` had 2, up to 1,176 ms); `fw46-bank0-20261027-01`, with nothing else running, had none (longest gap 123 ms) and passed. **Confirmed October 27 (physical):** `fw61-bank0-20261027-01` stopped at 103/222 sectors on the same signature (a 431 ms `erase_intent` fsync and a 734 ms read pause, its only one over 300 ms) while another Claude Code session in this checkout ran `cargo nextest` for the server crates (1-minute load 61.8); its `--interrupted` finish (`-02`) passed once the load fell. The two October 26 stops blamed on the logic analyzer (`fw19-bank0-20261026-01`, `-03`) also had their longest pause (504 ms, 343 ms) within the last 12 serial events before the `0x15`, so the analyzer is likely not the cause. **Rule:** no builds or tests on the bench host during a UART write; check `/proc/loadavg` first. A writer that reads through its fsyncs (or `ionice`/`nice` priority) would remove the dependence; that changes the pinned writer's path and needs its own reviewed card.

**Host and tools**

**Setup and pairing (FW45)**
- Not yet on hardware: a mistyped-code run, and the stale-offer fix.
- `wellfed setup`'s search misses an advertising feeder; meanwhile run `bluetoothctl scan le` and connect with `--address`.

## The bench

Scope is the **research PLAF103 only**; the household feeder is excluded. Reconfirm the wiring with the owner before relying on it, and consult the [UART procedure](docs/devices/plaf103/uart.md) before changing it.

- **Relays** (DSD TECH SH-UR04A, CP2102, 9600 8N1, `AT+CHn=1|0|?`):
  - Relay 1: the feeder's USB 5 V (relay off means feeder off, unless batteries are in; check with the owner).
  - Relay 2: the loader strap, in place of the removed button.
  - Relays 3 and 4: the UART adapter's TXD and RXD. `wellfed-bench` opens them after every power cut and closes them just before power-on, so a powered-off feeder isn't back-powered; `--uart-when-off connected` keeps the old behaviour. GND stays common.
- `wellfed-bench` drives the relays and chains `wellfed-flash` ([bench control](docs/devices/plaf103/bench.md)); ports are `/dev/serial/by-id/` paths in `private/bench.json`. A wiring diagram is at `private/bench-wiring.svg`.
- **Logic analyzer:** a 24 MHz fx2lafw clone (needs `sigrok-firmware-fx2lafw`), clipped at the motor's suppression capacitor (CH2 curved leg, blue-lead side; CH3 straight leg); the driver's 1.27 mm pins take no ordinary hook.
- **Hotspot:** a NetworkManager connection on the bench host, named in `private/hotspot.json`, off unless started. The host firewall allows DHCP, DNS, the broker port and UDP 5353 from the hotspot interface; forwarding stays blocked, so the feeder has no internet.
- **Batteries:** three D cells exist; whether they're fitted is the owner's to confirm.

**Before and after bench work**
- `wellfed-bench off` refuses ("Serial port: Device or resource busy") while a `capture` holds the UART; `boot` cuts power itself, so cut mid-feed with `boot`, or after the capture ends (October 25).
- After any interrupted or cancelled bench command, run `wellfed-bench status` before telling the owner the feeder is off: a cancelled `transients` run once left relay 1 on (October 24).
- A UART capture is capped at 180 s, and one observe segment at the flash tool's 512 KiB.
- `boot --off-seconds 15` gives an operator time to get ready before startup probes begin.
- Run `itest` from the main checkout's binaries, or pass `--build-bin`: `wellfed-build` compiles in its own repository root, so a worktree's builder builds that worktree. Rebuild the release binaries after builder sources change. A worktree's OTA watch needs its own `wellfed-flash` built.
- Run `wellfed-build` itself, not through `cargo run`, which sets `RUSTUP_TOOLCHAIN` (refused).
- Stock firmware prints the unit's serial and cloud secrets on UART: keep stock captures private.
- `sudo tcpdump -i <hotspot interface>` is the quickest way to tell feeder problems from host problems.
- If the disk fills, delete Cargo `target/debug/incremental` caches, not whole targets.
- Restore the prior network and service state after a live test and record what ran ([owner authorization](AGENTS.md#owner-authorization-october-22-2026)).

## Device state worth knowing

- **Storage region:** `0x340000–0x360000`, 32 sectors, every sector named in `wellfed_board_plaf103::storage`. Feed journal 0–1, seed 7–8, client store 9–10, event ring 13–24. Stock's clock-save block `0x3f0000–0x400000` is in `layout.json`'s preserve ranges.
- **Wi-Fi store:** empty since a signed clear (October 24); the feeder joins the hotspot with the build's credentials.
- Wi-Fi and broker credentials come from private files; images stay under `private/`.

## Evidence available in this local checkout

Start with **`private/bench-handoff/current.json`**. It indexes the installed review, frozen build, session, physical result, boot capture, verifier result, installation audit, baseline, ROM report and archived research snapshot; its file hashes identify the evidence. Dated paths and bench-specific details stay in that ignored file rather than becoming public fixture conventions.

A fresh clone has no `private/`, vendor images, captures or device backups. It supports ordinary development through the documented checks; continuing this bench session needs the private evidence bundles. Their absence does not require inventing a new baseline or weakening writer checks. See [fixtures](docs/development/fixtures.md) and [preserved research](docs/development/provenance.md).

## Owner decisions and preferences

Recorded scope decisions, not authorization to operate hardware or publish. Follow the current task and AGENTS.md.

- Use the fast [changed-sector UART updater](docs/development/uart-development.md); routine iterations need no full-flash read or stock round trip. Keep fresh helper, source, guard and status checks, exact plans and readback verification.
- No automatic retry or replay of a flash write. `--interrupted` also accepts one sector holding the staged target's first 1–3 program frames followed by erased flash (owner-approved, after a stop at `0x1d6000`), and a bank whose marker sector is wholly erased while every other sector still equals the source (owner-approved October 26, after a stop between that erase and its first program; `00de7e0b`, reviewed).
- Motor bring-up went ahead before FW30's power-on flicker was classified, since the flicker also appears in the ROM loader (October 24).
- The replacement firmware has no MQTT at all since AR6 (October 25): updates go over the device API; the broker stays only for stock.
- BLE setup starts only on a Lock-then-Mic hold on an unlocked panel; losing Wi-Fi never starts it (October 24).
- The startup LED probe is gone: startup only drives the indicators LOW (October 22).
- Preserve the demonstrated conservative recovery path. Stock return through the development updater remains untested.
