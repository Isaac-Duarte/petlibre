# Resource budget

This page measures whether the Wellfed product fits on the PLAF103, and what gives way where it doesn't. It covers flash, RAM, tasks, the loop's latency, how Wi-Fi stalls the scheduler, and the storage region. It is for contributors deciding where new work can go. The design it measures is in [product architecture](product-architecture.md).

## The short answer

It fits.

- **Flash:** the current `feeder` image uses about three quarters of a bank, with room for what's still planned.
- **Heap:** about 120 KB free in Bluetooth setup mode, the tightest case.
- **Tasks:** none spare, by design. Everything new joins the loop task.
- **Latency:** two rules keep the loop responsive: one handshake operation per pass, and feeds and Wi-Fi joins take turns.
- **Storage:** a 48-sector region, with 3 sectors still free; the meal call's clip takes the last 8.

## Flash

Each OTA bank holds 1,376,256 bytes, stock 1.1.15's own size (`layout.json`).

| Image | Payload | Of a bank |
| --- | ---: | ---: |
| `blink`, the smallest firmware | 684,032 | 50 % |
| `feeder` with the panel only | 684,032 | 50 % |
| `feeder` with Noise and the session table, BLE linked out | 720,896 | 52 % |
| `feeder` with BLE reachable | 868,352 | 63 % |
| `feeder` with the device API, mDNS and the event ring | 942,080 | 68 % |
| `feeder` with traces and the stock backup | 1,015,808 | 74 % |

- **BLE:** about 147 KB, which the product always carries, because any boot can enter setup mode.
- **Feeding owner:** about 12,288 bytes, 1 % of a bank, costed from the bring-up firmware's `motor` and `pulse` images before it joined the product.
- **mDNS:** the vendor library would have cost about 33,500 bytes of code. The product's own responder replaced it (see [mDNS](#mdns)).
- **Crypto:** the largest pieces are SHA-512 for update signatures (21 KB) and curve25519's field arithmetic.
- **Headroom:** with BLE, the first estimate left about 508 KB, and put the rest of the product (feeding owner, API protocol, schedules, alerts, pairing, mDNS, NTP) at 80–140 KB.

## RAM

KM4 static sections, in bytes, from the first product image:

| Section | `blink` | `feeder` normal | `feeder` setup |
| --- | ---: | ---: | ---: |
| `.ram_image2.text` (code in RAM) | 98,472 | 98,488 | 98,608 |
| `.ram_image2.data` | 12,936 | 24,616 | 25,792 |
| `.ram_image2.bss` | 37,576 | 37,616 | 41,028 |
| `.ram_image2.nocache.data` | 23,928 | 23,920 | 23,932 |

The session table is the 11,576-byte `BUDGET` static, about 2.9 KB a client. It is built at compile time (`Slot::filled`) because it is larger than the loop task's stack.

**Heap.** FreeRTOS heap, free bytes, from `PLAPI1 heap`. Task stacks come from this heap.

| Mode | At boot | Steady, free | Steady, least ever |
| --- | ---: | ---: | ---: |
| Normal | 271,776 | 146,464 | 141,792 |
| BLE setup mode | 267,040 | 131,904 | 123,136 |
| Normal, with the device API | — | 125,088 | — |

- **No leak:** the normal image held 146,464 free for three more minutes.
- **BLE:** setup mode costs about 15–19 KB of heap (`PLBLE1 ready heap_free=135136`).
- **Headroom:** about 120 KB, with the four-client table already counted. lwIP's own pools are separate (see [clients](#clients-request-ids-and-buffers)).
- **The device API's cost** (146,464 down to 125,088) is not yet attributed.

## Tasks

A firmware gets four tasks (`FIRMWARE_TASKS`). The product uses two:

| Task | Stack | Audited bound |
| --- | --- | --- |
| `wellfed-key`, the button service | 2 KiB | 1,248 B |
| `wellfed-loop`, the loop | 8 KiB (2,048 words) | 5,852 B in the latest build (battery mode); 5,412 B with traces and the stock backup; 4,784 B when the feeding owner and the join interlock arrived |

