# Startup and diagnostics

Every Wellfed boot puts the feeder in a known safe state before any firmware code runs: motor stopped, infrared emitters off, indicator lights dark. Then it proves it, by reading every output back and printing the result on the UART. This page explains that startup, what each UART record means, and how the firmware watches its own network link and its own hangs. It is for owners who want to know why the feeder is safe at power-on, and for contributors reading a capture.

The bring-up probes that once ran at startup, and the startup policies the audit still checks for them, are in [bring-up probes](bring-up-probes.md).

## What every boot does

1. **KM0 powers the shared pads.** Before it starts KM4, KM0 enables the audio-shared pads, sets the audio LDO's voltage trim and logs `PLPWR1` ([below](#shared-pad-power)). It also starts the WLAN firmware, which runs on KM0.
2. **KM4 stops every output.** The motor goes to STOP and the emitters to OFF, and each pin is read back ([stopped outputs](#stopped-outputs)).
3. **A synthetic diagnostic runs.** It exercises the feeding core with no hardware attached and reports pass or fail.
4. **The indicators are checked LOW.** With outputs still stopped, every light is verified dark.
5. **The runtime starts.** Wi-Fi, the device API and the firmware's tasks start. Probes open the sensors and the ADC only in the firmwares that run them.

Startup itself never opens the feeding owner. That the feeding backend is unopened does **not** mean startup has no electrical side effects: it drives pins, and their transients are measured separately ([motor driver](../devices/plaf103/motor-driver.md)).

## The startup report

Each build carries an identity that ties a capture to the exact sources it was built from. The SDK wrapper hashes canonical JSON containing the profile and the complete root-relative map of source-input hashes. That hash is `startup_identity` in the manifest and `WELLFED_BUILD_ID` in the firmware. It excludes timestamps, and the final OTA hash stays a separate identity for the artifact. Compiling the port for the target requires a lowercase 64-digit ID; host tests use `host`.

A boot prints:

```text
PLFW1 start build=<64 lowercase hex digits> bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000
PLFW1 result diagnostic=pass backend=unopened
```

- **`bank`** comes from live RSIP mapping entry 0. Virtual `0x0c000000` mapped to `0x08006000` is bank 0, and to `0x08186000` bank 1. Any other mapping reports unknown.
- **`mmu`** is the raw mapping, and **`reset`** the factory-preserved reset flags. A zero reset word does not establish a particular physical reset cause.

`wellfed-audit startup` ([src/tools/wellfed-audit](../../src/tools/wellfed-audit)) checks a capture against its build. It verifies the frozen OTA image and manifest, the embedded identity, the finalized receive-only capture hashes, event coverage, the actual bank mapping, ordering and the observation interval. A missing or wrong build, faults, incomplete or malformed records, a repeated startup and each policy's own failures are distinct failing outcomes. It emits fixed fields, never arbitrary device text. With your own complete evidence bundle:

```sh
cargo run --locked -p wellfed-audit -- startup \
  --capture private/captures/normal-boot \
  --manifest private/verification/review/build-manifest.json \
  --image private/verification/review/image.bin \
  --minimum-seconds 60 \
  --output private/verification/startup-result.json
```

A passing report shows that this build started cleanly once. It does not show long-term uptime, persistent boot counters or the physical reset cause.

## Stopped outputs

The stopped state is the stock-derived STOP and OFF state. The initializer sets it up in an order that never lets the motor driver see a half-driven pair:

1. It enables and checks the LP GPIO clock and function bits, leaving unrelated controls alone (logged as `PLCLK1`, before and after).
2. With the interrupt mask held, it preloads the latches: motor PA13 and PA14 HIGH and HIGH, infrared PA15 and PB3 LOW and LOW.
3. It enables both motor pins as outputs in one direction-register write, so the driver never sees one pin driven while the other floats.
4. It verifies the masked values and directions, then restores the interrupt mask.

PB3 is also `SWD_CLK`. Before configuring it, startup clears `REG_SWD_PMUX_EN` bit 0, as stock does, and stopped readback requires that bit clear ([why](optical-inputs.md#pb3-is-swd_clk)).

Infrared OFF needs LOW output latches as well as LOW pad inputs. A pad with a dead input path reads LOW whatever is driven, as PA0–PA7 do on our unit, so the input alone proves nothing.

`PLFW1 outputs` reports the PA and PB values and directions before the startup banner. If any check fails, startup logs an error and does not start the diagnostic task. A synthetic diagnostic pass can never hide an output mismatch. The manifest's policies must match the record formats in the hashed image.

If you need to isolate the motor, note that the CON3 harness carries both motor leads and board power. Unplugging it is not an established way to cut the motor while keeping the board powered.

## Indicators at startup

Startup leaves every indicator dark. After the stopped outputs and a diagnostic pass, it verifies each light LOW, prints `PLLED1 begin low_only` and `PLLED1 done low=all`, and starts the firmware's tasks at once. This is policy `low-only-v1`, with GPIO routing policy `read-only-v3`, whose only stage is `startup`.

The lights themselves were proved by an earlier blinking probe, `bounded-probe-v3` ([bring-up probes](bring-up-probes.md#indicator-probe)). It cost every boot about 15 s before Wi-Fi, the device API and the OTA watcher could start, so it no longer runs. What the lights mean is the [indicator service](indicators.md).

The pins, all lit HIGH on our unit: **PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning**. Only the warning's red color is recorded. PA0, PA2 and PA4 have no usable input readback, so they are output-only; PB22 is read back in full.

## Shared-pad power

The audio-shared pads need power before they work as GPIO. Stock and the vendor SDK set this up, but a minimal KM0 main leaves it out. Our KM0 restores the AMACRO and analog pad enables and calls the SDK's calibrated `SOCPS_AudioLDO(ENABLE)` before it enters the scheduler. It does not initialize audio playback.

Two details matter:

- **Order.** Stock and the SDK power the pads after booting KM4. We do it before `km4_boot_on()`, so the pads are powered before KM4 configures them and `PLPWR1` completes before KM4 logs anything.
- **Trim.** `SOCPS_AudioLDO(ENABLE)` restores a voltage trim (bits 7:1) that only its disable path saves, so a cold start gets zero. Stock KM4 `CODEC_Init` and the SDK's `CODEC_Init` write 0x2A, or 0x68 on a 1.8 V supply. Our KM0 writes that trim after the enable.

With the trim applied, the low Port A pads still read LOW. Raw Port A input sampled after each supply step gave the same word every time, with PA0–PA7 already reading 0 before the first step. So the shared-pad supply does not explain the readback: those pads appear to have no usable input path on our unit. PA13, PA14 and PB22 read back correctly.

## What the UART records mean

Every record starts with a fixed prefix. The ones a current firmware prints:

| Prefix | Written by | Meaning |
| --- | --- | --- |
| `PLPWR1` | KM0 | Shared-pad supply controls before and after |
| `PLCLK1` | KM4 startup | GPIO clock and function enable, and what was preserved |
| `PLFW1` | KM4 startup | Stopped outputs, build identity, bank and reset, diagnostic result or errors |
| `PLIO1` | KM4 startup | Read-only pad-group, AON and GPIO routing observations |
| `PLDBG1` | Indicator checks | Requested level, latch, input, directions, stopped state, pad control |
| `PLLED1` | KM4 startup | Indicator sequence begin, results, terminal LOW or failure |
| `PLFLS1` | KM4 and KM0 | Boot handshake between the cores ([probes](bring-up-probes.md#boot-handshake)) |
| `PLWIF3` | Wi-Fi task | Join, keep, gateway and leave events ([link liveness](#link-liveness-without-internet-dns)) |
| `PLSUP1` | Watchdog feeder | Supervision start, stop, lease faults and hang reports |
| `PLFLT1` | KM4 fault handler | Hard-fault registers |
| `PLPNC1` | KM4 | Panic location |
| `PLOTA1`, `PLWCH1` | Update stage | Updates over the device API |
| `PLBAT1` | Power monitor | Power samples ([power](power.md#inputs)) |
| `PLBTN1` | Button probe | Raw button changes ([buttons](buttons.md#the-raw-probe)) |
| `PLKEY1` | Button service | Gestures ([buttons](buttons.md#the-service)) |

Notes:

- **`PLPWR1` policies.** `before-km4-v1` requires one intact record before any KM4 report. `v2` adds the LDO trim, and `v3` the raw Port A input after each supply step. An image linking the `v3` record format is refused unless its manifest declares `before-km4-v3`.
- **`PLIO1`.** `read-only-v2` adds KM4's view of the shared-pad supply control and the raw Port A input word.
- **`PLDBG1`.** `bounded-probe-v2` adds `highs=N/64`, the extra input samples ([probes](bring-up-probes.md#indicator-probe)).
- **`PLFLS1`.** Under `fw06-hello-v1` it carries HELLO, READY, an injected peer change and a terminal watch result. Under `fw06-park-v1`, `fw06-read-v1`, `fw06-write-v1`, `fw15-journal-v1`, `fw15-settings-v1` and `fw15-settings-v2` it carries only HELLO and READY.
- **`PLWIF3`.** Policy `wifi-gateway-v1`. It is the `PLWIF2` join records plus a `gateway` event per probe (result, gateway IPv4, time). A loss carries its cause: `0` association, `2` lease, `3` gateway. Kind 7 `dns` and loss cause `1` belong to the retired DNS check and are never reused ([probes](bring-up-probes.md#wi-fi-dns-check-retired)). An intentional restart first logs `kind=leave` with `wifi_disconnect`'s `result`, `value` 1 if still associated after 1 s, and `ms` ([below](#leaving-before-a-restart)). The normal firmware also logs `hold until=startup max_ms=180000` once and keeps the radio off until the firmware reports `startup_done`, or 180 s pass. Bank 0 waits the same way, since it runs the firmware and its probes.
- **`PLFLT1`.** A builder overlay of the vendor handler prints `cfsr`, `hfsr`, `mmfar`, `bfar`, the exception `lr` and whether the process stack was in use, before the vendor backtrace. CFSR's IMPRECISERR, or a peripheral address in BFAR, marks a bus fault the stacked PC does not point at.
- **`PLSUP1`.** `open watchdog=… trial=…` on its first step. `stop …` once when it stops feeding the watchdog (with `restart=1` for an intentional restart), or `stop watchdog_fault=…`. At boot, before anything arms: `lease_fault …` ([OTA design](ota-design.md)) and `hang …` after a silent watchdog reset ([below](#silent-watchdog-resets)), then `trail …` with its `trail_mark …` and `trail_step …` lines, or `trail refused=empty|boot`, after a `masked` or `spin` hang ([the call trail](#the-call-trail)).
- **`PLPNC1`.** `panic file=… line=… col=…`, with the path's last 95 bytes, printed after the outputs are stopped and before the halt that the watchdog resets. It goes to the UART only: no backup register is free to carry it across the reset.
- **`PLOTA1` and `PLWCH1`.** Policy `device-api-v1`, from `ota::update`. At the stage's start: `update store=… min_version=… attempt=…`, then `bank0 bytes=… recorded=…` or `refused reason=…`. On bank 1: `PLWCH1 offer=1 version=…` and `give_way=1 version=… restart=1`, or `error stage=give_way`. On bank 0: `manifest version=… bytes=…`, `progress sectors=N/TOTAL` every 16 sectors, `activated version=… bytes=… sectors=…`, `error stage=…` (timeout, cancelled, order, image, write, hash, marker, store), `key retired=…` or `added=…`, and `restart watchdog=1`.

Records from retired probes (`PLTMR1`, `PLWDG1`, `PLPRK1`, `PLRDS1`, `PLWRS1`, `PLJNL1`, `PLSET1`, `PLADC1`, `PLWIF1`, `PLWIF2`, `PLMQT1`, `PLOPT1`) are described in [bring-up probes](bring-up-probes.md).

## Link liveness without internet DNS

The feeder is a local device, so it judges its network link only by what the LAN shows: the association, the DHCP lease and the gateway that lease named. Nothing in the firmware looks up a name. The Wi-Fi shim calls no resolver. lwIP's resolver is still linked, because the vendor `ping` console command reaches `lwip_gethostbyname`, `netconn_gethostbyname` and `dns_gethostbyname_addrtype`, and it takes the DHCP server's address. But it sends only when asked, and no firmware path asks.

The Wi-Fi task in C samples the link every 500 ms and calls `wellfed_wifi_watch(associated, leased, probe)` in `wellfed_hal::link`. The answer is `wellfed_core::liveness::Verdict::code`: hold, probe now, settled, or leave with a cause. The state machine (`Liveness`) is pure and host-tested in `wellfed-core/tests/liveness.rs`. `wellfed_wifi_outcome` starts a fresh one after every successful join. The C task is outside the stack audit, as the select and outcome calls already were.

| Check | How | Loss |
| --- | --- | --- |
| Association | `wifi_is_connected_to_ap` | Leave at once, cause 0 |
| Lease | `dhcp_supplied_address` and a nonzero address | Leave at once, cause 2 |
| Gateway | One ICMP echo every 5 s | Three unanswered in a row, cause 3 |
| Restart | A restart was requested | Leave, cause 4 ([below](#leaving-before-a-restart)) |

- **Lease.** lwIP renews and rebinds by itself. It gives the address up only when the whole lease ran out unanswered.
- **Gateway.** The echo goes over a raw socket (`LWIP_RAW` is on in the SDK's options) with a 1 s timeout. The reply must match on source, type, identifier and sequence. Once the gateway has answered during this join, three unanswered probes in a row (11–16 s of silence) are a loss. That catches an association that looks up while nothing gets through, which the driver's beacon-loss detection misses.
- **A gateway that never answers** this join is taken to refuse echo, since DHCP just worked over the same link. The link then rests on association and lease alone.
- **Why not ARP.** No router can refuse ARP, but lwIP 2.0.2 here runs without core locking, so a request would have to be posted to the stack's thread. Its table also cannot tell a fresh reply from a cached entry.
- **Settled.** Once per join, on the gateway's first answer or after three probes without one, the shim resets its rejoin backoff. So a network that drops the feeder right after DHCP backs off to 30 s instead of rejoining every second.
- **Live.** `net_status::Status::live` is written only by the watch: settled and, once the gateway has answered, its latest probe answered too. Trial confirmation waits for it. A leave clears it before the loss event is drained.

Records are `kind=gateway` events. `result` is 0 answered, `-1` no gateway in the lease, `-2` socket, `-3` send, `-4` timeout. Only a timeout counts as silence: a probe that could not be sent, as when the device API's sessions and mDNS hold every connection, is no observation. `value` is the gateway and `ms` the round trip. A loss is `kind=lost` with `result` 0, 2 or 3. The device API's `dns_losses` and the health record's `"dns"` keep their names on the wire and now count causes 2 and 3.

If your network has no internet route, that is fine: the watch never asks beyond the gateway.

## Leaving before a restart

Every intentional restart leaves the access point first. That covers an activation, a give-way, `request_reset` for a reboot the device API or setup asked for, and a trial variant's hang. Each is a watchdog reset. Without a leave, the access point still holds the old boot's PMF-protected association. The next boot's WPA3 SAE authentication then succeeds but the association is rejected with status 30. After that reject, the vendor's `wifi_connect` sometimes never returns, and the whole of KM4 stalls into a silent reset 4 s later. A trial boot right after an OTA restart nearly always meets status 30, so this could spend a good build's three trial boots.

How the leave works:

- **Order.** The runtime's `restart()` first calls `wellfed_hal::supervision::stopping()`. That sets `wellfed_hal::link`'s restart flag and marks the breadcrumb. Then it stops the watchdog feeder as before. The reset never waits for the leave: if the disconnect hangs or the task is busy, the reset comes when it always did.
- **Supervision stops leave too, once certain.** A stop that `supervise::feed` decides, such as a trial's 10-minute deadline or a hung task, is not a latch. Feeding resumes if the condition turns true within the watchdog's 1 s progress gap, so a sub-second false costs nothing and must not take the network down. `supervision::supervised` calls `stopping()` only once the stop has lasted the whole gap, when the reset is certain. A refused checkpoint calls it at once. An unconfirmed trial boot then leaves, and its next trial boot does not meet status 30.
- **The Wi-Fi task.** The shim checks `wellfed_wifi_restarting()` every 50 ms of its watch wait. The next `wellfed_wifi_watch` answers leave with cause 4 (`Cause::Restart`) whatever the link shows. The task stops DHCP, calls `wifi_disconnect()` from its own context (the driver queues a protected deauthentication), waits up to 1 s for `wifi_is_connected_to_ap` to report the association gone, logs `kind=leave` and parks. It joins no more, between attempts or after a join's turn waited.
- **Inside an attempt** it checks twice more. After the scan (up to 10 s), so a restart asked for meanwhile never calls `wifi_connect`. And right after a successful `wifi_connect`, so an association made while the restart was pending is left at once with the same disconnect, wait and `kind=leave` event, and DHCP never runs.
- **Callers.** Every reboot the device API or setup asks for already waits for its reply to go out (`reboot_at`, `Setup::reboot_due`). The update stage waits for its answer (`LEAVE_MS`). So disconnecting at the request loses no answer.

The timing is tight but fits:

- **A requested restart.** From the request to the end of the wait takes at most about 2 s: a gateway echo in flight (1 s), then the 1 s wait. The watchdog's shortest expiry less a feed period is 2.18 s at the clock's 54 kHz bound (`config::WATCHDOG`), and about 3.75 s at the measured 4.0 s expiry.
- **A supervision stop.** The decision comes on the feeder pass 1 s after the stop began, about 1.25 s after the last refresh (passes are 250 ms apart). That leaves about 2.75 s at the measured 4.0 s expiry, but only about 1.2 s at the 54 kHz bound (2.43 s shortest). A leave takes a 50 ms wake plus the disconnect, up to about 2 s with an echo in flight. It fits the measured expiry and may not fit the bound, where the disconnect is best effort.

If a restart comes while `wifi_connect` or `LwIP_DHCP` runs, the leave waits for that call to return. If the reset comes first, the association it was making may outlive the boot.

## Silent watchdog resets

A hang that stops the watchdog feeder task itself prints nothing: no `PLSUP1 stop`, just the ROM banner about 4 s later and `reset=00100000`. To explain such a reset, the FreeRTOS tick hook leaves a breadcrumb every tick in backup register 2, which survives the reset. The builder sets `configUSE_TICK_HOOK`, and the shim's `vApplicationTickHook` calls `wellfed_tick_hook` (`wellfed_hal::supervision::crumb`). The next boot prints the crumb once, before anything else runs:

```text
PLSUP1 hang kind=spin task=wellfed-wif since_ms=3968 suspended=0
```

- **`task`** is the task the last tick interrupted. It prints as the name's first three characters in lower case, or the three after the shared `wellfed-` or `wellfed_` prefix. `wellfed-svc` is the service task, `wellfed-wdg` the watchdog feeder, `wellfed-loo` the feeder's loop, `wellfed-wif` the Wi-Fi task and `wellfed` the diagnostic task (a name of exactly `wellfed-` prints the same). `tmr` is the timer service, `idl` idle, `cmd` and `rtw` the Wi-Fi driver's threads, and `none` means no task yet. The builder raises `configMAX_TASK_NAME_LEN` from the vendor's 10 to 16 so the suffix survives.
- **`since_ms`** is how long since the watchdog feeder task last ran, in 128 ms steps. 8064 means that long or longer. The feeder runs every 250 ms.
- **`kind`** is `spin` when `since_ms` is 2048 or more: the tick kept running without the feeder, so a task kept the CPU, and `task` names it (`suspended=1` if the scheduler was suspended). It is `masked` when less: the tick stopped while the feeder still ran, so interrupts were masked or the tick itself stopped, and `task` names what was running then. It is `stopped` when the feeder had stopped feeding on purpose; the line says what ran until the reset.
- **A `spin` naming `idl`** means the CPU was free. The feeder task was blocked, not starved, so look for what it waits on rather than a busy task.

Limits:

- **Only after a KM4 watchdog reset** (`BOOT_KM4_WATCHDOG`). Any other boot clears the register silently, and a cold boot finds it zero.
- **Tickless idle.** The SDK runs with `configUSE_TICKLESS_IDLE` 1. While every task sleeps, idle stops the tick and FreeRTOS steps the count afterwards without calling the hook. Suppressed ticks are not counted, so `since_ms` can read short when the system was mostly idle. A spin keeps the CPU busy, so it never suppresses ticks.
- **Inside a flash lease** no tick runs, because the lease masks interrupts. A hang there is reported by the lease fault's line instead.
- **Cost.** A few loads and one backup-register write per 1 ms tick.

Register 2 was free because the SDK leaves registers 1–5 to the application and the others are taken: 1 holds the flash attempt and update bits with the lease fault, 3 and 4 the two cores' boot counts (4 also the trial), and 5 KM0's heartbeat. Register 2 once held KM0's park-handler diagnostics, which nothing read, and KM0 no longer writes it. The crumb word is a 3-bit tag, the name in 6-bit characters, the two flags, `since` and a 2-bit check, so a stray value is rarely taken for a crumb.

### The call trail

The crumb names the task but not where it stalled. So the KM4 image also keeps a call trail in RAM a watchdog reset leaves alone: the last 64 calls into a few vendor primitives, and the last call of each rarer kind. After a `PLSUP1 hang` line of kind `masked` or `spin`, the next boot prints the trail once:

```text
PLSUP1 trail boot=41 count=184211 n=64 skipped=0 depth=1 last=save_and_cli
PLSUP1 trail_mark mark=connect seq=180002 step=wifi_connect depth=0 isr=0 lr=0x00000000 arg=0x00000001
PLSUP1 trail_mark mark=connect_ret none
PLSUP1 trail_mark mark=indication seq=184001 step=wifi_indication_ret depth=0 isr=0 lr=0x0e05ae01 arg=0x00000001
PLSUP1 trail_mark mark=disconnect seq=183990 step=rtw_os_indicate_disconnect depth=0 isr=0 lr=0x0e038b71 arg=0x10035000
PLSUP1 trail_mark mark=timer seq=184120 step=rtw_del_timer depth=0 isr=0 lr=0x0e05ac91 arg=0x10030510
PLSUP1 trail_step seq=184147 step=save_and_cli depth=1 isr=0 lr=0x0e05b2a7 arg=0x00000000
…
PLSUP1 trail_step seq=184210 step=save_and_cli depth=1 isr=0 lr=0x0e05b2a7 arg=0x00000000
```

- **`boot`** is the KM4 boot count the trail started with. **`count`** is how many notes the boot made, and **`n`** how many of the last 64 are shown. **`skipped`** counts shown slots that held no whole note, such as one a reset cut in the middle. With fewer than 64 notes, `n` is simply smaller. **`depth`** is how many wrapped critical sections were open at the end, and **`last`** is the newest note's step.
- **`trail_mark`** lines show the last note of each rarer kind, which the ring has usually lost. The driver masks interrupts with `save_and_cli` around every packet buffer it allocates or frees, so the ring turns over many times during a join. The marks are the last `wifi_connect` call (`connect`), its return (`connect_ret`), the last `wifi_indication` (`indication`), the last `rtw_os_indicate_disconnect` (`disconnect`) and the last timer call (`timer`), each as on a `trail_step` line, or `none`.
- **`trail_step`** lines show the ring, oldest note first. `step` is the call. `save_and_cli` is noted once it has masked interrupts, `restore_flags` just before it unmasks, and `rtw_set_timer`, `rtw_del_timer` and `rtw_cancel_timer` before they run. `wifi_indication` and `rtw_os_indicate_disconnect` are noted on entry and again with `_ret` when they return. `wifi_connect` and `wifi_connect_ret` come from the Wi-Fi task, around its join.
- **`lr`** is the return address in the function that made the call, with the Thumb bit set. A vendor function that ends in a tail call to a wrapped primitive leaves its own caller's address instead. Look it up in the `target_img2.axf` of the build that hung (`arm-none-eabi-addr2line -f -e target_img2.axf`, or the sorted `target_img2.map`). The Wi-Fi task's own steps print 0.
- **`arg`** is the call's first argument: the timer, the indication's event or the adapter. `save_and_cli` and `restore_flags` take none and print 0. For `wifi_connect` it is the join attempt, and for `wifi_connect_ret` the result.
- **`depth`** on a step counts the open wrapped critical sections, this one included (at most 127). It is one counter for the whole CPU, so a section an interrupt opens inside a task's counts too. **`isr=1`** means the note came from an interrupt handler.
- **`seq`** is the note's number since the trail started. It wraps at 2^32 notes; right after a wrap, the ring shows only the notes since.

A masked stall inside a wrapped critical section ends the ring at that section's entry, and its `lr` names the function that masked interrupts. If the `connect` mark is newer than `connect_ret`, or `connect_ret` is `none`, the join never returned.

If the RAM holds no trail of the boot just before, the line says why instead: `PLSUP1 trail refused=empty` (none was started, as after a power cut) or `PLSUP1 trail refused=boot boot=… current=…` (another boot's).

How it works:

- **Wraps.** The builder links the KM4 image with `-Wl,--wrap` for `save_and_cli`, `restore_flags`, `rtw_set_timer`, `rtw_del_timer`, `rtw_cancel_timer`, `wifi_indication` and `rtw_os_indicate_disconnect`. So every call to one of them from another object, the prebuilt Wi-Fi driver's included, goes through a wrapper in `example_entry.c`. The wrapper notes the call, then calls the real function. KM0 links no wraps.
- **The note.** It masks nothing on the ring's path. It claims its ring slot by incrementing the count with LDREX/STREX, which retries if an interrupt lands in between, then fills the slot with plain stores. A note from an interrupt that lands mid-note takes the next slot. A note of a rarer kind also rewrites its mark, with interrupts masked for those few stores (PRIMASK saved and restored), so no interrupt or higher-priority task can mix two notes in a mark, and it leaves a newer note it finds there, so a mark holds its kind's latest. That short section is only on the rarer kinds, never on a critical section's own note. Only the critical-section wrappers change `depth`, and they do so while interrupts are already masked. A note never blocks, allocates or prints. The wrappers run from RAM, so wrapping adds no flash access.
- **Where it lives.** 220 words (880 bytes) at `0x1007BC50`, just below the wall clock's time carry at `0x1007BFC0` ([clock](clock.md)). The builder's KM4 linker script takes them from the heap's end and names them `wellfed_status_trail`. Its size is one builder constant, which a test checks against the shim's `WELLFED_TRAIL_WORDS`, and the C checks its struct against that define. The build's audit pins the address with the time carry's and `__sram_end__`. A warm reset leaves this RAM alone, for the reasons the time carry gives. Each store reaches the RAM before a reset, because the D-cache writes through there. The ROM's non-secure `mpu_init` enables the MPU with the default memory map as background (`PRIVDEFENA`) and no region of its own. None of the regions `app_mpu_nocache_init` adds covers this address, so it takes the default map's Code-region attributes: Normal, write-through, read-allocate. The SDK's cache header says the same.
- **Every boot starts one.** At startup, right after the crumb, the runtime reads the last boot's trail where it lies, word by word, then starts an empty one (`wellfed_hal::supervision::trail::take`). No copy goes on the startup stack, which before the scheduler is the main stack the boot loader set up. Nothing is noted before that: the switch lives in `.bss`, which startup clears, so the SDK's own startup cannot overwrite the trail before it is read. The trail prints only after a hang the crumb names; any other boot just starts afresh.
- **Tied to its boot.** The start writes the KM4 boot count (backup register 3) into the header and folds it into the check. KM4 counts a boot when it arms the flash hooks, after the trail has started. So the next boot reads the trail only when its count is one more than the trail's.
- **Layout.** The header is the magic `0x504C5452` ("PLTR"), the boot count, the check `~(magic ^ boot ^ version)` with layout version 3, the note count, the open depth and three reserved words. Then come 5 marks of four words (the note's whole number, then the note's three words) and 64 entries of three words: the step, the depth from bit 8, the interrupt bit at 15 and the note's number's low 16 bits from bit 16, then `lr`, then `arg`. Note `n` goes to slot `n % 64`. The shim claims a slot first and stores the numbered word last. So a reset mid-note leaves the slot with the number it held a ring earlier, which the reader drops. A mark's first note word is cleared first and its whole number stored last, so a reset in the middle of a mark leaves one the reader drops. The magic and check refuse a trail of another layout version.

Limits:

- **Calls inside the defining object are not seen.** `--wrap` redirects only undefined references. `rtw_set_timer`, `rtw_del_timer` and `rtw_cancel_timer` are defined in the driver's `rtk_wlan_if.o`, and that object's internal timer calls go unnoted: `rltk_set_mode_prehandle` calls `cancel_timer_ex` and `rltk_set_mode_posthandle` calls `mod_timer` directly. The same holds for `freertos_intfs.o`'s own calls to `rtw_os_indicate_disconnect`, and `wifi_ind.c`'s to `wifi_indication`. The osdep atomic operations in `freertos_service.o`, which the driver reaches through `osdep_service`, call `save_and_cli` inside the object that defines it, so their critical sections go unnoted.
- **FreeRTOS's own critical sections are not wrapped.** `taskENTER_CRITICAL` inside the kernel, and the driver's `rtw_enter_critical_bh` spinlocks, leave no note.
- **The driver's register-access sections are not wrapped.** `rtw_enter_critical` and `rtw_exit_critical` guard every driver register access (`bus_read` and `bus_write`), the WLAN interrupt, the receive and transmit tasklets and the command queue. They are left out on purpose: wrapping them stretched the first WPA3 join past the watchdog feeder's 1 s gap (observed, October 28, 2026). So a stall inside one leaves no note of its own, and shows as the last note before it.
- **A boot that never armed.** A boot that hangs before it arms the flash hooks, or skips arming (the first boot after a store write that never finished), does not count itself. The next boot then refuses its trail as `refused=boot`.
- **Power cuts.** A power-on leaves whatever the RAM holds. The magic and check refuse it, and a build without the trail uses that RAM as heap, which they refuse too.
- **Timing.** About 40 instructions per wrapped call, a few more for a rarer kind's mark (masked for about 10 instructions), and 880 bytes of heap. Each wrapped critical section's masked window grows by about 40 instructions, so the hang rate may shift.
- **Many interrupts at once.** If 64 or more interrupt notes land while one task note is half written, they lap its ring slot, and that slot may mix two notes. Marks cannot mix.

## Evidence

- **Startup report.** Observed: an artifact-bound startup passed `wellfed-audit startup` on the research unit. This does not establish long-term uptime, persistent boot counters or a verified physical reset cause.
- **Stopped outputs.** Observed: exact GPIO readback, an artifact-bound normal boot and operator-reported no motion. Independent voltage and infrared-off measurements, repeated measured resets and transient checks belong to the electrical output verification ([motor driver](../devices/plaf103/motor-driver.md)). The single direction-register enable passed 50 cold boots on the analyzer (observed, October 27).
- **PB3 and SWD.** Observed: before startup released SWD, PB3 was never driven, and its LOW readback was the pad's pull-down ([optical inputs](optical-inputs.md#pb3-is-swd_clk)).
- **Shared-pad power.** Observed: a capture with the new order showed an intact `PLPWR1` ahead of KM4 output with `after_pad=00000301`, the LDO powered with a zero trim. A later build with the trim and per-step Port A sampling read the same word at every step. The removed `trace-stock-pad-power.py` (in Git history) executed the stock initializer and LDO instructions against modeled registers and delays, and its fixtures distinguished LDO power from the pad-enable bit: recovered, not an electrical measurement.
- **Indicator pins and polarity.** Observed: with the shared-pad prerequisite restored, the operator saw all four indicators blink twice and finish dark, establishing HIGH-on and LOW-off. The output-only build then passed the auditor on a single boot: intact `PLPWR1` with trim 0x2A, PA0, PA2 and PA4 `status=output`, PB22 `pass`, terminal LOW. An earlier capture (HIGH latch with LOW Port A input, two operator boots, interleaved KM0 and KM4 reports) keeps its failing verdicts.
- **Radio hold.** Observed (`fw40-diag-01`, `fw40-diag-03-wifi`): the first WPA3 join starves lower-priority tasks for about 700 ms, past a startup probe's 300 ms owner lease. That is why the radio waits for `startup_done`.
- **Link liveness.** Recovered and modeled (October 25): static analysis and host tests; not yet run on hardware. On the bench hotspot, which has no forwarding, the old DNS check passed only because the hotspot answers DNS locally. The bench host's firewall accepts ICMP on every interface, so expect `kind=gateway result=0` every 5 s and a hotspot outage to show as an association or gateway loss. A run with forwarding blocked is still to come.
- **Status 30 hangs.** Observed: 24 of 441 status-30 rejects on our builds stalled KM4, 17 of 35 on the October 25 evening builds, and 8 of 8 on stock. Status 53 never did. The leave itself is recovered and modeled (October 25), not yet run on hardware. **Cause and fix (October 28, physical):** the [call trail](#the-call-trail) caught 9 hangs in 19 power cuts, each with the same end: the join-timeout teardown in the timer task was clearing the key table (`rtw_clearstakey_cmd`, then `ROM_WIFI_CAM_WRITE`, plain stores to the WLAN MAC at `0x4008067x`) just after the power-save processor's `ips_enter` had powered the MAC down. A store to the powered-down MAC stalls the bus, and the tick with it. The Wi-Fi task now turns inactive power save off after `wifi_on` (`wifi_set_power_mode(0, 1)`, leisure power save kept): 20 cuts, status 30 on each, no hang. The leave before a restart stays, since it saves a rejected join.
- **Silent-reset crumb.** Recovered and modeled (October 25). The first hardware crumbs all read `task=wellfed-`, because the vendor's 10-character name limit cut every runtime task's name; the builder raised it to 16 on October 26.
- **Call trail.** Recovered and modeled (October 28): the wrapped symbols are undefined in the prebuilt driver objects that call them (`nm` over `lib_wlan.a`), and host tests cover the decoding. Not yet run on hardware, so it is unconfirmed that the RAM keeps the trail across the hang's reset. A build that also wrapped `rtw_enter_critical` and `rtw_exit_critical` failed all three OTA trial boots (observed, October 28, capture `fw66-trail-20261028-02`): each stopped about 10 s in, during the first WPA3 join, with the watchdog feeder more than 1 s late, and KM0 fell back to bank 0.
