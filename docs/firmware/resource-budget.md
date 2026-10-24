# Product resource budget (FW44)

Status: physical (October 23, 2026), nothing run by the owner. **FW47's actual figures** (the API, mDNS and the event ring linked, October 24) are in [device API](device-api.md#resources): payload 942,080, loop stack bound 4,552 of 6,144 B, about 125 KB of heap free. The `feeder` firmware was installed over the air as `fw44-budget-02` (v95, normal) and `fw44-setup-01` (v96, BLE setup mode from boot), each confirmed, with the normal build restored afterwards (`fw44-budget-04`, v97). Sizes are from the builds' manifests and ELF sections; heap and handshake times are from the feeder's own `PLAPI1` records. This answers whether the product ([product architecture](product-architecture.md)) fits on the PLAF103, and what gives way where it does not.

## What was linked

The `feeder` image as of FW44:
- **Runtime:** startup checks, supervision, settings writer, MQTT transport, update offer check.
- **Panel:** the button service and the child lock (FP11).
- **Noise:** `petlibre-core::noise`, XX and KK over 25519, ChaChaPoly and SHA-256, checked byte for byte against `snow` on the host.
- **Session table:** four clients, each with its Noise state, a 1 KiB receive and a 1 KiB send buffer, and a ring of 32 request IDs (`feeder/src/budget.rs`). The loop drives two of them through every handshake at boot.
- **BLE setup mode:** the `setup` variant started `BleSetup` in the loop from boot. The normal image links the stack but never starts it. FW45 retired the variant (October 24): setup mode now starts on the owner's gesture, at the same cost.

Not linked yet, and costed from other builds instead:
- **Feeding owner:** the `motor` firmware with and without its `fw16-pulse` variant.
- **mDNS:** the vendor library, `lib_mdns.a`.
- **Device API protocol (FW47):** no socket is opened, because pairing and keys are FW45's.

## Flash

Each OTA bank holds 1,376,256 bytes, stock 1.1.15's own size (`layout.json`).

| Image | Payload | Of a bank |
| --- | ---: | ---: |
| `blink`, the smallest firmware | 684,032 | 50 % |
| `feeder` at FP11 (panel only) | 684,032 | 50 % |
| `feeder` normal (Noise, session table; BLE linked out) | 720,896 | 52 % |
| `feeder` with BLE reachable (`setup`) | 868,352 | 63 % |
| Feeding owner, from `motor` + `fw16-pulse` | +12,288 | +1 % |
| mDNS, vendor library code | about +33,500 | +2 % |

- **BLE:** about 147 KB, which the product always carries, because any boot can enter setup mode. Count the `setup` figure.
- **Headroom:** about 508 KB with BLE. The rest of the product (feeding owner, API protocol, schedules, alerts, pairing, mDNS, NTP) is estimated at 80–140 KB, which leaves at least 200 KB spare.
- **Crypto:** the largest pieces are SHA-512 for update signatures (21 KB, already present) and curve25519's field arithmetic.

## RAM

KM4 static sections of the normal image (bytes):

| Section | `blink` | `feeder` normal | `feeder` setup |
| --- | ---: | ---: | ---: |
| `.ram_image2.text` (code in RAM) | 98,472 | 98,488 | 98,608 |
| `.ram_image2.data` | 12,936 | 24,616 | 25,792 |
| `.ram_image2.bss` | 37,576 | 37,616 | 41,028 |
| `.ram_image2.nocache.data` | 23,928 | 23,920 | 23,932 |

The session table is the 11,576-byte `BUDGET` static, about 2.9 KB a client. It is built at compile time (`Slot::filled`) because it is larger than the loop task's stack.

FreeRTOS heap, free bytes, from `PLAPI1 heap` (task stacks come from this heap):

| Mode | At boot | Steady, free | Steady, least ever |
| --- | ---: | ---: | ---: |
| Normal (v95) | 271,776 | 146,464 | 141,792 |
| BLE setup mode (v96) | 267,040 | 131,904 | 123,136 |

- **Steady:** the normal image held 146,464 free for three further minutes (`fw44-budget-03`), so nothing leaks.
- **BLE:** setup mode costs about 15–19 KB of heap (`PLBLE1 ready heap_free=135136`).
- **Headroom:** about 120 KB, and the four-client table is already counted. lwIP's own pools are separate; see clients below.

## Tasks

- **Firmware tasks:** the product stays within `FIRMWARE_TASKS`: the button service (`petlibre-key`, bound 728 B of 2 KiB) and the loop (`petlibre-loop`, bound 3,820 B of 6 KiB).
- **Loop stack:** 1,536 words. A handshake operation's X25519 is the deepest path, and a 4 KiB stack would not keep the audit's 25 % margin.
- **Not on the budget:** the BLE stack's own C tasks and, if used, the vendor mDNS task are C tasks outside the stack audit. They cost heap, which the setup-mode figure above includes for BLE.

