# Scheduling gaps across Wi-Fi rejoins

Status: the `gap-probe` variant is built and host-tested (static analysis and host tests, October 25). It has not run on hardware.

## Why

The feeding owner is stepped from the loop task (`wellfed-loop`, priority idle+2), and its beat is renewed only when the loop runs. On October 25 the WPA3 SAE handshake held that loop past the 1 s gap the beat promises while outputs may move (physical, `fw16-stall-01`; the beat now promises 10 s at rest). There are two ways to keep a feed's beat inside 1 s: move the owner to its own task at priority 4, above the Wi-Fi shim's task (idle+3), or hold Wi-Fi joins during a feed. Priority helps only if the driver's handshake work runs below priority 4 and with interrupts unmasked. This measurement settles that, with no motor motion.

## The variant

`wellfed-build build --profile firstboot --firmware feeder --variant gap-probe` builds the normal `feeder` firmware with two additions (`src/firmware/wellfed/feeder/src/gaps.rs`, wired in `app.rs`):

- **A probe task**, `wellfed-gap`, at priority 4 with a 256-word stack. It wakes every 10 ms, reads the 32 kHz system timer and records the gap since its last wake into atomics. It does nothing else: it never touches the owner, the board, flash or the watchdog. It is the firmware's third task of four (`FIRMWARE_TASKS`); the stack audit bounds it at 300 B of 1 KiB, and the loop at 4,848 B of 8 KiB in this variant.
- **The loop measures itself** the same way: each pass's start is a wake, and the delay the previous pass asked for is the expected gap.

Gaps are timed on the system timer rather than the FreeRTOS tick, so a driver section that masks interrupts (and with them the tick) still shows its full length.

**No feeds.** The variant's `Feeds::submit` returns `Unavailable` without calling `Feeding::submit`, which is compiled out of its path. The device API's `Feed` is refused. Nothing else submits a feed. The feeding owner still opens at rest as in the normal image, so the loop carries its usual load and the beat runs as it does in the product.

**Forcing rejoins.** The firmware cannot force one: the Wi-Fi shim in `example_entry.c` owns the connection, and its C interface has only start and an event ring. Adding a disconnect call would change the C interface for a measurement. Rejoins come from the host side: take the bench hotspot down and back up, and the shim loses the link, backs off and runs a full scan, connect (the WPA3 SAE handshake) and DHCP when it returns. The first join after each boot is another sample.

## Records

Every 10 s (`WINDOW_MS`), at the end of a loop pass, the loop logs three lines with the same `t`:

```
PLGAP1 p4 t=<ms> w=<ms> n=<wakes> max=<ms> late=<ms> p99=<ms> over=<a>/<b>/<c> worst=<ms>
PLGAP1 loop t=<ms> w=<ms> n=<passes> max=<ms> late=<ms> p99=<ms> over=<a>/<b>/<c> worst=<ms>
PLGAP1 net t=<ms> gen=<n> att=<n> lost=<n> dns=<n> rejoin=<0|1>
```

At startup, `PLGAP1 task created=1` (or `=0` if `spawn` refused the task).

| Field | Meaning |
| --- | --- |
| `t` | Uptime when the window closed |
| `w` | How long the window ran, by the loop's clock: more than 10,000 when the loop itself was held |
| `n` | Gaps measured in the window (wakes after the first) |
| `max` | The longest gap between two wakes, the asked delay included |
| `late` | The longest time past the asked delay. For `p4` the asked delay is 10 ms; for the loop it is what the previous pass returned, and lateness includes the pass's own run time, since the owner's beat sees that too |
| `p99` | The 99th percentile of lateness, rounded up to its bucket's top (exact below 32 ms, then powers of two: 63, 127, 255, …) |
| `over` | Wakes later than 50, 200 and 1,000 ms |
| `worst` | The longest `max` since boot, so a gap between captures loses nothing |
| `gen` | The Wi-Fi generation, bumped by every join (DHCP address) and every loss |
| `att` | The join attempt of the last successful scan |
| `lost`, `dns` | Losses since boot: of the association, and those the link check noticed (lease or gateway; the internet DNS check before October 25) |
| `rejoin` | 1 when `gen` changed during the window: the link was lost or joined |

