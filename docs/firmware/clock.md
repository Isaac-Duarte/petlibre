# Wall clock, offline time and missed-time policy (FW21, FW46)

Status: FW21's sync and jump classification are physical (October 22). FW46's error bound, drift learning, bounded steps, zone setting and NTP client are host-tested and simulated (October 26); their bench run is planned [below](#bench-plan). Code: `wellfed-core/src/clock/` (`wall.rs`, `zone.rs`, `ntp.rs`), `wellfed-hal/src/clock.rs`, `wellfed-feeder/src/feeder/time.rs` and `src/store/time.rs`, `wellfed-feeder-runtime/src/time.rs`, and `wellfed-client/src/time.rs` for the rule a client sends.

## Sources

- **A client's sync** (`Op::TimeSync`, inside a Noise session): the time, the zone's offset now and its next change. The `wellfed time` command sends this computer's clock and zone.
- **NTP** from the loop (below), when the time settings name a server. Offline means no server and no cloud, not no network: the default server is the DHCP gateway, which many home routers answer for.

A fix is a time and how far off it could be: a client sync counts 1 s for its trip across the LAN; an NTP answer half its round trip less the server's processing, plus the server's root delay and dispersion, plus 3 ms of rounding.

## The clock

- **Invalid at boot.** Nothing in use keeps time across a reset (see [warm resets](#warm-resets)), so every boot starts unsynced.
- **Carried by uptime** between fixes, corrected by the learned rate.
- **The trusted interval.** Only a trusted fix sets the clock's anchor: an owner's sync, or any fix while the clock is invalid (the first, or after expiry). From the anchor, the true time lies within the anchor's error plus the rate bound times the time since.
- **Drift learning,** from trusted fixes only. The uptime clock ran a steady 735 ppm slow against the server (two 10-minute resyncs, physical, October 22). Before a rate is learned the bound is 2000 ppm. Trusted fixes are measured against a baseline (the first trusted fix since the rate was last forgotten); once it measures the rate to within 50 ppm, the correction applies and the bound drops to 100 ppm. A later measurement that disagrees widens the bound by the difference; a rate past ±5000 ppm is taken for a bad fix. An owner's sync that disagrees with the clock forgets the learned rate. The rate is not stored; a reboot learns it again. With owner syncs a few tens of minutes apart (NTP-like errors) or about 11 hours apart (a client's one second) it is learned; with NTP alone it is learned across the first expiry (about 12.5 h), from one trusted fix to the next.
- **Untrusted fixes** (NTP, or a member's or the server's sync) must fall inside the trusted interval, or they are refused (`Refusal::Time`, `PLTIM1 sync ok=0 error=bounded`, NTP's `bounded`). Inside, the reading moves to the overlap of the two intervals, but its reported error never drops below what the anchor allows: the reading's distance from the anchor's estimate plus the anchor's error. So an untrusted fix can improve where the clock reads, cannot make it claim more than it knows, cannot teach it a rate, and cannot walk it outside the trusted interval. Internally everything is kept in microseconds, so rounding cannot walk it either.
- **Validity.** The clock reads invalid once its reported error passes **2 minutes** (`MAX_ERROR_MS`): about 16.5 h after a trusted fix unlearned (sooner if untrusted fixes moved the reading, which the error counts), 7 to 14 days learned. Then the next fix of any origin sets it again, and the NTP exchange asks within 64 s instead of waiting out its poll. Three NTP answers in a row that all miss the trusted interval mean the anchor, the rate or the server is wrong: the clock forgets the anchor and the rate and reads invalid, so the feeder says it needs the time rather than stay silently valid; a member's disagreement does not count.
- **What this bounds.** Host-tested with the review's attacks: an attacker answering NTP hourly at the edge of the clock's interval never makes a valid reading claim less error than it has, and with an owner's sync every 12 hours the clock stays inside its bound for a week; without one it expires (within 17 h) rather than lie; one spoofed answer teaches no rate. What NTP is trusted with is setting an invalid clock (as any source is): whoever answers the feeder's requests at that moment sets it. An on-path attacker can therefore make the clock need the time and then set it; an owner's sync, or the app showing the time, is the check.
- **Classification:** a fix that moves the clock by at most 2 s is a step, anything else a jump, logged with its signed size and recorded as a `ClockSync` event.
- **Refused fixes** leave the clock unchanged: a time before 2026 or after 2099; for a sync, an offset outside −12 h to +14 h or a next change that is not in the future.

## Time without a source

No valid clock means the feeder needs the time. The device API says so: `Status.clock_ms` is `None`, and the time view's `clock_ms` and `error_ms` are `None`. Scheduled meals hold (there is no scheduler yet; FP03 runs nothing while the clock is invalid, by the missed-meal rule below), and the Dispense button and client feeds still work: no feed path reads the wall clock. Showing it on the panel is FW23's and FP12's.

## Time zone

The zone is a feeder setting (`PutTime`'s `zone`): a POSIX TZ rule such as `MST7MDT,M3.2.0,M11.1.0`, which the feeder evaluates for any year without a zone database (`wellfed_core::clock::zone`). A client turns the owner's IANA name into the rule with `wellfed_client::time::rule`: the footer of the zone's TZif file (RFC 8536) in the tzdb `jiff-tzdb` bundles, checked by the feeder's own parser (offsets −12 h to +14 h, as syncs) and against both the bundled zone and the host's view of it, which the caller reads and passes in, at every transition of the next ten years. A footer only describes the zone after its file's last listed transition, so a zone that lists irregular changes ahead is refused (`RuleError::Irregular`) and stays on the syncs' offsets; in the current tzdb no zone is (host-tested over every zone). Every footer fits the 48 bytes the store keeps.

Without a zone setting, local time comes from the last client sync's offset and next change, as before: correct across one change. A rule wins over a sync's offsets. `wellfed time --zone America/Denver` (or `--zone system`) sets it; `--clear-zone` clears it.

## NTP

`wellfed_core::clock::ntp` is the exchange, sans-IO; `wellfed_feeder::feeder::time` runs it in the loop task on one non-blocking UDP socket, as the mDNS responder does (no new task):

- **Server:** the time settings' `ntp`: `Off`, `Gateway { port }` (the default, port 123: the lease's gateway) or `Address { ip, port }`. Hostnames (`pool.ntp.org`) would need a DNS lookup in the loop; they are not supported yet. Nothing is sent before DHCP gives an address, and a changed server or gateway is asked at once.
- **Requests** carry nothing but a random 64-bit nonce in the transmit timestamp (RFC 9109's data minimization); an answer must come from the server's address and port and echo the nonce, checked first, so a datagram of any other shape leaves the exchange waiting for the real answer; then be a version 3 or 4 server answer, synchronized (leap not 3, stratum 1–15), dated 2026–2099 (NTP era 1 from 2036 is read), and loose by at most 10 s.
- **Pacing:** one request at a time, an answer within 2 s; a lost or unusable answer retries after 8 s, backing off to 64 s; after a good one the next goes 1024 s (about 17 minutes) later. A kiss-o'-death `RATE` waits an hour; `DENY` or `RSTR` a day.
- **Records:** `PLAPI1 ntp server=… | socket open=… | answer result=used|bounded|refused error_ms=… | refused reason=… | lost`; the clock itself logs `PLTIM1 sync ok=1 source=ntp kind=first|step|jump delta_ms=… error_ms=… at_ms=…`. A first fix and a jump are recorded in the history as `ClockSync` by `Source::Feeder`; a fix in step is not, so polling does not wear the event ring.

## Time settings

`GetTime` and `PutTime { base, changes }`, behind `Capabilities::TIME` ([device API](device-api.md#evolution)): a new op pair, a new outcome (`Time(TimeView)`) and new enums, appended with their fixtures; nothing existing changed bytes. `GetTime` is anyone's; `PutTime` is an owner's only, since the NTP server decides which fixes the clock hears; it is answered `Done` once the store holds the settings asked for (or already held them), and refused `NotWritable` when the writer could not write them. The bit is 8 (`FEED` 1, `UPDATE` 2, `READINESS` 4). The view carries the settings' generation, whether they are writable, the zone's rule, the server, the clock (`clock_ms`, `error_ms`, `offset_s`), the learned rate and its bound, the sync count, the last fix's source and age, and the last NTP result.

**Storage.** The settings live in a store of their own, storage sectors 27 and 28 (`wellfed_feeder::store::time`): a 128-byte dual-slot record, magic `PLFTIM01`, version 1 (pinned in `store_layout.rs`). The settings writer opens it with the Wi-Fi store and writes it under the write turn and the arming attempt marker, as every store write. The settings record (sectors 2 and 3) is unchanged, and so is its stale `utc_offset_minutes` field. **No bank-0 refresh is needed first:** a build that predates this store never reads or writes sectors 27 and 28 (they were free), so bank 0's build keeps running on its own settings and ignores these, and bank 1 reads defaults until an owner writes them.

## Warm resets

Static analysis of the pinned SDK (October 26), so far without a bench run:

- **KM0 backup registers** survive watchdog and system resets but not power loss. Every register the SDK leaves to the user is taken (1: the attempt and update bits and the lease fault; 2: the silent-reset breadcrumb; 3: KM4's boot count; 4: the peer boot count and the trial count; 5: the KM0 heartbeat), so none can carry a time.
- **Retention SRAM** (1 KiB at `0x000C0000`) is zeroed by KM0's start code on every boot that is not a deep-sleep wake (`rtl8721dlp_app_start.c`), so a warm reset loses it.
- **The KM0 RTC** (`0x48004000`) is an always-on BCD calendar: a 9-bit day count and hours, minutes and seconds, from the 32 kHz clock, which its header says "never stops, regardless of the device status (run mode, low power mode or under reset)" while the supply holds. Nothing in this firmware or the SDK's start code initializes it, and stock 1.1.15 may or may not have. If it runs and survives a watchdog reset, it could carry the time: its count gives the elapsed seconds since a reference, with second resolution, for up to 512 days. Using it also needs the reference (wall time at a known RTC count) and the error bound to survive, which no free register holds; the event ring's newest timestamp is a candidate.
- The `feeder` image therefore logs the RTC's registers read-only once at boot (`PLTIM1 rtc tr=… cr=… isr=… prer=… at_ms=…`; `ISR` bit 4 is the calendar's initialized flag). It never writes them. The bench check below decides whether FW46 uses it.

## Missed-meal rule (for FP03)

`wall_clock::occurrence(due, evaluated, now)` decides each scheduled occurrence. `evaluated` is a persisted high-water mark of time already evaluated; after each evaluation it becomes `max(evaluated, now)`.

| Situation | Result |
| --- | --- |
| No valid clock | Nothing runs |
| `due` after `now` | Later |
| `due` at or before the mark | Evaluated: never runs twice, even after a backward jump |
| `due` in (mark, now], at most 15 min late | Runs once |
| `due` in (mark, now], more than 15 min late | Missed: reported, never run |

So a forward jump, a restart or a long unsynced spell runs only occurrences that are at most 15 minutes late, and a backward jump never repeats a meal.

## Bench plan

Physical work for FW46, none of it motor motion. Each step needs the bench; durations are wall time.

**Network for NTP (static, October 26).** The bench host's firewall (`/etc/computer-security.nft`, table `inet computer_security`, input policy drop) admits from the hotspot interface `wlan1` only DHCP (UDP 67), DNS (TCP and UDP 53) and the broker (TCP 1883 to `10.42.103.1`). UDP 123 from the feeder is dropped, and so would any other port be. No time service runs on the host (`systemd-timesyncd` and `chronyd` inactive), and unprivileged processes cannot bind below 1024 (`ip_unprivileged_port_start` is 1024). So the live test needs the owner to add one input rule beside the hotspot's others, and then either:

- **the standard port:** `iifname "wlan1" ip saddr 10.42.103.0/24 ip daddr 10.42.103.1 udp dport 123 accept`, with the responder run with the privilege to bind port 123 (`sudo`, the owner's call); or
- **an unprivileged port**, which the setting's port allows: `iifname "wlan1" ip saddr 10.42.103.0/24 ip daddr 10.42.103.1 udp dport 1123 accept`, and the feeder set to `--ntp gateway:1123`.

Replies need no rule (the output path is unfiltered and the input chain accepts established flows). The responder serves the host's own clock, which nothing on the host disciplines; that is fine for this check, whose reference is the host.

**1. Live NTP sync (about 45 minutes).** Build and install the `feeder` image over the air. With the hotspot up and the firewall rule in place, run the bounded responder in the foreground: `cargo run -p wellfed-rig --example ntp-serve -- 10.42.103.1:1123 --seconds 2700` (or `:123` with the standard rule). Then `wellfed time --ntp gateway:1123 --zone America/Denver --address <feeder>` and `wellfed time --show`. Pass when, without any client sync since boot: `PLAPI1 ntp server=10.42.103.1:1123`, a `PLTIM1 sync ok=1 source=ntp kind=first` within seconds, the view's `clock_ms` within 1 s of the host and `error_ms` under 100, `offset_s` −21600 or −25200 as the date says, and a `ClockSync` event by the feeder in `wellfed events`. Leave it 45 minutes (three polls): `learned` turns true with `rate_ppb` near 735 000 and `rate_bound_ppb` 100 000. Then a spoof check: point `--ntp` at a second responder on another port whose host clock is offset (not needed if the responder gains a `--offset-ms`; a member's `TimeSync` an hour off is the simpler check, and must be refused `Time`). Stop the responder and restore the settings (`--ntp gateway`), the hotspot and the firewall as the owner wants them.

**2. The RTC across warm resets (about 20 minutes).** Read `PLTIM1 rtc` at a cold boot, then after a watchdog reset (`wellfed reboot`), an OTA activation and a trial boot (any `wellfed-bench ota` run shows both), and after a power cycle. It keeps time if `ISR` bit 4 is set and `TR` advances by the wall time between boots across the warm resets. If the calendar is not initialized (`isr` bit 4 clear at every boot), the check needs a build that starts it (`RTC_Init`, `RTC_SetTime` to 0) once per power-up; that is a new write to an always-on peripheral and goes to review first.

**3. Expiry after a long uptime (the carried FW21 item; about 17 hours, unattended, no motion).** Sync the clock once (`wellfed time`), set `--ntp off`, and stop anything that syncs it (the server, the bench's `itest`). After 16.5 hours the time view's `clock_ms` turns `None` and `Status.clock_ms` with it; a reboot before then also shows it invalid at once (unless step 2 finds a clock that survives). With NTP on the clock instead stays valid and `error_ms` small.