**Stack audit fix:** the first FW44 build (`fw44-budget-01`, v94) overflowed the loop's 4 KiB stack (`cfsr=100000`, STKOF) on every trial boot, and KM0 rolled it back. The audit had passed it because its frame pattern skipped Thumb-2 `subw sp, sp, #imm`, which LLVM uses for frames up to 4 KiB. The pattern now counts it. The corrected audit also needed:
- the runtime's `petlibre-net` raised to 2,048 words (bound 5,488 B);
- Noise public keys derived with the X25519 ladder instead of `PublicKey::from`, whose Edwards multiplication has a 1.9 KiB frame.

Every firmware and variant builds under the corrected audit.

## Handshakes and loop latency

From `PLAPI1 handshake`, the worst of three runs each, timed on the 32 kHz system timer. A run does one Noise operation (one side writing or reading one message) per loop step.

| | XX (pairing) | KK (reconnect) |
| --- | ---: | ---: |
| Messages | 3 | 2 |
| Initiator, all operations | 42–55 ms | 56–122 ms |
| Responder, all operations | 42 ms | 56–72 ms |
| Longest single operation | 28 ms | 45–77 ms |
| 1 KiB frame, sealed and opened each way | 3.4 ms | 3.4 ms |

- **X25519:** one operation costs about 25–28 ms on the KM4. Generating each side's ephemeral key adds one more, inside `Handshake::new`, which the run builds outside the timing.
- **Spread:** the 77 ms outlier came from the same work as 45 ms in the other run, so other tasks (Wi-Fi, MQTT) preempt the loop by tens of milliseconds.
- **Per handshake:** the feeder spends about 80–110 ms of CPU, spread over two or three loop steps.

**Loop latency:** one handshake operation blocks the loop for up to about 80 ms. The FW47 loop must therefore:
- do at most one handshake operation per pass, across all clients, so four clients connecting at once cannot stack up to 320 ms;
- defer handshake operations while a feed is in progress, so the feeding owner's steps are never more than one transport message apart. The motor's stop guarantees stay with the supervision and hardware timers, never with loop timing.

Transport messages are cheap: about 0.9 ms to seal or open a kilobyte.

## Clients, request IDs and buffers

- **Client limit: 4.** lwIP allows 10 TCP connections and 5 listeners (`lwipopts.h`). The MQTT transport (until FW47 removes it), an OTA download, a speaker stream and the API listener each hold one. Four clients leaves room to refuse a fifth.
- **Request-ID ring: 32 per client** (512 B). A client retries a lost answer within seconds, and 32 outstanding or recent requests cover any realistic burst; a feed's ID also goes into its journal reservation (FW17), so the ring is not what stops a double feed.
- **Frame: 1 KiB.** That covers every planned request and event batch; firmware images move in chunks through FW48.

## Event history (7 days)

- **Record:** 32 bytes (sequence, time, kind, source, a 16-byte body, check).
- **Rate:** about 100 events a day in normal use. Feeds, button gestures, lock changes, settings, boots and hourly clock-sync summaries; samples are never stored. Plan for 200 a day.
- **Size:** 7 days at 200 a day is 1,400 records, 44.8 KB. That is 11 sectors, plus one being erased: **12 sectors**.
- **Wear:** each sector is erased about every 8 days, about 50 times a year, far inside NOR endurance.

**It did not fit the original region,** 16 sectors (0x340000–0x350000):
- **In use:** the journal (0–1), settings (2–3), confirmation (4) and Wi-Fi (5–6).
- **Free:** 9 sectors, 36 KB.
- **Wanted from the free sectors:** client keys (FW45) and plans (FP03) also need a few.

**The owner chose to extend the region (October 23)** to 32 sectors, 0x340000–0x360000. A static pass over stock 1.1.15 found no fixed or bounded computed flash access in 0x306000–0x3e8000, and all 15 full dumps of the research unit are blank there, nine of them from before the replacement ran. The area is headroom left by PetLibro's shifted SDK layout; only an oversized OTA image (over about 1.79 MB) or the UART console's `FLASH` command could reach it. Sectors 7–31 (25) are free for client keys (FW45), plans (FP03) and the 12-sector ring (FW47). The alternative was 16 sectors and about 3 days of history.

The same pass found that stock's clock checkpoint save block-erases **0x3f0000–0x400000** (64 KiB at a time) at minute 30 and on save. It is now in `layout.json`'s preserve ranges (`clock_checkpoint_block`); nothing of ours is near it.

## mDNS

The vendor responder is 33.5 KB of code and 2.3 KB of BSS, and it creates its own FreeRTOS task, outside the stack audit. The loop needs only the feeder's own advertisement, a PTR, SRV, TXT and A answer for one service. A responder in the loop task on one UDP socket is a few KB and no task. FW47 writes that.

## What this leaves for FW45–FW48

- **Flash:** at least 200 KB spare after everything planned.
- **Heap:** about 120 KB in BLE setup mode, the tightest case.
- **Tasks:** none spare by design; everything new joins the loop.
- **Latency:** the loop's two rules above.
- **Storage:** 25 free sectors in the extended region.