The shim's own `PLWIF3` records (scan, connect, DHCP, loss, backoff, with their durations) fall between the `PLGAP1` lines and show where in a window a join ran.

**Reading it.** For each rejoin window, compare `p4 late` with `loop late`. If the priority-4 task stays far under 1 s (say, tens of milliseconds) while the loop's lateness reaches hundreds of milliseconds or more, priority helps, and the owner can move to its own task at priority 4. If both are held about as long, the hold is in a driver section above priority 4 or with interrupts masked, priority does not help, and joins must be held during a feed instead.

**Artifacts.** Logging holds the scheduler while UART drains, about 16 ms for a full line at 115200 baud. Each window's three lines, and every `PLWIF3` or other record line, therefore add a gap of up to that length to whatever the probe sees. A `p4 max` of 10–40 ms in quiet windows is that logging, not the radio.

## Procedure (no motion)

The bench must be powered and on the replacement firmware, with the hotspot and `wellfed-local serve` up for the install. The owner authorization of October 22 covers the OTA install and the hotspot; this procedure moves no motor.

1. **Install over the air:** `target/release/wellfed-bench ota NAME-install --build --firmware feeder --variant gap-probe`. It builds and freezes the review, offers it, and captures until the build confirms. (Over UART instead: the same build, frozen with `wellfed-build review build`, then `wellfed-bench flash-prepare`/`flash-execute` as the [development updater](../development/uart-development.md) describes.) Once health shows the build, stop `serve`: rejoins need only the hotspot.
2. **Capture** in back-to-back 180 s segments: `target/release/wellfed-bench capture NAME-01 --seconds 180`, then `-02`, and so on. Each segment holds about two rejoin cycles.
3. **Force N rejoins** (N = 10 is enough to see the spread) from another shell, with the hotspot connection named in `private/hotspot.json`:
   ```
   HOTSPOT=$(jq -r .connection private/hotspot.json)
   trap 'nmcli connection up "$HOTSPOT"' EXIT   # an interrupted loop leaves the hotspot up
   for i in $(seq 10); do sleep 60; nmcli connection down "$HOTSPOT"; sleep 15; nmcli connection up "$HOTSPOT"; done
   ```
   After a loss the shim backs off from 1 s, doubling up to 30 s between attempts, and a scan waits up to 10 s, so a rejoin lands 15–45 s after the hotspot returns. A 60 s quiet time leaves at least one quiet window between rejoins.
4. **Cold boots** add first-join samples: `wellfed-bench boot NAME-boot-01 --seconds 180` with the hotspot up.
5. **Read:** for each `PLGAP1 net … rejoin=1` window, the `p4` and `loop` lines before it, and the `worst` values at the end. Count the rejoins from `gen` (a loss and a join each bump it).
6. **Restore:** check the hotspot is up (`nmcli connection show --active`), then reinstall the build that was on bank 1 before: its frozen review (`wellfed-bench ota NAME-restore --review private/verification/<prior review>`), or `--build --firmware feeder` from the same `master` commit, so the restored identity is the recorded one. Put the hotspot and `serve` back as found, and record what ran and the restored build id in RESUME.

Pass for the measurement: at least N rejoins captured (`gen` rose by about 2N), and every rejoin window has both lines.

## Result (October 25, 2026, physical)

`gap-probe` v170 (`2b0806ce…`) on bank 1, six 180 s captures (`ar9-gap-20261025-cap-01`…`06`) while the hotspot was cycled ten times (60 s up, 15 s down); 86 complete windows, 20 with a rejoin, across two boots (see the fault below).