- Both bounds include the formatting the task can reach. The audit charges it, since it bounds indirect calls by class.
- In the loop, a handshake operation's X25519 is the deepest path. The device API took the bound past a 6 KiB stack's 25 % margin; the feeding owner's step stays below it.
- **The loop's headroom is narrowing.** The audit refuses a bound above 75 % of the stack: 6,144 B of 8 KiB. At 5,852 B the loop has 292 B left, down from 732 B at 5,412 B. The next feature that deepens the loop's deepest path may need a bigger stack or a shallower path. The build fails the audit rather than overflowing, so this cannot slip through unnoticed.
- Test variants add a third task: `gap-probe` adds `wellfed-gap` (256 words at priority 4) for the [scheduling-gap measurement](#scheduling-gaps-across-wi-fi-rejoins), and `held-stall` adds `wellfed-hld` (256 words at priority 3) for the reset-storm drill.

**Outside the audit.** The stack audit takes its roots from the firmware's `spawn` calls, so C-created tasks are outside it:

- The C Wi-Fi task (`wellfed_wifi`, 1,280 words, created by `example_entry.c`). The Rust it reaches through `wellfed_wifi_select`, `wellfed_wifi_outcome` and `wellfed_wifi_watch` is unbounded by the audit, as is the shim's own C. That Rust is the credential choice, a flash-mapped read of the stored credentials, the interlock's atomics and `vTaskDelay` while a join waits, and the link's liveness machine. Nothing on that path formats.
- The BLE stack's own C tasks, which cost heap. The setup-mode figure above includes them.

**A stack audit fix.** The first product build overflowed the loop's 4 KiB stack (`cfsr=100000`, STKOF) on every trial boot, and KM0 rolled it back. The audit had passed it because its frame pattern skipped Thumb-2 `subw sp, sp, #imm`, which LLVM uses for frames up to 4 KiB. The pattern now counts it. The corrected audit also needed:

- the runtime's `wellfed-net` task raised to 2,048 words (bound 5,488 B), a task since deleted with the MQTT transport;
- Noise public keys derived with the X25519 ladder instead of `PublicKey::from`, whose Edwards multiplication has a 1.9 KiB frame.

Every firmware and variant builds under the corrected audit.

## Handshakes and loop latency

The loop runs one Noise operation (one side writing or reading one message) per step. Timed on the 32 kHz system timer, the worst of three runs each:

| | XX (pairing) | KK (reconnect) |
| --- | ---: | ---: |
| Messages | 3 | 2 |
| Initiator, all operations | 42–55 ms | 56–122 ms |
| Responder, all operations | 42 ms | 56–72 ms |
| Longest single operation | 28 ms | 45–77 ms |
| 1 KiB frame, sealed and opened each way | 3.4 ms | 3.4 ms |

- **X25519:** one operation costs about 25–28 ms on the KM4. Generating each side's ephemeral key adds one more, inside `Handshake::new`, which the runs built outside the timing.
- **Spread:** the 77 ms outlier did the same work as a 45 ms run. Other tasks (Wi-Fi, and then the MQTT task) preempted the loop by tens of milliseconds.
- **Per handshake:** the feeder spends about 80–110 ms of CPU, spread over two or three loop steps.
- **Transport messages are cheap:** about 0.9 ms to seal or open a kilobyte.

Sessions today use Noise IK rather than KK ([device API](device-api.md)).

**What that means for the loop.** One handshake operation blocks the loop for up to about 80 ms. So the loop does at most one handshake operation per pass, across all clients, and four clients connecting at once cannot stack up to 320 ms.

During a feed, only IK session handshakes go on, one operation a pass, and the owner is stepped again right after each such pass. A refused one closes unanswered and holds the rest until the feed ends; invites and recovery wait. The owner's observation gap (100 ms) bounds a pass during a drive. An operation (one or two X25519s, about 55 ms with no preemption) fits it, with the owner stepped on both sides ([device API](device-api.md#in-the-firmware)). `PLOWN1 steps` measures the real gap per feed.

The motor's stop guarantees never depend on loop timing. They stay with the supervision and hardware timers.

## Scheduling gaps across Wi-Fi rejoins

The feeding owner is stepped from the loop task (`wellfed-loop`, priority idle+2), and its beat is renewed only when the loop runs. The WPA3 SAE handshake once held that loop past the 1 s gap the beat promises while outputs may move. (At rest the beat now promises 10 s.)

There were two ways to keep a feed's beat inside 1 s: move the owner to its own task at priority 4, above the Wi-Fi shim's task (idle+3), or hold Wi-Fi joins during a feed. Priority helps only if the driver's handshake work runs below priority 4, with interrupts unmasked. A measurement settled it, with no motor motion.

### The result

**A Wi-Fi association holds every task, not just the loop.**

- In each window where a join's association ran, the priority-4 task was late by 444–450 ms, and the loop by 443–474 ms: the same stall, within a pass's run time.
- Windows with only a scan or DHCP stayed at 3–17 ms for the probe, and 28–29 ms for the loop (its own pass).
- An association rejected with status 53 (stale PMKID) held both for about 720 ms (`worst` 724 and 741 ms).

**So a higher priority does not shorten the gap.** The stall is below the scheduler: the vendor driver masks interrupts or holds the CPU. A priority-4 feeding task would see the same 0.45–0.72 s. The design that follows is the interlock below.

**One hard fault in the loop.** Right after the status-53 rejection and its 720 ms stall, the 8 KiB `wellfed-loop` faulted with UNDEFINSTR (`cfsr=10000`) at `0x0e010ffe`. That address holds a valid `mrs r0, IPSR` in `unscheduled` (from `keys::identity_since`), executing from XIP flash. The board reset and came back on bank 1. A valid instruction faulting as undefined points to a bad fetch from flash, not a code bug, but one capture cannot establish the cause. It is the same class as the bank-0 download faults that stopped once the flash clock was lowered to 50 MHz ([OTA design](ota-design.md#bank-0-download-faults)).

### The interlock

`wellfed_hal::interlock` holds whose turn it is, in one atomic the loop and the C Wi-Fi task share, with no change to the C shim.

- The loop claims the outputs before it submits a feed. It releases them once no feed is in flight and the owner is at rest, with motor STOP and the emitters off.
- The shim calls `wellfed_wifi_select` before each attempt and `wellfed_wifi_outcome` after it. The select takes the join's turn and the outcome ends it, so the turn covers the scan, the association and DHCP.
- Battery mode's radio changes take the same turn: the leave with `wifi_off`, and each `wifi_on` (`wellfed_wifi_radio_begin` and `wellfed_wifi_radio_end`). They stall the driver as an association does ([battery mode](power.md#battery-mode)).
- Watching a joined link takes no turn. That watch no longer leaves a network that has no internet, so such a network no longer costs a rejoin, and a refused feed, every few seconds ([link liveness](startup-diagnostics.md#link-liveness-without-internet-dns)).

How each case plays out:

- **A feed during a join** is refused `Busy` (`PLOWN1 feed portions=… held=1`). Clients treat that as final for that write id.
- **A join during a feed** waits in the Wi-Fi task, looking again every 50 ms, and marks itself waiting. A second feed is then refused (`held=2`), and the release hands the turn straight to the join, so back-to-back feeds cannot starve it.
- **Joins cannot starve feeds either.** An attempt ends within the shim's 10 s scan wait, plus one association and one DHCP run, and attempts back off at least 1 s. The association and DHCP runs are bounded only by the vendor SDK's own timeouts inside `wifi_connect` and `LwIP_DHCP`, which are prebuilt and not in this repository. So the loop counts a join that holds its turn past 60 s (`LONG_JOIN_MS`, logged as `PLOWN1 interlock … long_joins=`). Feeds stay refused while it does.
- **No deadlock.** A join waits at most the claim's bound, `FEED_BOUND_MS`: the owner's `optics_max_on_ms` plus 2 s, which is 580 s for 48 portions. (The owner's configuration check holds the prepare timeout and the longest drive inside `optics_max_on_ms`.) Past the bound the join goes ahead and is counted as forced (`PLOWN1 interlock … forced=`). A loop that stops stepping during a feed resets through the beat long before then.
- **Storage writes take a turn too.** A flash lease while the outputs may move fails its stopped-outputs check and resets the board. So every store write outside the feed journal takes the write turn (`WriteTurn::begin`) for one record, which it gets only while no feed holds the outputs.
  - A write during a feed stays queued and counts among the waiting writers, so the next claim lets it go first.
  - It is forced only past `FEED_BOUND_MS` of one claim (timed per claim), and only while the outputs read stopped.
  - A claim waits in the loop, polling every 10 ms, for at most `WRITE_WAIT_MS`: 3 s, one record's three leases at their whole budget plus `WRITER_POLL_MS`, the slowest writer poll in the service task. It waits for a write running, a writer waiting, or a client's change still in its mailbox. A write still running, or a client's change still in its mailbox, then refuses the feed (`held=3`). Waiting writers are passed, since they have not started.
  - Writes and joins do not wait for each other.
  - The feed journal writes under the loop's own claim, at the owner's rest boundaries.
- **The cost.** Feeds are refused for a whole attempt, scan and DHCP included, though only the association stalls. While the network is away, that is the scan (up to 10 s) every backoff (1–30 s). Narrowing the turn to the association alone would need a shim call between scan and connect.

### The `gap-probe` variant

`wellfed-build build --profile firstboot --firmware feeder --variant gap-probe` builds the normal `feeder` firmware with two additions (`src/firmware/wellfed/feeder/src/gaps.rs`, wired in `app.rs`):

- **A probe task,** `wellfed-gap`, at priority 4 with a 256-word stack. It wakes every 10 ms, reads the 32 kHz system timer, and records the gap since its last wake into atomics. It never touches the owner, the board, flash or the watchdog. It is the firmware's third task of four. The stack audit bounds it at 300 B of 1 KiB, and the loop at 4,848 B of 8 KiB in this variant.
- **The loop measures itself** the same way: each pass's start is a wake, and the delay the previous pass asked for is the expected gap.

Gaps are timed on the system timer rather than the FreeRTOS tick, so a driver section that masks interrupts (and with them the tick) still shows its full length.

**No feeds.** The variant's `Feeds::submit` returns `Unavailable` without calling `Feeding::submit`, which is compiled out of its path. The device API's `Feed` is refused, and nothing else submits a feed. The feeding owner still opens at rest, so the loop carries its usual load and the beat runs as in the product.

**Rejoins come from the host.** The firmware cannot force one: the Wi-Fi shim in `example_entry.c` owns the connection, and its C interface has only start and an event ring. Adding a disconnect call would change the C interface for a measurement. Taking the bench hotspot down and back up makes the shim lose the link, back off, and run a full scan, connect (the WPA3 SAE handshake) and DHCP when it returns. The first join after each boot is another sample.

**Records.** Every 10 s (`WINDOW_MS`), at the end of a loop pass, the loop logs three lines with the same `t`:

```
PLGAP1 p4 t=<ms> w=<ms> n=<wakes> max=<ms> late=<ms> p99=<ms> over=<a>/<b>/<c> worst=<ms>
PLGAP1 loop t=<ms> w=<ms> n=<passes> max=<ms> late=<ms> p99=<ms> over=<a>/<b>/<c> worst=<ms>
PLGAP1 net t=<ms> gen=<n> att=<n> lost=<n> dns=<n> rejoin=<0|1>
```

At startup it logs `PLGAP1 task created=1`, or `=0` if `spawn` refused the task.

| Field | Meaning |
| --- | --- |
| `t` | Uptime when the window closed |
| `w` | How long the window ran, by the loop's clock: more than 10,000 when the loop itself was held |
| `n` | Gaps measured in the window (wakes after the first) |
| `max` | The longest gap between two wakes, the asked delay included |
| `late` | The longest time past the asked delay. For `p4` that is 10 ms; for the loop it is what the previous pass returned, and lateness includes the pass's own run time, since the owner's beat sees that too |
| `p99` | The 99th percentile of lateness, rounded up to its bucket's top (exact below 32 ms, then powers of two: 63, 127, 255, …) |
| `over` | Wakes later than 50, 200 and 1,000 ms |
| `worst` | The longest `max` since boot, so a gap between captures loses nothing |
| `gen` | The Wi-Fi generation, bumped by every join (DHCP address) and every loss |
| `att` | The join attempt of the last successful scan |
| `lost`, `dns` | Losses since boot: of the association, and those the link check noticed (lease or gateway) |
| `rejoin` | 1 when `gen` changed during the window: the link was lost or joined |

The shim's own `PLWIF3` records (scan, connect, DHCP, loss, backoff, with their durations) fall between the `PLGAP1` lines and show where in a window a join ran.

**Reading it.** For each rejoin window, compare `p4 late` with `loop late`. If the priority-4 task stays far under 1 s while the loop's lateness reaches hundreds of milliseconds, priority helps. If both are held about as long, the hold is in a driver section above priority 4 or with interrupts masked, and joins must be held during a feed instead.

> [!NOTE]
> Logging holds the scheduler while the UART drains, about 16 ms for a full line at 115200 baud. Every record line adds a gap of up to that length to what the probe sees. A `p4 max` of 10–40 ms in quiet windows is logging, not the radio.

### Repeating the measurement

No motor moves. The feeder must be powered and on the replacement firmware, with the hotspot up for the install, which goes over the device API. `wellfed-stock serve` is not needed.

1. Install over the air: `target/release/wellfed-bench ota NAME-install --build --firmware feeder --variant gap-probe`. It builds and freezes the review, offers it, and captures until the build confirms. Over UART instead, freeze the same build with `wellfed-build review build`, then use `wellfed-bench flash-prepare`/`flash-execute` as the [development updater](../development/uart-development.md) describes.
2. Capture in back-to-back 180 s segments: `target/release/wellfed-bench capture NAME-01 --seconds 180`, then `-02`, and so on. Each segment holds about two rejoin cycles.
3. Force 10 rejoins from another shell, with the hotspot connection named in `private/hotspot.json`:
   ```
   HOTSPOT=$(jq -r .connection private/hotspot.json)
   trap 'nmcli connection up "$HOTSPOT"' EXIT   # an interrupted loop leaves the hotspot up
   for i in $(seq 10); do sleep 60; nmcli connection down "$HOTSPOT"; sleep 15; nmcli connection up "$HOTSPOT"; done
   ```
   After a loss the shim backs off from 1 s, doubling up to 30 s between attempts, and a scan waits up to 10 s. So a rejoin lands 15–45 s after the hotspot returns. The 60 s quiet time leaves at least one quiet window between rejoins.
4. Add first-join samples with cold boots: `wellfed-bench boot NAME-boot-01 --seconds 180`, with the hotspot up.
5. Read each `PLGAP1 net … rejoin=1` window, the `p4` and `loop` lines before it, and the `worst` values at the end. Count rejoins from `gen`: a loss and a join each bump it. You should see `gen` rise by about 20, and both lines in every rejoin window.
6. Restore. Check the hotspot is up (`nmcli connection show --active`). Reinstall the build that was on bank 1 before: its frozen review (`wellfed-bench ota NAME-restore --review private/verification/<prior review>`), or `--build --firmware feeder` from the same `master` commit, so the restored identity is the recorded one. Put the hotspot and `serve` back as you found them, and record what ran and the restored build id.

## Clients, request IDs and buffers

- **Client limit: 4.** lwIP allows 10 TCP connections and 5 listeners (`lwipopts.h`). An update download and the API listener each hold one. Four clients leaves room to refuse a fifth.
- **Request-ID ring: 32 per client** (512 B). A client retries a lost answer within seconds, and 32 outstanding or recent requests cover any realistic burst. A feed's ID also goes into its journal reservation, so the ring is not what stops a double feed.
- **Frame: 1 KiB.** That covers every planned request and event batch. Firmware images move in 512-byte chunks.

## Event history (7 days)

- **Record:** 32 bytes (sequence, time, kind, source, a 16-byte body, check).
- **Rate:** about 100 events a day in normal use: feeds, button gestures, lock changes, settings, boots and hourly clock-sync summaries. Samples are never stored. We plan for 200 a day.
- **Size:** 7 days at 200 a day is 1,400 records, 44.8 KB. That is 11 sectors, plus one being erased: **12 sectors**.
- **Wear:** each sector is erased about every 8 days, about 50 times a year, far inside NOR endurance.

## The storage region

The replacement's storage lives at 0x340000–0x370000: 48 sectors of 4 KiB. The full sector map is in [pairing](pairing.md#storage-sectors).

**Why it grew.** The original region was 16 sectors (0x340000–0x350000). The journal (sectors 0–1), settings (2–3), confirmation (4) and Wi-Fi (5–6) left 9 free sectors, 36 KB. Client keys and schedules needed a few of those too, so a 12-sector event ring did not fit. Staying at 16 sectors would have meant about 3 days of history.

So the owner extended the region to 32 sectors (0x340000–0x360000), then to 48 (0x340000–0x370000) for the lifetime statistics.

**Why that space is safe to use.**
- A static pass over stock 1.1.15 found no fixed or bounded computed flash access in 0x306000–0x3e8000.
- All 15 full dumps of the research unit are blank there, nine of them from before the replacement ran.
- The area is headroom left by the vendor's shifted SDK layout. Only an oversized OTA image (over about 1.79 MB) or the UART console's `FLASH` command could reach it.
- 0x370000 is still 480 KiB below stock's settings at 0x3e8000.
- The region grows only upward, so bank 1's room is unchanged.

**Stock's clock block.** The same pass found that stock's clock checkpoint save block-erases **0x3f0000–0x400000**, 64 KiB at a time, at minute 30 and on save. It is in `layout.json`'s preserve ranges (`clock_checkpoint_block`). Nothing of ours is near it.

**Statistics wear.** Sectors 32–33 hold the lifetime statistics: one 288-byte record, saved hourly, as each boot starts and at an intentional restart, within a budget each hourly save refills by one. That is at most 6 erases plus 2 per hour of uptime under any crash pattern, about 8,800 a year of each sector ([statistics](device-api.md#statistics)).

## mDNS

The vendor responder is 33.5 KB of code and 2.3 KB of BSS, and it creates its own FreeRTOS task, outside the stack audit. The feeder needs only its own advertisement: a PTR, SRV, TXT and A answer for one service. So the device API has its own responder in the loop task, on one UDP socket: a few KB and no task ([device API](device-api.md#mdns)).

## Evidence

- **Observed** (October 23, 2026), the first product budget: the `feeder` firmware installed over the air as `fw44-budget-02` (v95, normal) and `fw44-setup-01` (v96, BLE setup mode from boot), each confirmed, then the normal build restored (`fw44-budget-04`, v97). The steady heap was held for three more minutes in `fw44-budget-03`. Sizes come from the builds' manifests and ELF sections; heap and handshake times from the feeder's `PLAPI1` records. Nothing was run by the owner. At that point the feeding owner, the vendor mDNS library and the device API protocol were not linked, and were costed from other builds.
- **Observed** (October 23): the stack overflow that exposed the audit's `subw` gap, `fw44-budget-01` (v94).
- **Modeled** (October 25–26, image audits): the loop's stack bounds of 4,784 B (the feeding owner and the join interlock) 5,412 B (traces and the stock backup, [device API](device-api.md#stock-backup)) and 5,852 B (October 28, battery mode, on the desiccant reminder), and the payloads of the later builds.
- **Observed** (October 24): the device API's figures, from the `feeder` builds' manifest audits and `PLAPI1` records ([device API](device-api.md#resources)).
- **Observed** (October 25), the scheduling gaps: `gap-probe` v170 (`2b0806ce…`) on bank 1, six 180 s captures (`ar9-gap-20261025-cap-01`…`06`) while the hotspot was cycled ten times (60 s up, 15 s down). That gave 86 complete windows, 20 with a rejoin, across two boots. The hard fault was in `cap-04`. Bank 1 was then restored to the reviewed `e43324be…` (v171, `ar9-restore-20261025-01`).
- **Observed** (October 25): the WPA3 SAE handshake holding the loop past 1 s, in `fw16-stall-01`.
- **Modeled:** the interlock, including storage writes' turn, is host-tested. A feed with the hotspot cycling, showing that no association overlaps a feed, has not yet run on hardware.
- **Modeled:** the `gap-probe` variant was built and host-tested before its run.
- **Recovered:** the storage region's safety, from a static pass over stock 1.1.15 and the research unit's 15 full dumps. The owner chose 32 sectors on October 23 and 48 on October 26.
