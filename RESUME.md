# Resume firmware development

Handoff checkpoint: **October 25, 2026 (about 1 pm), Mountain time**. Read this, then [AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md), and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) (stock parity: [FEATURE_BOARD.md](FEATURE_BOARD.md)).

This file is the handoff, not the history. Keep it to the current state, today's work, what is open, and what a new session must know before touching the bench. When a day ends, fold its entries into the boards' evidence columns and the topic docs, and cut them here; the full log up to October 25 is `git show 756f68f:RESUME.md`.

## Checkpoint

Reconfirm with the owner before any bench operation; this is the last recorded state, not a live observation.

- **Bank 1:** `feeder` `2661f365…` (v198, `ar18-01`, confirmed, October 25): `master` at `052cb6e` (through AR18 and AR8), installed over the device API with the hotspot forwarding dropped; only gateway liveness logged. Five cold boots (`ar-boot-20261025-01`…`05`) came up on bank 1 with the owner idle and unfaulted; the pre-epoch event ring kept epoch 0.
- **Bank 0:** `feeder` R0 `693db48f…` (UART, `ar21-bank0-20261025-02`, October 25, review `ar21-r0-review`): since AR7 it runs the full feeder and installs updates over the device API; a failed trial falls back to it and it feeds. A full-flash backup from before the install is indexed in the evidence file.
- **Next manifest version:** 199; update floor (`min_version`) 198. **Feed journal:** high water 18 (feed 14 was interrupted on purpose and resolved uncertain).

- **Client store:** one owner, the bench CLI's key (`private/wellfed-owner.json`).
- **Bench:** feeder on (relay 1), UART relays 3 and 4 closed, hotspot up (found up on October 25 afternoon and left so), no server, no capture. Hopper empty. The logic analyzer may still be clipped at the motor.
- **Next (FW16):** the motor lease under a stalled loop during a feed, and a failed stop; then FW17. Cancellation during motion passed on October 25 (`feedwrites-20261025-01`, v173: feed 18 cancelled 2 s into the drive, `outcome=2 reason=2`, 0/2 portions, outputs stopped) ([procedure](docs/firmware/motor-pulse.md#feeds-through-the-owner)).
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
- **Silent watchdog reset during a bank-0 install** (physical, October 25, `ar21-itest-20261025-02-uart-02`): about sector 192 of 229 of v183, no fault dump, reset flags `0x00100000`; earlier same-size installs completed. The next boot skipped arming (`PLCFG1 skip marker=0x300`), as the once-per-power-cycle rule says, so it could not feed or take offers until power was cut. Root cause being chased (service task beat during an install). Whether the skip boot should re-arm after one more reset is the owner's call: it trades feeding sooner against a possible hang loop.
- **A feed's write id is forgotten if its session closes first** (static, October 25, `feed-writes` review): the device API remembers a write's outcome only when its pending answer resolves (`wellfed-feeder` `api.rs`), so a client that gives up on a Feed and resends the same id on a new session after the feed ended starts a second feed. Clients never resend an uncertain feed (AGENTS.md), so nothing does today; the feeder should still remember the id from the moment it queues the feed. FW17's request-id rules.
- **No new session during a feed** (physical, October 25, v173): the feeder holds off Noise handshakes while a feed is claimed, so `wellfed cancel-feed` or any CLI command from a fresh process can't connect until the feed ends; only sessions opened before the feed (the server's, the app's) reach it. Matters for FW16's cancel-during-motion step and AR25's held-session test.
- **Loop hard fault after a rejected association** (physical, October 25, `ar9-gap-20261025-cap-04`): UNDEFINSTR on a valid XIP instruction in `wellfed-loop` right after a status-53 association reject and a 720 ms stall; reset, back on bank 1. One in about 14 forced rejoins. Same class as bank 0's vendor Wi-Fi faults. A fault during a feed would interrupt it (see the latch below). [Result](docs/firmware/scheduling-gaps.md#result-october-25-2026-physical).
- **Idle `acquire()` failure reset-loops a confirmed image** (static, October 24): it faults with `init_failed` false, so it never beats again. Holding the fault would strand a transient glitch. FW19's call.
- **Flash writes during a feed** (static, October 25 review): the event ring, settings or keys can take a lease while the motor is on; the quiesce check fails and the board resets. Must be interlocked before scheduled feeds: AR25 (October 25).
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
- Installs sometimes stop on a loader `0x15` (an idle NAK read as a reply): `fw16-loop-04`…`09` after 10–49 sectors, earlier `fw10-pipeline-bank0` sessions. Nothing was retried and `--interrupted` finished each; the cause is open.

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
- No automatic retry or replay of a flash write. `--interrupted` also accepts one sector holding the staged target's first 1–3 program frames followed by erased flash (owner-approved, after a stop at `0x1d6000`).
- Motor bring-up went ahead before FW30's power-on flicker was classified, since the flicker also appears in the ROM loader (October 24).
- The replacement firmware has no MQTT at all since AR6 (October 25): updates go over the device API; the broker stays only for stock.
- BLE setup starts only on a Lock-then-Mic hold on an unlocked panel; losing Wi-Fi never starts it (October 24).
- The startup LED probe is gone: startup only drives the indicators LOW (October 22).
- Preserve the demonstrated conservative recovery path. Stock return through the development updater remains untested.