- **Rejoins hold every task, not just the loop.** In each window where a join's association ran, the priority-4 task was late by 444–450 ms and the loop by 443–474 ms: the same stall, within a pass's run time. Windows with only a scan or DHCP stayed at 3–17 ms (p4) and 28–29 ms (loop, its own pass). An association rejected with status 53 (stale PMKID) held both for about 720 ms (`worst` 724 and 741 ms).
- **So a higher priority does not shorten the gap.** The stall is below the scheduler (the vendor driver masking or holding the CPU), so moving the feeding owner to its own priority-4 task would see the same 0.45–0.72 s. The design that follows is the interlock: a feed does not start while a join is in progress, and the Wi-Fi task defers a join while outputs may move.
- **A hard fault in the loop, once.** In `cap-04`, after the status-53 rejection and its 720 ms stall, the 8 KiB `wellfed-loop` faulted with UNDEFINSTR (`cfsr=10000`) at `0x0e010ffe`, a valid `mrs r0, IPSR` in `unscheduled` (from `keys::identity_since`), executing from XIP flash; the board reset and came back on bank 1. A valid instruction faulting as undefined points to a bad fetch from flash rather than a code bug, but one capture cannot establish the cause. It is the same class as bank 0's occasional fault in the vendor Wi-Fi threads.

Bank 1 was then restored to the reviewed `e43324be…` (v171, `ar9-restore-20261025-01`).

## The interlock (static analysis and host tests)

`wellfed_hal::interlock` holds whose turn it is, in one atomic the loop and the C Wi-Fi task share, with no change to the C shim. The loop claims the outputs before it submits a feed and releases them once no feed is in flight and the owner is at rest with motor STOP and the emitters off. The shim calls `wellfed_wifi_select` before each attempt and `wellfed_wifi_outcome` after it, so the select takes the join's turn and the outcome ends it: the turn covers the scan, the association and DHCP. Watching a joined link takes no turn. Since October 25 that watch no longer leaves a network that has no internet, so such a network no longer costs a rejoin, and a refused feed, every few seconds ([link liveness](startup-diagnostics.md#link-liveness-without-internet-dns)).

- **A feed during a join** is refused `Busy` (`PLOWN1 feed portions=… held=1`), which clients treat as final for that write id.
- **A join during a feed** waits in the Wi-Fi task, looking again every 50 ms, and marks itself waiting: a second feed is refused (`held=2`) and the release hands the turn straight to the join, so feeds back to back cannot starve it. Joins cannot starve feeds either: an attempt ends within the shim's 10 s scan wait plus one association and one DHCP run, and attempts back off at least 1 s. The association and DHCP runs are bounded only by the vendor SDK's own timeouts inside `wifi_connect` and `LwIP_DHCP` (prebuilt, not in this repository), so the loop counts a join that holds its turn past 60 s (`LONG_JOIN_MS`, logged as `PLOWN1 interlock … long_joins=`); feeds stay refused while it does.
- **No deadlock.** A join waits at most the claim's bound, `FEED_BOUND_MS`: the owner's `optics_max_on_ms` (its configuration check holds the prepare timeout and the longest drive inside it) plus 2 s, 580 s for 48 portions. Past it the join goes ahead and is counted as forced (`PLOWN1 interlock … forced=`); a loop that stops stepping during a feed resets through the beat long before then.
- **Storage writes take a turn too** (host-tested). A flash lease while the outputs may move fails its stopped-outputs check and resets the board, so every store write outside the feed journal takes the write turn (`WriteTurn::begin`) for one record, which it gets only while no feed holds the outputs. A write during a feed stays queued and counts among the waiting writers, so the next claim lets it go first. It is forced only past `FEED_BOUND_MS` of one claim (timed per claim) and only while the outputs read stopped. A claim waits in the loop, polling every 10 ms for at most `WRITE_WAIT_MS` (3 s: one record's three leases at their whole budget plus `WRITER_POLL_MS`, the slowest writer poll in the service task), for a write running, a writer waiting, or a client's change still in its mailbox; a write still running or a client's change still in its mailbox then refuses it (`held=3`), and waiting writers are passed, since they have not started. Writes and joins do not wait for each other. The feed journal writes under the loop's own claim, at the owner's rest boundaries.
- **The cost.** Feeds are refused for a whole attempt, scan and DHCP included, though only the association stalls. While the network is away that is the scan (up to 10 s) every backoff (1–30 s); narrowing the turn to the association alone would need a shim call between scan and connect.

Not yet run on hardware: a feed with the hotspot cycling, to show no association overlaps a feed.
