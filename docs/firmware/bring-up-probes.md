# Bring-up probes

Before the replacement firmware could feed, each piece of hardware was brought up by a probe: a bounded test that ran after startup, printed fixed UART records and then stopped. This page is the reference for those probes: what each one did, the records it printed, and what a passing capture looks like. It is for contributors reading old captures or rerunning a probe. Owners can skip it.

The probes still matter because `wellfed-audit startup` checks every startup policy below. Each policy is bound both ways to a record format in the image: a manifest that declares the policy needs the marker string in the image, and an image carrying the marker needs the policy in its manifest. Policies for different probes exclude each other, so an image is audited under exactly the probe it carries. The everyday startup is in [startup and diagnostics](startup-diagnostics.md).

Most probes no longer run in the normal firmware. The bring-up firmwares and the [QA firmware](qa.md) still run the optics, buttons, ADC, speaker and pulse probes from `src/sdk/wellfed-probes`.

## Indicator probe

Policy `bounded-probe-v3`, kept so older captures still audit. Startup now uses `low-only-v1` instead ([startup](startup-diagnostics.md#indicators-at-startup)).

The controller preloaded the indicator latches LOW and selected the stock-derived GPIO, no-pull configuration. It changed only PA0, PA2, PA4 and PB22. Every request needed valid indicator directions and stopped-output readback. Each request yielded for 10 ms, then observed the output latch, input, pad control and direction. Timing requests included that settle.

The sequence waited a nominal 3 s, then gave each pin two HIGH pulses, in PA0, PA2, PA4, PB22 order: HIGH 1 s, LOW 0.5 s, HIGH 1 s, LOW 1 s. All lines were then requested LOW and the task ended. There was no indefinite blinking, no feeding request and no autonomous retry.

- **Output-only pins.** PA0, PA2 and PA4 have no usable input readback on our unit, so their steps report `status=output` when latch, pad control, directions and stopped state agree. The begin line names them: `PLLED1 begin output_only=PA0,PA2,PA4`. `output` is never `pass`: it does not establish an electrical level, which rests on the operator's eyes. PB22 keeps full input verification.
- **Audit fields.** The sequence outcome is `pass`, but `led_probe_verified` stays `false` whenever output-only pins are present. `led_input_verified_pins` (`PB22`) and `led_output_only_pins` (`PA0`, `PA2`, `PA4`) say what was read back.
- **Extra samples.** 64 extra input samples (`highs=N/64` in `PLDBG1`) tell a dead input path (0/64) from a marginal threshold (a partial count). They are informational: verification uses the single input read, so a high count cannot verify a failed step.
- **Failures.** A HIGH input mismatch is a failed observation but allows the already bounded visible interval. A LOW mismatch, a changed latch, mux or direction, or a lost stopped state aborts. Losing stopped readback requests STOP and OFF again. `finish=input_mismatch low=all` is a failure, never a verified success.

## Supervision probe

Policy `fw05-probe-v1`, bound to the `PLTMR1` and `PLWDG1` records. It proved the stop timer and the watchdog with motion disabled.

After the indicator probe, a cold boot opened the stopped board and TIM2, then:

1. Logged 13 `PLTMR1 sample` snapshots: SYSTIMER raw, supervision ms, accepted TIM2 interrupts, and ready, healthy and stopped.
2. Ran a 2 s busy stall.
3. Armed the watchdog (`PLWDG1 arm`) and fed it for 15 s (`fed`).
4. Withheld progress (`stall_begin`, `stalled`) until one watchdog reset.

The next boot repeated startup and the indicator probe, then printed only `PLWDG1 skip`. The startup auditor treats a two-boot capture as `repeated_startup`.

The probe opened only the board's outputs (motor STOP, emitters OFF). It never configured sensor inputs, read efuse calibration or touched the LP-domain ADC, and it requested no drive or emitter.

Reset-loop guards: before arming, it wrote a marker to SDK user backup register 1 and read it back. Any boot that saw a nonzero marker or boot reason cleared the marker and skipped the probe, so a reset could not repeat the watchdog test until power was removed. If both guards had failed, the board would have repeated the probe about every 100 s until power-off. The UART loader stays available at power-on.

Nominal clocks: SYSTIMER and TIM2 at 32 768 Hz (SDK: 31 µs ticks), and a watchdog of 4095 × 32 counts at about 33.3 kHz (`WDG_Scalar`), bounded to 2.9–5.3 s by 25–45 kHz limits. The capture measured them.

## Boot handshake

Policy `fw06-hello-v1`, bound to the HELLO record format. It needs the indicator policy and exactly one fully parsed HELLO, READY, peer and done record, in that order.

After `PLLED1 done low=all`:

1. KM4 prints `PLFLS1 hello authority=<16 hex> count=<n>`. Its authority session is the boot count in the high word and a fresh low word.
2. KM0 answers READY with its own session. KM4 binds a flash coordinator only while outputs are stopped and prints `PLFLS1 ready peer=<16 hex> reply=0 phase=1 fault=0 inactive=1` (ACCEPTED, IDLE, no fault).
3. The test build's KM0 republishes a new session about 10 s later, simulating a rebooted peer. KM4 reports `PLFLS1 peer peer=<16 hex> reply=5 phase=8 fault=7 blocked=1` (FAILED, FAULT, PEER_LOST), with the new session's high word one above the first.
4. After a 30 s watch from READY it prints `PLFLS1 done phase=8 fault=7 blocked=1`.

One `PLFLS1 foreign` line may appear after HELLO. Failures print `PLFLS1 error timeout ms=<n>` or `PLFLS1 error clock`.

The audit checks the session and field values and the host receive times: READY within 2.5 s of HELLO, the peer change 9–12 s after READY, done 29–32 s after READY. Outcomes are `flash_handshake_missing`, `flash_handshake_malformed`, `flash_handshake_failed`, `flash_handshake_order` and `flash_handshake_timing`. Session values are reported as fixed hex fields. This is UART protocol evidence; it does not show that any flash write path is safe.

## Park round trip

Policy `fw06-park-v1`, bound to `PLPRK1 start mode=` and still requiring the HELLO format. The startup auditor checks only each boot's handshake: one HELLO and one READY (one `PLFLS1 foreign` allowed) after `PLLED1 done low=all`, the `fw06-hello-v1` field values, READY within 2.5 s of HELLO. It refuses `PLFLS1 peer` and `PLFLS1 done` and requires a `PLPRK1 start` after READY.

After READY, KM4 prints `PLPRK1 start mode=<0|1|2> marker=<hex8> watchdog=<pass|fail>` and four `PLPRK1 batch leases=<n> park_max=<ticks> release_max=<ticks> frozen=<n> alive=<n>` records at 25, 50, 75 and 100 leases. Maxima are 32 768 Hz ticks, bounded by the 40 ms exchange limit (1310). A passing capture is one power-on with exactly three startups:

1. **Mode 0** injects `PLPRK1 inject mode=0 lease=101 begin=0 ... ms=0`: KM4 stops acknowledging a park, and the watchdog resets 2–6 s later.
2. **Mode 1** injects `inject mode=1 lease=101 begin=1 ... ms=0` and withholds the release. KM0 publishes TIMEOUT (`peer=504b0081`) after its 700 ms park budget, reported as a second inject line with `ms` within 650–760. The watchdog resets 2–6 s after the first inject line.
3. **Mode 2** prints `PLPRK1 done leases=100 marker=00000002`, then nothing more from `PLPRK1` or `PLFLS1` for at least 20 s.

Every batch needs frozen equal to leases and alive at least leases − 1. HELLO counts must be 1, 2 and 3, and the KM0 peer session high words nonzero and strictly increasing. Failures print `PLPRK1 error lease=... stage=... reply=... phase=... fault=...`, `PLPRK1 error no_reset ms=<n>` or `PLPRK1 skip marker=<hex8>`.

## Mediated flash read

Policy `fw06-read-v1`, bound to `PLRDS1 start marker=` and still requiring the HELLO format. A read image cannot be audited under `fw06-hello-v1` or `fw06-park-v1`, nor the reverse. It is a single-boot policy: a reset shows up as `repeated_startup`. HELLO and READY are checked as under `fw06-park-v1`. No `PLPRK1`, `PLFLS1 peer` or `PLFLS1 done` may appear.

After READY, KM4 prints `PLRDS1 start marker=<hex8> watchdog=<pass|fail> installed=<0|1>`, four `PLRDS1 batch reads=<n> id=<6 hex> mismatches=<n> park_max=<ticks> release_max=<ticks> frozen=<n> alive=<n>` records at 25, 50, 75 and 100 reads, and `PLRDS1 done reads=<n> leases=<n> phase=<n> fault=<n>`. Failures print `PLRDS1 skip marker=<hex8>` or `PLFLS1 error ...`.

A pass needs:

- `marker=00000000 watchdog=pass installed=1`;
- every batch with JEDEC ID `c84017` (the research unit's GigaDevice part), no mismatches, frozen equal to reads, alive at least reads − 1, and park and release maxima in 1–1310 ticks;
- `done reads=100 leases=100 phase=1 fault=0` within 30 s of start;
- at least 30 s of capture after done with no more `PLRDS1` or `PLFLS1` output.

Outcomes are `flash_read_missing`, `flash_read_malformed` (unparsed or repeated `PLRDS1` records, any `PLPRK1`), `flash_read_failed`, `flash_read_order` and `flash_read_timing`. HELLO and READY problems keep the `flash_handshake_*` outcomes. The report adds `flash_read_verified`, `flash_read_id`, `flash_read_reads`, `flash_read_leases`, `flash_read_park_max`, `flash_read_release_max`, `flash_read_frozen`, `flash_read_alive`, `flash_read_seconds` (start to done) and `flash_read_tail_seconds`. This is protocol and timing evidence for the mediated read; it does not show that any write path is safe.

## Scratch-sector write

Policy `fw06-write-v1` covers one owner-approved 4 KiB scratch sector at flash offset `0x185000`. It is bound to `PLWRS1 start marker=` and still requires the HELLO format. A write image cannot be audited under `fw06-hello-v1`, `fw06-park-v1` or `fw06-read-v1`, nor the reverse. Single boot; HELLO and READY as under `fw06-read-v1`. No `PLPRK1`, `PLRDS1`, `PLFLS1 peer` or `PLFLS1 done` may appear.

After READY, KM4 prints `PLWRS1 start marker=<hex8> watchdog=<pass|fail> installed=<0|1>`, `PLWRS1 preimage id=<6 hex> blank=<n>`, three `PLWRS1 cycle n=<n> erase_ticks=<ticks> blank=<n> program_max=<ticks> errors=<n>` records, `PLWRS1 restore erase_ticks=<ticks> blank=<n>` and `PLWRS1 done cycles=<n> leases=<n> phase=<n> fault=<n> park_max=<ticks> release_max=<ticks> frozen=<n>`. Ticks are 32 768 Hz. Failures print `PLWRS1 skip marker=<hex8>` or `PLFLS1 error ...`.

A pass needs:

- `marker=00000000 watchdog=pass installed=1`, and a preimage with JEDEC ID `c84017` and all 1024 sampled words blank;
- cycles 1, 2 and 3 in order, each with `blank=1024`, `errors=0`, erase in 1–16384 ticks (the 500 ms lease active budget) and program maximum in 1–1638 ticks (50 ms);
- a restore erase in 1–16384 ticks leaving `blank=1024`;
- `done cycles=3 leases=53 phase=1 fault=0 frozen=53` with park and release maxima in 1–1310 ticks, within 60 s of start;
- at least 30 s of capture after done with no more `PLWRS1` or `PLFLS1` output.

Outcomes are `flash_write_missing` (no start, preimage, restore or done), `flash_write_malformed` (unparsed or repeated `PLWRS1`, any `PLPRK1` or `PLRDS1`), `flash_write_failed` (skip, error or any field outside its bounds, including a non-blank preimage or a wrong ID), `flash_write_order` and `flash_write_timing`. HELLO and READY problems keep the `flash_handshake_*` outcomes. The report adds `flash_write_verified`, `flash_write_id`, `flash_write_preimage_blank`, `flash_write_cycles`, `flash_write_restore_erase_ticks`, `flash_write_restore_blank`, `flash_write_leases`, `flash_write_park_max`, `flash_write_release_max`, `flash_write_frozen`, `flash_write_erase_ms_max` (largest cycle or restore erase), `flash_write_seconds` (start to done) and `flash_write_tail_seconds`. This covers one scratch sector; it does not show that writes elsewhere, or under power loss, are safe.

## Journal on NOR

Policy `fw15-journal-v1` covers a journal in the owner-approved 64 KiB region `0x340000`–`0x350000` (16 sectors). It is bound to `PLJNL1 start mode=` and still requires the HELLO format. A journal image cannot be audited under any `fw06-*` policy, nor the reverse. The startup auditor checks only each boot's handshake, as under `fw06-park-v1`, and requires a `PLJNL1 start` after READY. No `PLPRK1`, `PLRDS1`, `PLWRS1`, `PLFLS1 peer` or `PLFLS1 done` may appear.

After READY, KM4 prints:

- `PLJNL1 start mode=<0|1|2> marker=<hex8> watchdog=<pass|fail> installed=<0|1>`;
- in mode 0, `PLJNL1 preimage id=<6 hex> blank_sectors=<n>`;
- `PLJNL1 op kind=<n> id=<n> ok=<0|1> error=<n> ticks=<n>`, where kind 0 is create, 1 reserve, 2 finish, 3 open, 4 torn and 5 wipe;
- `PLJNL1 view sequence=<n> high_water=<n> pending=<0|1> result=<n> outcome=<n> slot=<n>`;
- in modes 1 and 2, `PLJNL1 refuse id=<n> reserved=<0|1>`;
- `PLJNL1 end mode=<n> reset=<0|1> leases=<n> frozen=<n> erases=<n> phase=<n> fault=<n>`.

Ticks are 32 768 Hz. Every boot needs `watchdog=pass installed=1` and its mode as the marker. A passing capture is one power-on with exactly three startups:

1. **Mode 0** finds JEDEC ID `c84017` and all 16 sectors blank. It creates the journal, reserves and finishes ids 1 and 2, and reserves id 3. It prints the view `sequence=6 high_water=3 pending=1 result=3 outcome=0 slot=1` and `end mode=0 reset=1 erases=5 phase=1 fault=0`. The watchdog resets 2.0–6.5 s later.
2. **Mode 1** opens the journal, prints the same view, refuses id 4 (`reserved=0`), tears the pending record and ends `mode=1 reset=1 erases=1 phase=1 fault=0`. The watchdog resets 2.0–6.5 s later.
3. **Mode 2** opens the journal, prints the same view and refusal, wipes the journal and ends `mode=2 reset=0 erases=2 phase=1 fault=0`, then nothing more from `PLJNL1` or `PLFLS1` for at least 30 s.

Every operation needs `ok=1 error=0`. Reserve, finish and wipe need nonzero ticks. Reserve and finish are bounded by 26214 ticks (800 ms), since one journal write is an erase, two programs and verify reads, each in its own lease. Each end needs nonzero leases with frozen equal to leases. HELLO counts must be 1, 2 and 3, and the KM0 peer session high words nonzero and strictly increasing. Failures print `PLJNL1 skip marker=<hex8>` or `PLFLS1 error ...`.

## Settings power-cut test

Policies `fw15-settings-v1` and `fw15-settings-v2` are bound to `PLSET1 start marker=` and still require the HELLO format. A settings image cannot be audited under any other flash policy, nor the reverse. The startup auditor checks only a boot's handshake, as under `fw15-journal-v1`, and requires a `PLSET1 start` after READY. No `PLPRK1`, `PLRDS1`, `PLWRS1`, `PLJNL1`, `PLFLS1 peer` or `PLFLS1 done` may appear.

After READY, KM4 prints `PLSET1 start marker=<hex8> watchdog=<pass|fail> installed=<0|1>` and `PLSET1 open status=<n> generation=<n> volume=<n> lights=<0|1> slot=<n> consistent=<0|1>`. Status is 0 LOADED, 1 EMPTY, 2 RECOVERED, 3 CORRUPT, 4 FUTURE, 5 IO or 6 WORN. It then saves settings and prints `PLSET1 batch saves=<n> generation=<n> ticks_max=<n>` after every 25 completed saves. In place of the last batch it prints `PLSET1 end saves=<n> generation=<n> leases=<n> frozen=<n> phase=<n> fault=<n>`.

| Policy | Saves | Pace | Open record |
| --- | --- | --- | --- |
| `fw15-settings-v1` | 300 | About 8 a second | No `other` field |
| `fw15-settings-v2` | 1500 | Back to back, about 35 a second | `other=<n>` before `consistent` |

`other` is the state of the slot not loaded: 1 ERASED, 2 VALID, 3 FUTURE, 4 DIRTY (a prefix of our record without its commit) or 5 INVALID. Failures print `PLSET1 error save generation=<n> status=<n>`, `PLSET1 skip marker=<hex8>` or `PLFLS1 error ...`.

The capture is one receive-only recording while the operator cuts and restores feeder power at moments of their choosing. Every `PLFW1 start` must report reset flags `00000000`. A boot cut before its open contributes nothing. A pass needs:

- at least three opened boots, each with `marker=00000000 watchdog=pass installed=1`, `consistent=1` and status LOADED or RECOVERED (EMPTY with generation 0 only for the first opened boot);
- each later open generation between the previous opened boot's highest reported generation and 25 more (up to 24 saves after its last batch, plus one interrupted save);
- batches at 25, 50, … in order, with generation equal to the open generation plus saves, and `ticks_max` nonzero and at most 26214 (800 ms);
- an end with every batch before it (11 or 59), generation equal to the open generation plus the save count, `phase=1 fault=0` and nonzero leases equal to frozen;
- no error or skip line anywhere, and at least two boots cut after their open, before an end, and followed by another startup;
- under `fw15-settings-v2` only, `other` on every open, agreeing with its status (LOADED: ERASED or VALID; RECOVERED: DIRTY or INVALID; EMPTY: ERASED), and at least one opened boot after the first finding it not VALID, meaning a cut inside a save. Under `fw15-settings-v1` no open may carry `other`.

## Power-input probe

Policies `fw14-adc-v1` and `fw14-adc-v2` are bound to `PLADC1 open outputs=` (v2 also to its ` supply=` field) and need the indicator policy. The `adc` bring-up firmware still runs it, sampling for as long as the build runs. It never binds the KM0 flash handshake, so the build confirms as usual. The [QA firmware](qa.md) takes five samples.

After startup, KM4 opens the board's outputs (motor STOP, emitters OFF, no flash authority, so every drive is refused) and then its sensors. It prints:

- `PLADC1 clock function_before=<hex8> clock_before=<hex8> function_after=<hex8> clock_after=<hex8> status=<pass|fail>`: `REG_LP_FUNC_EN1` and `REG_LP_CLK_CTRL1` around the explicit ADC enable (bit 8 in each, clock first, no other bit changed).
- `PLADC1 open outputs=<0|1> efuse=<hex8|none> offset=<n> gain=<n> default_offset=<0|1> default_gain=<0|1> status=<pass|fail>`: the efuse bytes 0x1d0..0x1d3 in read order and their decoding. Erased halves default to 2480 and 12050, as in stock.
- Once a second without end, `PLADC1 sample n=<n> ticks=<n> food=<0|1> outlet=<0|1> dc=<0|1> stopped=<0|1> current=<min,max,mean,scaled> battery=<min,max,mean,scaled>`. v2 appends ` supply=<min,max,mean,scaled>`. `ticks` times the whole capture (32 conversions, 48 under v2) against the SYSTIMER.
- `PLADC1 error n=<n> ticks=<n> stopped=<0|1>` on a capture timeout or failed stopped readback, which ends the probe with the board tripped.

`wellfed-audit adc` takes the usual arguments. It requires one passing startup, the clock and open records after `PLLED1 done low=all`, a verified enable, and efuse bytes that decode to the reported calibration. It then needs at least 30 samples numbered from 0, about a second apart (mean host spacing 0.8–1.5 s), each with `stopped=1`, `ticks` in 1–3277 (100 ms), `min <= mean <= max <= 4095`, and the scaled value equal to the stock formula on the mean. Under v2 every sample must carry the supply channel, and its mean must differ from both external channels' means by at least 1000 raw, which shows the channel select takes effect. Outcomes are `startup_*`, `adc_sequence`, `adc_clock`, `adc_open`, `adc_samples`, `adc_values`, `adc_channels`, `adc_timing` and `pass`. The report lists the registers, whether startup had already enabled the ADC, the calibration, the levels on each GPIO input and per-channel ranges. What the channels mean is in [power](power.md#inputs).

## Wi-Fi scan

Policy `fw07-scan-v1`, bound to `PLWIF1 start created=`, with the indicator policy. KM4 started the Wi-Fi shim in `example_entry.c`. One task ran `LwIP_Init()` and `wifi_on(RTW_MODE_STA)`, then three scans 5 s apart, each waiting at most 10 s and keeping the 32 strongest networks. The probe printed:

- `PLWIF1 start created=<0|1>` and `PLWIF1 on result=<n> ms=<n>`;
- per scan, `PLWIF1 scan n=<n> result=<n> ms=<n> count=<n> dropped=<n> stopped=<0|1>`, then `PLWIF1 ap n=<n> i=<n> ssid_crc=<hex8> rssi=<dBm> channel=<n> security=<hex8>` for each kept network;
- `PLWIF1 done scans=3`, or `PLWIF1 error timeout stage=<n> ms=<n>` from a stalled shim.

Networks are identified only by a CRC-32 of the SSID, and the build held no credentials. Nothing joined a network. `count` is the networks kept (at most 32) and `dropped` those beyond. KM0 did not start the WLAN firmware in these builds.

KM4 log lines print with the scheduler suspended when called from a task, so another task's print cannot split them; interrupts still run.

**The write audit.** The build sets `CONFIG_EXAMPLE_WLAN_FAST_CONNECT` to 0, because fast connect saves reconnect data to `FAST_RECONNECT_DATA` (0x105000), inside the application bank. The write-reachability audit (`wellfed-build`'s `audit/writes.rs`) fails the build if any eFuse or flash writer is linked: `rtw_efuse_boot_write`, `wifi_set_mac_address`, the fast-connect and FTL writers, the mbed flash write API, or any shell entry. It also fails if any reference to a ROM or RAM eFuse or flash write entry differs from its pinned set. That set is the RAM `*XIP` wrappers, the unreferenced `FLASH_EreaseDwordsXIP`, the driver's private-command eFuse path (`rtw_private_handler`, then `rtw_ex_set`, then `rtw_config_map_write`), and the vendor shell command table, which no linked code starts. It sees direct and stored-address references only.

## Wi-Fi join

Policy `fw07-join-v1`, bound to `PLWIF2 start created=`, with the indicator policy. The shim ran `LwIP_Init()` and `wifi_on(RTW_MODE_STA)`, then repeated one attempt:

1. Scan for the configured SSID, learning its security and RSSI.
2. `wifi_connect`.
3. `LwIP_DHCP(0, DHCP_START)`. Any DHCP state other than assigned, including the SDK's static 192.168.1.80 fallback, stops DHCP, releases the address and disconnects.

A joined link is polled every 500 ms. When it drops, the shim records the loss, stops DHCP and releases the address. Between attempts it waits 1 s, doubling to 30 s, and resets to 1 s after a join. Nothing resets, writes flash or registers the SDK's own reconnect. Events go to a 16-entry numbered ring that the Rust side drains, printing `PLWIF2 ev seq=<n> kind=<on|scan|connect|dhcp|lost|backoff> attempt=<n> result=<n> value=<hex8> rssi=<dBm> security=<hex8> ms=<n> stopped=<0|1>`, or `PLWIF2 error overrun seq=<n>`. The everyday firmware keeps this join loop and adds the [gateway watch](startup-diagnostics.md#link-liveness-without-internet-dns).

The SSID and password came from the ignored `private/hotspot.json`, which `wellfed-build` turned into an ignored generated header in the SDK worktree. Only the file's hash reaches the manifest, and our records never print either value. The SDK driver does print the SSID (`set ssid [...]`), so join captures stay private.

**The WLAN firmware runs on KM0.** On AmebaD it comes from `lib_wifi_fw.a` through `wifi_FW_init_ram()`, which starts `TaskInit_8721D` and the IPC command task `TaskFW_IPCMD_8721D`. Without it the driver's firmware commands are never consumed. `wellfed_wifi_fw_start()` calls `wifi_FW_init_ram()` when `BKUP_Read(0) & BIT_WIFI_ENABLE`, the same gate the vendor main uses (`app_start` sets it from `ps_config.km0_config_wifi_enable`), before `km4_boot_on()`. The first-boot main audit (`audit/mains.rs`) pins the call. It creates three KM0 tasks, about 6 KB of stack from the 20 KB KM0 heap. A park runs with interrupts masked, so none of that firmware's code runs while KM4 holds a flash lease, though it stalls for up to the park budget. The write audit covers KM0 too: `lib_wifi_fw.a` imports no flash, eFuse or shell symbol, every KM0 reference to a flash or eFuse write entry is pinned (the SDK's boot-time flash setup and the shell table), and nothing references the ROM shell entries. The firmware's ROM half is not covered.

The vendor makefiles track no header dependencies, so the build records the platform options' override digest and deletes every KM4 object when it changes.

## Wi-Fi DNS check (retired)

Policy `fw07-dns-v1`, bound to `PLWIF3 start created=`, with the indicator policy. The startup auditor still reads it. It was replaced by the [gateway watch](startup-diagnostics.md#link-liveness-without-internet-dns) because a feeder without a route to the internet churned rejoins under it.

The shim joined as under `fw07-join-v1`. Once it held a lease, it sent its own A query for `example.com` to the DHCP-provided DNS server (`dns_getserver(0)`) every 5 s, over a UDP socket with a 3 s timeout. It did not use lwIP's resolver, whose cache answered lookups without any network traffic. A lookup succeeded on a response with the query's ID, no truncation, rcode 0 and an A record. Failures were -1 (no server), -2 (socket), -3 (send), -4 (timeout), -5 (mismatch or truncation), -6 (rcode) and -7 (no address).

Two consecutive failures were a DNS loss (`lost` with result 1): the shim stopped DHCP, released the address, disconnected and rejoined. A link drop was a loss with result 0. The backoff reset only after a successful lookup, so a network that leased addresses but had no working DNS backed off to 30 s. Records were the `PLWIF2` set under the `PLWIF3` prefix, plus `kind=dns`.

## Health transport (deleted)

Policy `fw08-transport-v1`, bound to `PLMQT1 start created=`, needing `fw07-dns-v1`. A second task beside the Wi-Fi probe connected to the local server's MQTT broker once DNS worked and reported health. `PLMQT1` recorded task start, broker connect attempts, session ready, health publishes with their trigger, request outcomes, closes with their reason, or a config error. The transport was deleted on October 25. The builder records no transport policy, and the startup audit refuses an image with the start record.

## Optics probe

Policies `fw13-optics-v1` and `fw13-optics-v2`, bound to `PLOPT1 start created=`, with the indicator policy. Records, plans and results are in [optical inputs](optical-inputs.md).

## Evidence

All observed on the research unit over the UART, unless marked.

- **Supervision probe.** One receive-only capture of a cold boot passed, and the operator saw the indicator sequence twice. Both boots passed every startup policy.
  - SYSTIMER: about 32 752 Hz against host receive times over 60 s, −475 ppm from 32 768 Hz, within the host timestamp uncertainty of about ±0.1%. It only moved forward.
  - TIM2: a 10.015 ms period, against 10.010 ms nominal for 328 ticks.
  - A 2 s busy stall (65 536 ticks) kept TIM2 interrupts running, and supervision stayed ready, healthy and stopped.
  - The watchdog armed with vendor word `5601001f` and was fed for 15 s with 60 checkpoints, without a reset. The loop that stopped making progress latched fault 4 (PROGRESS) and never refreshed again.
  - The reset followed about 3.0–3.1 s after the last refresh, but the probe printed `stalled` only once a second, so `ms=3015` bounds the reset only between about 3.0 and 4.3 s. Later resets measured the clock directly (below).
  - The next boot reported boot reason `00100000` (`BIT_BOOT_KM4WDG_RESET_HAPPEN`) and the surviving marker, then skipped the probe. Both reset-loop guards worked.
  - Not exercised: owner-lease expiry while a task stalls, since the probe never ran owner-side processing. That was later shown under real motion ([motor pulse](motor-pulse.md#a-stalled-loop-and-a-failed-stop-during-a-feed)).
- **Watchdog clock.** The handshake runs' resets came about 4.0–4.2 s after the last rate-limited refresh, an effective clock near 31–33 kHz. On October 25, five resets from the `feeder` runtime's watchdog came 3.97–4.05 s after the logged last refresh (`PLSUP1 stop … fed_ms=`), timed by the host's UART chunks: about 32.8 kHz, the SDK's SDM 32.768 kHz clock, which KM0 calibrates on every boot (recovered).
- **KM4 SysTick CLKSOURCE** reads 1 whatever is written, so park and hook context checks test only ENABLE and TICKINT.
- **Boot handshake.** Passed on one cold boot. KM0 answered within one receive chunk, and a deliberately changed KM0 session latched PEER_LOST 10.0 s later, with outputs blocked. Nothing was written to flash.
- **Park round trip.** One power-on produced three boots, as designed: the withheld ACK reset 3.91 s later; after the withheld RELEASE, KM0 published TIMEOUT at 705 ms and the platform reset; a clean run followed. All 300 leases showed a frozen KM0 heartbeat while parked and an advancing one between leases. The worst park took 6 SYSTIMER ticks and release 2.
- **Mediated read.** Passed on one cold boot. With the lease provider in the SDK flash-lock hooks, 100 SDK `FLASH_RxCmdXIP` JEDEC reads all returned `c84017`, each inside its own lease, with KM0 frozen for every one. The worst park took 4 ticks and the worst release 2; the reads took 2.06 s in total. The coordinator ended idle and unfaulted, with no reset. This shows one read-only SDK path working through the hooks, not exclusive flash ownership for every SDK user.
- **Scratch write.** Passed on one cold boot, on sector `0x185000`. The ID read `c84017` and the preimage was blank. Three erase, program and verify cycles had zero errors, and a final erase restored the sector to blank. All 53 leases froze KM0. Erase took 27–35 ms including the lease, and a page program at most 0.7 ms. This covers the hooked read, sector-erase and page-program paths only; at the time, KM0's own SDK flash users and other KM4 users (Wi-Fi, OTA) were not mediated.
- **Power inputs, adapter only.** One cold boot, no batteries, motor stopped. Startup had already set the ADC function and clock bits (`001115f7`, `00131557`), and the explicit enable read back unchanged. The efuse holds programmed calibration, not the erased defaults. 93 samples arrived 1.02 s apart, each 32-conversion capture in at most 10.1 ms. PA19 read 1 throughout; PA16 and PA17 read 0 with emitters off. Channels 4 and 5 both read raw 239–243 (mean 240, scaled 16), consistent with no battery and no motor current.
- **Supply channel.** A second boot under `fw14-adc-v2` (95 samples) read channel 10 at raw 3911–3924 while channels 4 and 5 stayed at 239–244, so the channel select works. Its stock-scaled value was 3315–3317. Channel 10 measures the I/O supply (the SDK's 3.3 V detection reads it), and the feeder's idle UART TX, which sits at that supply, measured about 3.29 V on a meter ([UART notes](../devices/plaf103/uart.md)). So the stock scaling gives millivolts within about 1%, at least for this channel. Each 48-conversion capture took at most 15.1 ms.
- **Batteries** (October 23, UART and meter): the `adc` probe, installed over the air (v64) and confirmed. A cold boot with no batteries passed `wellfed-audit adc` (58 samples, `fw14-batt-01-baseline`). The owner then inserted three D cells and later pulled and replugged the adapter, without a reboot. Results are in [power](power.md#evidence). Captures `fw14-batt-02-both`, `-04-unplug` and `-05-meter` are receive-only with no startup, so they were read, not audited.
- **Wi-Fi scan.** Two cold boots on adapter power. `wifi_on(STA)` took 234–236 ms and all three scans completed in 4.0–4.3 s with stopped outputs, hearing 51–63 networks. The driver printed `FW Enable failed` up to three times a boot, then suspected to come from the WLAN firmware not running on KM0. In the first boot that print split the `PLWIF1 start` line and the capture was rejected; the second boot, with scheduler-suspended logging and strongest-32 retention, passed. The owner's hotspot was not among the networks kept in either boot; whether it was broadcasting was not recorded.
- **Wi-Fi join.** Without the WLAN firmware on KM0, SAE authentication, association and the 4-way handshake completed but DHCP got nothing, and the driver printed `fw cmd fifo full` 98 times. With it started, those prints stopped and the feeder's DHCP DISCOVERs reached the hotspot well formed; the bench host's own nftables input policy was dropping them. After the owner allowed DHCP from the hotspot interface, one cold boot passed `fw07-join-v1` (UART plus the dnsmasq journal). `wifi_on` took 149 ms. The first association was rejected with status 30, because the access point still held the previous boot's protected association. One 1 s backoff later, attempt 2 joined 24.3 s after radio on: connect took 5.9 s and DHCP 3.8 s, with a dnsmasq-acknowledged lease. The operator's outage showed as a loss after 30.5 s connected. Backoffs ran 1, 2, 4 and 8 s, and the feeder rejoined 41.7 s after the loss, renewing the same lease in 39 ms. The password never appears in the UART output. The same hotspot had leased addresses to this feeder under stock firmware. Objects compiled before the platform options override once still linked `restore_wifi_info_to_flash`, which the link audit caught.
- **DNS check.** Three cold boots with the owner's hotspot (UART plus the host's journal for block and unblock times). Using lwIP's resolver, every lookup passed through a 51 s DNS block, each in 0 ms from its cache. With the explicit query, a boot with no block made 27 lookups in 1–63 ms, absorbing one spontaneous timeout. The passing boot joined 21.5 s after radio on. The operator blocked DNS; 11 s later, after two timeouts, the feeder declared a loss and rejoined, and each rejoin got a lease then failed DNS twice, with backoffs of 1, 2 and 4 s. DNS was unblocked 50 s after the block, and the next attempt resolved 10 s later, 48.4 s after the first loss; 14 more lookups succeeded. Nothing reset. DHCP-server loss and lease expiry (the lease is 1 h) were not exercised.
- **No flash writes from Wi-Fi.** A full 8 MiB flash read in loader mode, one 3-minute boot with a 24 s hotspot outage and a 41 s DNS block (passing `fw07-dns-v1`), then a second full read: the two were byte-identical, so nothing wrote flash during that boot by any path. The outage surfaced as a DNS loss before the driver's beacon-loss detection. This covers one boot of a few minutes, not long-running operation or events such as lease renewal.
