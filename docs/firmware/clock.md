# Clock and time

The feeder needs to know the time to run meals on schedule, but it has no battery-backed clock. This page explains how Wellfed gets the time, how it keeps an honest bound on how wrong it might be, how it carries the time across a restart, and what happens when it doesn't know the time. The central idea: the clock never claims to know more than it does. Once its possible error passes 2 minutes it reads invalid, and the feeder says it needs the time.

Status: **Partial**. Client syncs, the zone setting, NTP and the carry across restarts are observed on our unit. The clock needs a time source after each power-up. Learning the drift rate and the long-uptime expiry are not yet checked on hardware.

Code: `wellfed-core/src/clock/` (`wall.rs`, `zone.rs`, `ntp.rs`), `wellfed-hal/src/clock.rs`, `wellfed-feeder/src/feeder/time.rs` and `src/store/time.rs`, `wellfed-feeder-runtime/src/time.rs`, and `wellfed-client/src/time.rs` for the rule a client sends.

## Sources

- **A client's sync** (`Op::TimeSync`, inside a Noise session) carries the time, the zone's offset now and its next change. `wellfed time` sends this computer's clock and zone.
- **NTP** from the loop ([below](#ntp)), when the time settings name a server. Offline means no server and no cloud, not no network: the default server is the DHCP gateway, which many home routers answer for.

A fix is a time and how far off it could be. A client sync counts 1 s for its trip across the LAN. An NTP answer counts half its round trip less the server's processing, plus the server's root delay and dispersion, plus 3 ms of rounding.

## The clock

**What counts the time.** Between fixes the clock runs on the SysTimer, the 32 kHz free-running counter, corrected by the learned rate. It is extended to milliseconds at every clock access, and the loop makes one every pass. It does not use the scheduler's tick: every flash lease masks interrupts and stops SysTick for the length of its erase (about 45 ms), so the tick falls behind by each erase. The SysTimer runs through them, and the lease provider times its masked spans with it. A read that finds the count moved backwards, or by half its range (about 18 h without a read), forgets the time and clears the carry. The NTP exchange times its round trip on the same count, so an erase during an exchange widens the answer's error instead of shifting it. Records keep `at_ms` as uptime. `PLTIM1 sync` and `carry` add `count_ms`, the clock's count, whose lead over `at_ms` is the time the tick lost.

**When it is valid.** After a power-on, a brownout, or a reset that finds nothing to resume, the clock starts invalid. A warm reset (watchdog or software: a restart, an activation, a trial boot) resumes it ([warm resets](#warm-resets)).

**The trusted interval.** Only a trusted fix sets the clock's anchor: an owner's sync, or any fix while the clock is invalid (the first, or after expiry). From the anchor, the true time lies within the anchor's error plus the rate bound times the time since.

**The rate bounds:**

- **Unlearned: 5,000 ppm.** That gives about 6.6 h of validity after a trusted fix.
- **Learned floor: 1,500 ppm.** A learned correction is never trusted tighter than this. That gives about 22 h of validity after a trusted fix. A bench run that logs the rate across a long uptime can lower it.
- **A bad fix:** a measured rate past ±10,000 ppm (1 %).

The bounds were set when the tick-counted clock seemed to drift by up to 2,060 ppm. That drift turned out to be the tick losing erases; the SysTimer measured about 2 ppm against NTP ([evidence](#evidence)). The bounds stay until a long run measures the SysTimer's rate.

**The cadence this gives.** The clock holds about 6.6 h after a trusted fix while the rate is unlearned, and about 22 h once an owner's syncs have taught it. In practice it is re-set at least daily:

- **With NTP** (the gateway by default) nothing is needed. Polls every 17 minutes keep the reading, and at each expiry the next answer (within 64 s) re-sets it inside the old interval. The clock reads invalid for up to a minute each time.
- **Without NTP,** something must sync it: the app or the server at least every 6 h or so until an owner's syncs have taught the rate, then at least daily. A home without a router that answers NTP can set a public pool's address (`wellfed time --ntp IP[:PORT]`; hostnames are not resolved yet). The app and the server should sync on every connection and warn when the time view says the clock needs the time.

**Drift learning** comes from owners' syncs only:

- An owner's sync is measured against a baseline, the first owner's sync since the rate was last forgotten. Once it measures the rate to within 50 ppm, the correction applies and the bound drops to the floor.
- A later measurement that disagrees widens the bound by the difference. An owner's sync that disagrees with the clock forgets the learned rate.
- The rate is not stored or carried across a reset; a boot learns it again.
- With owner syncs a few tens of minutes apart (NTP-like errors), it is learned within the window. Owner syncs alone (a client's one second each) need about 11 hours, so the second lands after the first window expired and is trusted then.
- NTP alone never teaches the rate, so an NTP-only clock stays at the unlearned bound. Its polls make that no worse than a learned one. A first owner's sync then becomes the baseline, and a second one measures the rate.
- A free NTP set (the first fix after a boot, or after three misses) is never a baseline. Otherwise an attacker's boot-time answer 100 s off would let an owner's sync six hours later learn +4,630 ppm with a 1,500 ppm bound, leaving the clock 333 s off at 20 h while it claimed 109 s.

**Why not learn from NTP fixes inside the interval.** They would teach the rate sooner. But a fix that only had to fall inside the trusted interval can sit anywhere in it, so a rate measured from two of them is uncertain by the interval's width over the span: the unlearned bound again. Such a rate could move the reading but never tighten the bound, which is all learning is for. The same holds for the NTP answer that re-sets an expired clock: it only had to fall inside the expired interval, so it teaches no rate either. Without that rule, an answerer at the edge each expiry could teach the clock a skewed rate and walk it about 30 minutes in three days while it claimed the learned bound.

**Untrusted fixes** (NTP, or a member's or the server's sync) must fall inside the trusted interval, or they are refused: `Refusal::Time`, `PLTIM1 sync ok=0 error=bounded`, or NTP's `bounded`. Inside, the reading moves to the overlap of the two intervals, but its reported error never drops below what the anchor allows: the reading's distance from the anchor's estimate plus the anchor's error. So an untrusted fix can improve where the clock reads, but cannot make it claim more than it knows, teach it a rate, or walk it outside the trusted interval. Everything is kept in microseconds internally, so rounding cannot walk it either.

**Validity and expiry.** The clock reads invalid once its reported error passes 2 minutes (`MAX_ERROR_MS`). That is about 6.6 h after a trusted fix unlearned (sooner if untrusted fixes moved the reading, which the error counts), or about 22 h learned. Then:

- An owner's or a client's sync sets it freely, and the NTP exchange asks within 64 s instead of waiting out its poll.
- An NTP answer must fall inside the expired anchor's interval, carried to now and grown at the rate bound with no two-minute cap (about ±2 minutes just after expiry). Outside, it is refused `bounded` and counts as a miss. So each NTP re-set moves the clock by at most that interval, and an answerer at the edge every expiry walks it about 2 minutes per 6.6 h.
- Three NTP answers in a row that all miss the trusted interval mean the anchor, the rate or the server is wrong. The clock forgets the anchor and the rate and reads invalid, so the feeder says it needs the time rather than staying silently valid. Then any answer sets it. A member's disagreement does not count toward the three.

**What this bounds.** An attacker answering NTP hourly at the edge of the clock's interval never makes a valid reading claim less error than it has. With an owner's sync every 12 hours the clock stays honest for a week; without one it expires (within 7 h) rather than lie. One spoofed answer teaches no rate. What NTP is trusted with is setting an invalid clock within its expired interval, and setting a clock with no anchor at all (the first fix, or after three misses) to anything: whoever answers the feeder's requests at that moment sets it. An on-path attacker can therefore walk the clock about 2 minutes each expiry, or make it miss three times and then set it freely. An owner's sync, or the app showing the time, is the check. Feeding plans that must not run on an unconfirmed time can treat a clock whose last fix was NTP after a `First`, with no owner sync since (the time view's `last: Ntp`), as unconfirmed.

**Steps and jumps.** A fix that moves the clock by at most 2 s is a step; anything else is a jump, logged with its signed size and recorded as a `ClockSync` event.

**Refused fixes** leave the clock unchanged: a time before 2026 or after 2099, or, for a sync, an offset outside −12 h to +14 h or a next change that is not in the future.

## Time without a source

No valid clock means the feeder needs the time, and the device API says so: `Status.clock_ms` is `None`, and the time view's `clock_ms` and `error_ms` are `None`. Scheduled meals hold: there is no scheduler yet, and when there is, it runs nothing while the clock is invalid, by the [missed-meal rule](#missed-meal-rule). The Dispense button and client feeds still work, since no feed path reads the wall clock. Showing the missing time on the panel is light policy.

## Time zone

The zone is a feeder setting (`PutTime`'s `zone`): a POSIX TZ rule such as `MST7MDT,M3.2.0,M11.1.0`. The feeder evaluates it for any year without a zone database (`wellfed_core::clock::zone`).

A client turns the owner's IANA name into the rule with `wellfed_client::time::rule`. It takes the footer of the zone's TZif file (RFC 8536) from the tzdb that `jiff-tzdb` bundles. It checks it with the feeder's own parser (offsets −12 h to +14 h, as syncs), and against both the bundled zone and the host's view of it, which the caller reads and passes in, at every transition of the next ten years. A footer only describes the zone after its file's last listed transition, so a zone that lists irregular changes ahead is refused (`RuleError::Irregular`) and stays on the syncs' offsets. In the current tzdb no zone is (host-tested over every zone). Every footer fits the 48 bytes the store keeps.

Without a zone setting, local time comes from the last client sync's offset and next change, which is correct across one change. A rule wins over a sync's offsets. `wellfed time --zone America/Denver` (or `--zone system`) sets it, and `--clear-zone` clears it.

## NTP

`wellfed_core::clock::ntp` is the exchange, sans-IO. `wellfed_feeder::feeder::time` runs it in the loop task on one non-blocking UDP socket, as the mDNS responder does, so it needs no new task.

- **Server.** The time settings' `ntp`: `Off`, `Gateway { port }` (the default, port 123: the lease's gateway) or `Address { ip, port }`. The default trusts nothing about the gateway except that it is on the path: an unauthenticated answer comes from whoever holds the gateway's address and sees the request's nonce. That bounds what it can do to a valid clock (above), not what it can set an invalid one to. Hostnames (`pool.ntp.org`) would need a DNS lookup in the loop, and are not supported yet. Nothing is sent before DHCP gives an address, and a changed server or gateway is asked at once.
- **Requests** carry nothing but a random 64-bit nonce in the transmit timestamp (RFC 9109's data minimization). An answer must come from the server's address and port and echo the nonce, checked first, so a datagram of any other shape leaves the exchange waiting for the real answer. It must then be a version 3 or 4 server answer, synchronized (leap not 3, stratum 1–15), dated 2026–2099 (NTP era 1 from 2036 is read), and loose by at most 10 s.
- **Pacing.** One request at a time, with an answer due within 2 s. A lost or unusable answer retries after 8 s, backing off to 64 s. After a good one, the next goes 1024 s (about 17 minutes) later, or within 64 s while the clock needs the time. A kiss-o'-death `RATE` holds the exchange for an hour, and `DENY` or `RSTR` for a day. Nothing shortens a hold, not even a clock that needs the time.
- **Records.** `PLAPI1 ntp server=… | socket open=… | answer result=used|bounded|refused error_ms=… | refused reason=… | lost`. The clock logs `PLTIM1 sync ok=1 source=ntp kind=first|step|jump delta_ms=… error_ms=… at_ms=…`. A first fix and a jump are recorded in the history as `ClockSync` by `Source::Feeder`. A fix in step is not, so polling does not wear the event ring.

## Time settings

`GetTime` and `PutTime { base, changes }` sit behind `Capabilities::TIME` ([device API](device-api.md)). They are a new op pair, a new outcome (`Time(TimeView)`) and new enums, appended with their fixtures; nothing existing changed bytes.

- `GetTime` is anyone's.
- `PutTime` is an owner's only, since the NTP server decides which fixes the clock hears. It is answered `Done` once the store holds the settings asked for (or already held them), and refused `NotWritable` when the writer could not write them.
- The bit is 8 (`FEED` 1, `UPDATE` 2, `READINESS` 4).
- The view carries the settings' generation, whether they are writable, the zone's rule, the server, the clock (`clock_ms`, `error_ms`, `offset_s`), the learned rate and its bound, the sync count, the last fix's source and age, and the last NTP result.

**Storage.** The settings live in a store of their own, storage sectors 27 and 28 (`wellfed_feeder::store::time`): a 128-byte dual-slot record, magic `PLFTIM01`, version 1, pinned in `store_layout.rs`. The settings writer opens it with the Wi-Fi store and writes it under the write turn and the arming attempt marker, as every store write. The settings record (sectors 2 and 3) is unchanged, and so is its stale `utc_offset_minutes` field. No bank-0 refresh is needed: a build that predates this store never reads or writes sectors 27 and 28, so bank 0 keeps running on its own settings, and bank 1 reads defaults until an owner writes them.

## Warm resets

The clock survives a warm reset, but not a power cut. Two things make that work: the KM0 RTC counts seconds across the reset, and 64 bytes of KM4 RAM hold the last known time.

**The RTC.** The KM0 RTC (`0x48004000`) is an always-on BCD calendar on the 32 kHz clock: a 9-bit day count and hours, minutes and seconds. It counts seconds from power-on although nothing initializes it (`ISR`'s INITS flag is clear at every boot). It keeps counting across watchdog, activation and trial resets, and restarts at 0 on a power cut. The firmware never writes it, and logs it read-only at boot (`PLTIM1 rtc tr=… cr=… isr=… prer=… at_ms=…`).

**Where the time waits.** It needs somewhere a warm reset keeps and a power cut loses (recovered from the pinned SDK):

- **KM0 backup registers** survive warm resets, but every register the SDK leaves to the user is taken: 1 the attempt and update bits and the lease fault, 2 the silent-reset breadcrumb, 3 KM4's boot count, 4 the peer boot count and the trial count, 5 the KM0 heartbeat.
- **Retention SRAM** (1 KiB at `0x000C0000`) is zeroed by KM0's start code on every boot that is not a deep-sleep wake (`rtl8721dlp_app_start.c`).
- **KM4's main RAM** is not cleared by a warm reset in any code the SDK ships. The KM4 boot loader (`boot_flash_hp.c`) clears its own `.bss` in its secure region and copies only the image's RAM data (`.ram_image2.entry`, `.text`, `.data`, the sections the image's `ram_2.bin` holds). `app_start` clears only `.ram_image2.bss`. The FreeRTOS heap (`heap_5`, `CONFIG_DYNAMIC_HEAP_SIZE`) runs from `__bfsram_end__` to `__sram_end__`, the top of `BD_RAM_NS` at `0x1007C000`. The ROM's and boot loader's RAM, the secure stack and the non-secure main stack lie outside it (`0x1007C000` and up, below `0x10005000`).

So the builder's KM4 linker script takes the top 64 bytes from the heap (`SdkAnchor::SramEnd`) and names them `wellfed_time_carry`, at `0x1007BFC0`, the same address in every build so a record survives an activation into a new build. The 880 bytes below hold the [call trail](startup-diagnostics.md#the-call-trail), so the heap ends at `0x1007BC50`. `example_entry.c` loads and stores the 16 words and cleans the store out of the D-cache. The SDK's header says the cache writes through; the clean holds either way.

**The record** (`WallClock::carry`, in RAM only, so no flash format changes) holds the RTC second, the trusted estimate, its error and the reading at that second (in microseconds), the zone hint, the last fix's origin and age, a version and a CRC-32. Keeping the last fix's origin means the time view's `last` survives, and with it whether an owner confirmed the time. The record is written after every accepted fix and every 10 s from the loop, and cleared while the clock is invalid. So a reset never finds a clock older than the last correction, or one that had expired. The record's version is 2: a record from a build that counted on the tick is refused as `corrupt`.

**Resuming** happens in `WallClock::resume`, from the start hook, before anything reads the clock. It logs `PLTIM1 carry result=resumed gap_s=… error_ms=… rtc_s=… at_ms=…`, or `result=cold|empty|corrupt|rtc|backwards|stale|expired|set`. A record resumes only when all of these hold:

- the boot follows a watchdog or software reset, by the ROM's reset flags (a power-on, brownout or deep-sleep wake is `cold`);
- the record passes its magic, version, CRC and range checks;
- the RTC reads as a 24-hour count, with two reads that agree;
- the RTC is no earlier than the record's second (`backwards`: the RTC restarted), and at most 600 s after it (`stale`).

A gap of `g` RTC seconds moves both estimates by `g` s and grows the error by 1 s (both ends are whole RTC seconds) plus `(g + 1)` × 10,000 ppm, the RTC's rate, wide since the gap is short. A grown error past 2 minutes is `expired`. The resumed interval is trusted, as the saved one was, since it only grew from a trusted fix; untrusted fixes still only narrow it. It is no baseline for a rate: the rate and sync count start again, while the last fix keeps its origin and age. Whatever came of it, the boot then stores its own record or clears it.

**What it does not catch.** Every build that resumes clears the record at a cold boot. A build that does not know the record leaves it alone; bank 0's `92c7adec…` uses that RAM as heap top. So a record could outlive a power cut if the RAM held its contents through it, the next boot ran that build, and a later warm reset into this one found the RTC within 600 s past the record's second. Then the gap would miss the time before the cut. That needs the RAM to keep 64 bytes exactly through a power loss, and it goes away once bank 0 runs a build that resumes. Until bank 0 runs a build that counts on the SysTimer, a hop through it leaves the trial boot unsynced.

## Missed-meal rule

`wall_clock::occurrence(due, evaluated, now)` decides each scheduled occurrence. `evaluated` is a persisted mark per plan: the latest occurrence already run, missed or before the plan's save ([schedules](schedules.md#at-most-once)).

| Situation | Result |
| --- | --- |
| No valid clock | Nothing runs |
| `due` after `now` | Later |
| `due` at or before the mark | Evaluated: never runs twice, even after a backward jump |
| `due` in (mark, now], at most 15 min late | Runs once |
| `due` in (mark, now], more than 15 min late | Missed: reported, never run |

So a forward jump, a restart or a long unsynced spell runs only occurrences that are at most 15 minutes late, and a backward jump never repeats a meal.

## Bench plan

The physical checks still to run. None of them moves the motor.

**Network for NTP.** The bench host's firewall (`/etc/computer-security.nft`, table `inet computer_security`, input policy drop) admits from the hotspot interface `wlan1` only DHCP (UDP 67), DNS (TCP and UDP 53) and the broker (TCP 1883 to `10.42.103.1`). UDP 123 from the feeder is dropped, as is any other port. No time service runs on the host (`systemd-timesyncd` and `chronyd` inactive), and unprivileged processes cannot bind below 1024 (`ip_unprivileged_port_start` is 1024). So a live test needs the owner to add one input rule beside the hotspot's others, and then either:

- **the standard port:** `iifname "wlan1" ip saddr 10.42.103.0/24 ip daddr 10.42.103.1 udp dport 123 accept`, with the responder run with the privilege to bind port 123 (`sudo`, the owner's call); or
- **an unprivileged port**, which the setting's port allows: `iifname "wlan1" ip saddr 10.42.103.0/24 ip daddr 10.42.103.1 udp dport 1123 accept`, and the feeder set to `--ntp gateway:1123`.

Replies need no rule: the output path is unfiltered and the input chain accepts established flows. The responder serves the host's own clock, which nothing on the host disciplines; that is fine, since the reference is the host.

1. **Live NTP sync** (about 45 minutes). Install the `feeder` image. With the hotspot up and the rule in place, run the bounded responder in the foreground: `cargo run -p wellfed-rig --example ntp-serve -- 10.42.103.1:1123 --seconds 2700` (or `:123` with the standard rule). Then run `wellfed time --ntp gateway:1123 --zone America/Denver --address <feeder>` and `wellfed time --show`. You should see, with no client sync since boot: `PLAPI1 ntp server=10.42.103.1:1123`, a `PLTIM1 sync ok=1 source=ntp kind=first` within seconds, the view's `clock_ms` within 1 s of the host and `error_ms` under 100, `offset_s` −21600 or −25200 as the date says, and a `ClockSync` event by the feeder in `wellfed events`. Leave it 45 minutes: every poll is `used`, and each poll's `delta_ms` over its spacing is the clock's rate. `learned` stays false with NTP alone. Stop the responder and restore the settings (`--ntp gateway`), the hotspot and the firewall as the owner wants them.
2. **Learning the rate.** Two owner's `wellfed time` syncs about 11 hours apart should turn `learned` true, with `rate_ppb` near the measured rate and `rate_bound_ppb` 1 500 000. Each sync carries `SYNC_ERROR_MS` (1 s), and a rate is learned only to within 50 ppm. Logging the rates over a long run shows how much the rate moves within a boot, which is what the learned floor stands for.
3. **Spoof check.** A member's `TimeSync` an hour off must be refused `Time`. Alternatively, point `--ntp` at a second responder whose clock is offset (this needs the responder to gain an offset option, or a second host clock).
4. **Warm resets, remaining cases.** After a power cycle (relay), the boot logs `result=cold` with the clock invalid, and after the next warm reset `empty`. With the clock made invalid first (`--ntp off`, then wait out the window or reboot cold), `wellfed reboot` gives `empty`, since an invalid clock is never stored. To check the carry again, confirm in the link map that `wellfed_time_carry` is at `0x1007BFC0` and `__sram_end__` at `0x1007BC50`, below the call trail.
5. **Expiry after a long uptime** (about 7 hours, unattended). Sync the clock once (`wellfed time`), set `--ntp off`, and stop anything that syncs it (the server, the bench's `itest`). After about 6.6 hours the time view's `clock_ms` turns `None`, and `Status.clock_ms` with it. A warm reset before then resumes it with the gap added, and a power cycle shows it invalid at once. With NTP on, the clock instead stays valid with a small `error_ms`, apart from up to a minute around each expiry, when the next poll (within 64 s) re-sets it with `PLTIM1 sync ok=1 source=ntp kind=first`.

## Evidence

- **Client syncs and jumps.** Observed (October 22): the first sync from the server (`PLTIM1 sync ok=1 kind=first`, MDT with the November 1 change) put health's clock within 0.2 s of the server, with an uncertainty of 1.06 s. With `examples/time_jump.rs`, a +1 h and a −2 h jump were classified (`jump_ms` 3600097 and −7200007). Two clean 10-minute resyncs found +442 and +439 ms: the tick-counted clock ran about 735 ppm slow.
- **Zone and first NTP fix.** Observed (October 26, v232, `combo-fw46-20261026-01`…`04`): the zone setting survived a reboot and a power cycle. A first NTP fix had an error of 25 ms and recorded a `ClockSync`. The next poll, 1,026 s later, was refused `bounded`: the tick-counted clock ran about 2,060 ppm slow against the host's timesyncd clock, past the then 2,000 ppm bound.
- **The RTC.** Observed (October 26, v232): it kept counting across watchdog, activation and trial resets (1,613 s against the host's 1,613.1 s) and restarted at 0 on a power cut.
- **Tick loss.** Observed, with the cause recovered from the logs' arithmetic (October 26): the carry held across `wellfed reboot` (`gap_s=2 error_ms=2131`, within 0.3 s of the host) and read `cold` after a power cut. But after a v237 install through bank 0's recovery role, both boots logged `resumed` (bank 0 `gap_s=6 error_ms=3852 rtc_s=438`, the trial `gap_s=6 error_ms=5174 rtc_s=506`), and the clock then read 12.2 s behind the host while claiming 5,510 ms. Bank 0 ran 62 RTC seconds before its last store, yet its error grew by only about 252 ms at 5,000 ppm (5,174 − 3,852 − the trial's 1,070 ms of RTC allowance). So its tick saw about 50 s: about 12 s lost across the bank's 249 erases. Modeled: run on the tick, the same download in `wellfed-hal`'s `clock.rs` leaves the clock 11.2 s off while claiming 1.3 s. That is why the clock counts on the SysTimer. About 47 erases in the 1,026 s poll would explain the 2,060 ppm (recovered).
- **The carry across an activation.** Observed (October 27, both banks on the SysTimer build `807f7283`): bank 0 was written over UART (`fw46-bank0-20261027-01`) and given way to (`fw46-giveway-20261027-01`: `result=cold` after the power cut). After a `wellfed time` sync, v243 of the same build went into bank 1 through bank 0's recovery role (249 sectors, `fw46-v243-20261027-01`, confirmed). The trial boot logged `carry result=resumed gap_s=3 error_ms=2341`, and the view read 121 ms from the host, request time included, claiming 2,571 ms, with no sync since boot.
- **Live NTP on the SysTimer.** Observed (October 27, same boot, `fw46-ntp-20261027-01`): a bounded responder on `10.42.103.1:1123` for 45 minutes and `--ntp gateway:1123`. Three polls 1,024 s apart were all `used`. The third logged `delta_ms=2`, about 2 ppm against the host's timesyncd clock, with `count_ms` 7,448 ms ahead of the tick's `at_ms` after 36 minutes of the boot. `learned` stayed false with NTP alone, and an NTP fix on a valid clock moved the reading without resetting the owner-anchored bound, as designed. Two owner syncs 45 minutes apart left the rate unlearned, as the 50 ppm rule requires.
- **Why the bounds are where they are.** The unlearned 5,000 ppm is more than twice the worst rate measured (2,060 ppm). The learned floor of 1,500 ppm sits above the 1,325 ppm by which the tick-counted rate moved between the October 22 and October 26 measurements. The earlier bounds were 2,000 ppm unlearned (16.5 h of validity) and 100 ppm learned (7 to 14 days).
- **SysTimer rate.** Observed but rough: the supervision probe read the SysTimer about 475 ppm slow over one 60 s probe, about ±1,000 ppm.
- **Host and simulation.** Modeled (October 26): the error-bound clock and its rules, the review's attacks (an hourly edge answerer, a multi-day walk, a poisoned drift rate, a boot-time baseline, a kiss-o'-death flood, each with a regression test), the zone parser over every tzdb zone, the NTP exchange including a denying server on the firmware's loop, the carry and resume (`wall_clock.rs`, and through the fake RAM and RTC in `wellfed-hal`'s `clock.rs`), an erase during an NTP exchange, and the missed-meal rule.
