# Resume firmware development

Handoff checkpoint: **October 21, 2026, Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**October 22, latest (offline, nothing run on the bench).** FW33: the relay bench is now the Rust `wellfed-bench` (crate `crates/wellfed-bench`). It chains `wellfed-flash` as a child process and serves the read-only page with `--ui`. `bench.py`, `bench-dashboard.py`, `relay-demo.py`, `uart-give-way.py` and seven finished one-off Python analyses are removed (list in [tools/README.md](tools/README.md)). It is host-tested only. **Next:** with the owner, build both binaries (`cargo build --locked --release -p wellfed-bench -p wellfed-flash`), then run `status`, `on`/`off`, `loader` and a no-op `flash-prepare`/`flash-execute` with `--ui` ([bench](docs/hardware/bench.md)).

**October 22, later (physical).** The **first live OTA** and a **hands-off OTA** both succeeded (FW10). The second was started by the running normal image, with no UART. FW11 trial, fallback and confirmation work on hardware (see the board).
- **Flash now:** bank 1 holds OTA-installed `fw10-watch-review-20261022-01` (`4d37c21a…`, v6, confirmed, offer watcher running). Bank 0 holds the same build, installed by the Rust client (FW32). Bank 0 holds `fw10-recovery-review-20261022-02` (`635476bd…`), the fixed updater.
- **Idle state:** the feeder is off, the hotspot down, and the server stopped.
- **Loader NAKs:** 2 in the last 3 large UART installs, one with the host idle. Ask the owner to reseat the UART header.
- **FW10 and FW32 are done.** The `rust-flash` worktree is merged with master (crates in `crates/`). **Paused at the owner's request.** Remaining board items need the owner: FW30 analyzer, FW14 batteries, PB3 test points, FW16 motion.

**October 22 (autonomous, physical through the relay bench).** The FW10 recovery-role updater is in (`d541c80` plus the audit fix). Bank 0 now holds `fw10-recovery-review-20261022-01` (identity `d2ed7d93…`); bank 1 still holds bench live target review 01 and boots normally. A UART give-way booted bank 0: `PLOTA1 start role=recovery key=1`, `ready`, then `armed watchdog=1 installed=1`. It ran 180 s on one boot, scanning for the hotspot, which was off. The bank-1 marker was restored, and bank 1 booted again. **Next:** a live download needs the hotspot up and `wellfed-local serve` running with `ota-offer` (owner approval: routing and a service), then give-way and watch `PLOTA1 progress`/`activated`.

**October 21–22 overnight (autonomous, offline, nothing flashed).** The owner set the goal of doing what needs no owner, committing and pushing as I go:
- **FW09:** the actual factory loader always prefers a valid bank 1 and only falls back to bank 0 (simulation). See [bank policy](docs/firmware/bank-policy.md).
- **FW10/FW11:** designed around that: bank 0 holds a recovery/updater role, and bank 1 the normal role ([OTA design](docs/firmware/ota-design.md)). Every marker step and partial marker write keeps a bootable bank in emulation.
- **OTA code:** the core OTA logic (signed manifests, the bank-1 writer, boot trials) and the server's `ota-keygen`/`ota-sign` are in, host-tested.
- **FW30:** the logic-analyzer transient command and its auditor are ready for Friday.
- **Needs the owner:** any write to bank 0 or the markers (the first OTA hardware stage), the PB3 test points, the analyzer and battery sessions, and any motion.

The installed image is the FW13 plan 2 build (5 s emitter windows; see below).


**FW09 is Done (October 22, physical).** Both banks now hold the same replacement image (identity `8d67a407…`). Through the relay bench:
- **Both valid:** the feeder booted bank 1.
- **Give-way:** clearing bank 1's first marker byte made it boot bank 0, the first replacement run from bank 0.
- **Restore:** it booted bank 1 again.

Full-flash reads before and after differ only in bank 0's application region. The updater now has `--bank 0`, `give-way` and `--restore-marker`, with the factory region never writable ([bank policy](docs/firmware/bank-policy.md)).

**FW13 and FW31 are Done (October 21).** FW13 (motion-free scope): PA15 lights
the food receiver (PA17), PB3 the outlet receiver (PA16), and a blocked beam
reads LOW on both. Receivers settle within one 2 ms poll, and the supervisor
cut the emitters 300–305 ms into a deliberate owner stall. PB3 is SWD_CLK:
startup now releases SWD as stock does, and before that PB3 was never driven.
Passage pulses and PA18 feedback are carried to FW16; emitter duty and OFF
levels to FW30. See [optical inputs](docs/firmware/optical-inputs.md). FW31:
the relay bench installed a changed image (one loader NAK, finished with
`--interrupted`) and ran a full 8 MiB dump. Only bank 1 differs from the FW08
read B, and it matches the installed review byte for byte.
`tools/bench/bench-dashboard.py` shows bench progress on localhost.

**FW14 (current, battery and external power inputs) is in progress.** FW15 is done (storage scope); real-motion journal wiring is carried to FW16.

**FW14 read-only ADC probe (physical, passed).** It ran for one cold boot on
adapter power, with no batteries and the motor stopped. Startup had already
set the LP ADC function and clock bits, and the explicit enable read back
unchanged. The efuse holds programmed calibration, not the defaults. 93
samples came 1.02 s apart, each 32-conversion capture taking at most 10.1 ms.
PA19 read 1 throughout. Both ADC channels read raw 239–243, just above the
calibration offset. That fits no battery and no motor current. A second build
(`fw14-adc-v2`) also reads internal channel 10, the chip's supply. It read raw
about 3918 (scaled 3316: millivolts, matching the 3.29 V measured on the idle TX line) while channels 4
and 5 stayed at 240, so the channel select works.

**Next for FW14:** with batteries in, the battery channel should rise and
differ from the current channel. Unplugging the adapter with batteries in
should drop PA19. A meter reading of the pack voltage then scales the channel.
The owner has a meter but no batteries yet. The millivolt reading of the
scaled value is confirmed: channel 10 gave 3316 against the feeder TX idle
level measured earlier at about 3.29 V.

**Install notes.** Two updates hit loader NAKs, most likely from a partly
seated UART header (owner's finding). The first stopped after its marker
commit, which had landed. The second stopped after 8 of 19 sectors, with bank 1
unbootable. It was finished with `uart-development.py --interrupted` (11
sectors plus the marker) after reseating, from a fresh read-only scan.

The owner allocated `0x340000–0x350000` (64 KiB) for replacement-firmware storage.
It is blank in the backup and not referenced by stock 1.1.15, and it lies clear
of both banks and of the stock settings:

| Sectors | Use |
| --- | --- |
| `0x340000`, `0x341000` | Journal slots |
| `0x342000`, `0x343000` | Settings slots (hold stress values, generation about 1 460) |
| Remaining 12 | Reserved |

**Journal on NOR (physical, passed).** The feed journal ran through mediated
erase/program and mapped reads. One power-on gave three boots:

1. Create the journal; reserve and finish requests 1 and 2 (recorded honestly
   as CANCELLED before motion; nothing moved); reserve request 3; reset.
2. Request 3 read back pending and request 4 was refused; a torn record (half
   a record, no commit) went into the other slot; reset.
3. The journal reopened past the torn record, still pending and still
   refusing, then wiped blank.

Every lease froze KM0. Journal writes took 19–25 ms.

**Settings store (physical, passed).** The store is two slots at `0x342000`
and `0x343000`, with a schema version, CRC and commit marker. It falls back to
the older copy when the newer one is torn. It refuses to write over invalid or
newer-schema data until an explicit reset.

In one capture of 8 cold boots, the operator cut power at random during
continuous saves. All six reopens were LOADED and consistent, and each landed
6–16 generations past the last reported save (bound 25). Saves took at most
28 ms. No cut landed inside a save.

A second run saved back to back (policy `fw15-settings-v2`), and each open
reported the slot it did not load. Across 8 cold boots, six of six cuts
landed inside a save: five found the slot being written erased, and one found
it torn mid-program and reported RECOVERED. Every reopen was consistent,
7–23 generations past the last report (bound 25). Saves took at most 30 ms.
No cut left a partly erased slot; that case is host-tested.

Both stores now enforce wear limits: 100 000 records, about half the part's
rated erase life. The journal still lets a pending request finish.

The FW06 plan has five stages, each with a write-free bench check until the last:

- **A, production constants (host).** `config.rs` defines the production
  watchdog and a 700 ms flash-lease budget. The owner's sample period must
  then stay at or under 139 ms.
- **B, boot handshake (physical, passed).** KM4 announces a fresh session
  (HELLO); KM0 answers with its own (READY). KM4 binds the coordinator only after
  reading back STOP and infrared OFF. A deliberately changed KM0 session
  latched PEER_LOST with outputs blocked. Backup registers were cleared by
  power loss.
- **C, park round trip (physical, passed).** One power-on produced three
  boots:
  1. 100 leases, then KM0 withheld its ACK; the watchdog reset 3.9 s later.
  2. 100 leases, then KM4 withheld its RELEASE; KM0 published TIMEOUT
     after 705 ms (budget 700), and the watchdog reset.
  3. 100 clean leases, then idle with the watchdog fed.

  Across every lease, KM0's heartbeat stayed frozen while parked and advanced
  between leases. The worst park took 6 ticks (about 0.2 ms) and the worst
  release 2. No flash was touched; the park loops run from RAM on both cores.
- **D, mediated read (physical, passed).** KM4 installed its lease provider in
  the SDK flash-lock hooks, then read the JEDEC ID 100 times through the SDK's
  RAM `FLASH_RxCmdXIP` (lock, ROM read-ID in SPIC user mode, unlock). All
  reads returned `c84017` in 100 leases, with KM0 frozen during each. The
  coordinator stayed idle and unfaulted, and the platform did not reset.
- **E, scratch write (physical, passed; owner-approved sector `0x185000`).**
  1. The ID read `c84017` and the sector read blank (1024 words).
  2. Three cycles each erased to blank, programmed 16 pages and verified with
     zero errors.
  3. The final erase left the sector blank.

  All 53 leases froze KM0. Sector erase took 27–35 ms, including lease
  overhead, against the 500 ms active budget; page programs took at most 0.7 ms.

Bench lessons from stage C:

- **SysTick on KM4.** CLKSOURCE (bit 2) is read-only 1 on the Cortex-M33, so
  the park and hook context checks now test only ENABLE and TICKINT. The
  narrower mask refused every park in the first stage C build; diagnostics
  located the cause.
- **Watchdog clock.** The resets came about 4.0–4.2 s after the last refresh,
  an effective clock near 31–33 kHz. FW05 measured about 43 kHz with the same
  counter, and the cause is unknown. The production bounds are now 24–54 kHz in
  source; the installed image predates that change.
- **Interrupted capture.** One capture was cut off by a host crash and is
  kept only as an observation. The first audit of the good capture hit an
  auditor timing bug, now fixed; both results are retained.

## What is actually installed

The research feeder runs the **bench live target review 01** build (FW13 plan 2 probe, identity `8d67a407…`) in **both** banks; it boots bank 1. Before October 22 it ran **FW13 optics review 02** in bank 1, starting at
flash offset `0x186000`. It was installed through the relay bench (122
sectors: 31 before a loader NAK, 91 more with `--interrupted`).

| Identity | Recorded value |
| --- | --- |
| OTA SHA-256 | `064b76290486ca774f28dce826ca2a0e079b84e768b7d2d35cdf0646d8a03842` |
| Embedded startup build ID | `fb1769937b77edbcc092897b8a201fe1ff51f14868b3ee9bd0382e44156f9747` |

**Every power-on runs the Wi-Fi DNS probe, the FW08 transport and the FW13
optics probe**: after the LED sequence it joins the owner's hotspot and reports
health to the local broker as before. For about two minutes the optics task
pulses the infrared emitters in budgeted windows (motor STOP throughout), then
stalls on purpose until the supervisor cuts them, and leaves them off. Wi-Fi and
broker credentials come from private files; the image stays under `private/`.
It drives no motor and writes no flash.

## Last confirmed physical state

This describes the last operator-confirmed session, not a live observation.
Reconfirm it with the operator before any bench operation.

- Scope is the **research PLAF103 only**. The existing household feeder is excluded.
- Feeder USB 5V runs through relay 1 of a DSD TECH SH-UR04A USB relay board
  (CP2102, 9600 8N1, `AT+CHn=1`/`AT+CHn=0`, `AT+CHn=?`): male + to COM1, NO1 to
  female +; GND, D+ and D- pass straight through. Relay off means feeder off.
- Feeder TX (breadboard row 1) goes to adapter RXD and, through the 4.7 kΩ
  resistor (row 5), to COM2; NO2 is on the GND rail. Relay 2 replaces the button,
  which is removed. Feeder RX (row 2) goes to adapter TXD. Relays 3 and 4 are spare.
- `wellfed-bench` drives the relays and chains `wellfed-flash` ([bench control](docs/hardware/bench.md));
  ports are `/dev/serial/by-id/` paths in ignored `private/bench.json`. The relay
  board can take `/dev/ttyUSB0`.
- Physical, October 21, 2026: relay-1 boots reached the Rust bring-up banner and
  relay-2-held boots printed `Flash Download Start` and NAKs, 16 of 16 each, off
  times 0.3-2 s, with or without UART break held during the off time. The earlier
  stuck-entry symptom (fixed by replugging the adapter) did not reproduce.
- Adapter power/VCC and flow-control pins remain unwired. No batteries were in use.
- CON3 remains connected: this motor harness also carries board power.
  The front indicator PCB is difficult to access; no disassembly or wire cutting
  was needed for the successful LED test.

Consult the [UART procedure](docs/hardware/uart.md) before changing wiring.
The old loader/helper session is finished; historical receipts are not resumable.

## Evidence available in this local checkout

Start with **`private/bench-handoff/current.json`**. It indexes the exact local
installed review, frozen build, session, physical result, boot capture, verifier
result, installation audit, baseline, ROM report, and archived research snapshot.
Its file hashes identify the indexed evidence. Dated paths and bench-specific
details stay in that ignored file rather than becoming public fixture conventions.

A fresh Git clone does not contain `private/`, vendor images, captures, or device
backups. It can support ordinary development through the documented setup/checks;
continuing this particular bench session requires the private evidence bundles.
Their absence does not require inventing a new baseline or weakening writer checks.
See [fixtures](docs/development/fixtures.md) and [preserved research](docs/development/provenance.md).

## Review fixes included since FW04

These review fixes landed on top of `c8bb49f` with static and host evidence. The
installed FW05 probe build includes them. The FW05 capture exercised only the
timer-repeat and stopped-output paths; the others have no hardware evidence:

- Infrared OFF requires LOW output latches as well as LOW inputs, because Port A
  input readback is not trustworthy. PA15/PB3 readback is still unmeasured.
- A determined initialization or recovery fault holds verified STOP and keeps
  refreshing the watchdog instead of reset-looping; a hang still resets.
- The owner refuses flash limits whose total lease can exceed the watchdog
  progress gap. Production limits need measured erase/program times (FW06).
- One timer IRQ without systimer advance is tolerated; two consecutive fault.
- `Events::close()` turns infrared off; the KM4 panic handler requests STOP
  and infrared off; SysTick checks ignore COUNTFLAG.
- `build.py` refuses SDK worktree drift outside an allowlist, Rust build
  overrides, and toolchain file-hash mismatches. The `plaf103` and
  `plaf103-firstboot` private builds were rebuilt afterwards.

## Next work and retained decisions

FW14 continues once batteries are available (above).

**FW07 stage B (physical, passed).** Radio on and scanning work on our
firmware: `wifi_on(STA)` took about 235 ms, and three scans took 4.0–4.3 s each,
hearing 51–63 networks; outputs stayed stopped. The driver prints
`FW Enable failed`, caused by the KM0 WLAN firmware not running (fixed in
stage C). The owner's hotspot was most likely off during those scans (it
was off when checked before stage C). KM4 log lines now print
with the scheduler suspended, because a driver print split one in the first
capture.

**FW07 stage C (physical, passed).** Review 02 joins the owner's hotspot,
gets a DHCP lease, notices the hotspot going away, and rejoins with bounded
backoff when it comes back. Two fixes got it there:
- KM0 now starts the WLAN firmware (`wifi_FW_init_ram()`), whose absence
  caused `FW Enable failed` and `fw cmd fifo full`.
- The bench host's nftables input policy (`/etc/computer-security.nft`) dropped
  everything on the hotspot interface. The owner allowed DHCP from it; DNS and
  forwarding from the hotspot are still blocked there.

Details are in docs/firmware/startup-diagnostics.md. The hotspot is a
NetworkManager connection on the bench host (`nmcli connection up|down
<connection>`, named in `private/hotspot.json`) and is off unless the owner
starts it. The UART recorder caps a capture at 180 s. A host packet capture
(`sudo tcpdump -i <hotspot interface>`) is the quickest way to tell feeder
problems from host problems.

**FW07 stage D (physical, passed).** The owner's host firewall now allows DHCP
and DNS from the hotspot subnet; forwarding stays blocked (the feeder has no
internet). The probe sends its own DNS query every 5 s, since lwIP's resolver
cache hid the first DNS outage. Two failures are a DNS loss and trigger a
rejoin, and the backoff resets only after a successful lookup. An operator DNS
block (`sudo nft insert rule inet computer_security input iifname "<hotspot
interface>" udp dport 53 drop`, undone with `sudo systemctl reload
computer-security`) was detected in 11 s and recovered 10 s after unblocking.

**FW07 runtime flash check (physical, passed).** Full 8 MiB reads before and
after one drill boot (hotspot outage plus DNS block) were byte-identical.

FW07 is Done (no-renewal scope). DHCP-server loss and lease renewal are carried
to FW27.

**FW08 stage A (offline, done).** The pieces:
- `wellfed-core` `transport.rs`: the MQTT subset, session, request parser and
  health/reply writers.
- KM4 `transport.rs`, `health.rs` and `net_status.rs`: a second task beside the
  Wi-Fi probe.
- C TCP helpers in `example_entry.c`.
- The server's firmware role, `firmware-credential` command and API.
- A structural no-dispense check in `tools/check-repository.py`.
- `audit-firmware-transport.py`.

`private/fw-mqtt.json` now exists. `private/server.json` gained a `firmware`
section; `private/server.json.before-fw08` is the copy from before. Health and
the will are not retained, because the broker has retain off.

**FW08 stage B (physical, passed).** The installed image is FW08 transport
review 01. The owner's firewall now also allows the broker port from the
hotspot subnet. `wellfed-local serve` runs in the owner's terminal.

**FW08 stage C (physical, passed); FW08 is Done (read-only scope).** One boot
survived a 20 s hotspot outage and a server restart, reconnecting each time.
Full-flash reads before and after were identical. The server can't see an
access-point outage faster than the 45 s keepalive or 75 s stale timeout.
Refusal of unsupported requests is host-tested only.

**FW13 and FW31 Done** (see the top of this file). **Next:** FW14 needs
batteries; FW30 (electrical output checks with a meter) is ready and now
includes PB3's driven OFF level and emitter current.

`--interrupted` now also accepts one sector holding the staged target's first
1–3 program frames followed by erased flash, the state a NAK during programming
leaves. The owner approved this narrow change after a stop at 0x1d6000.

The FW07 plan from the static survey, written before stage A (stages A and B are
now done; the "Today" item describes the build before them):

- **Today.** Wi-Fi is compiled and linked but garbage-collected: neither our
  KM4 main nor our `example_entry` calls `wlan_network()`, and KM0 skips
  `wifi_FW_init_ram()`. The bindings have no Wi-Fi or LwIP functions, and
  FTL is not compiled.
- **Hazards to close before the radio runs.**
  - The SDK's default `FAST_RECONNECT_DATA` (0x105000), `AP_SETTING` and FTL
    sectors lie inside the application region. Override
    `CONFIG_EXAMPLE_WLAN_FAST_CONNECT 0` (which also disables fast DHCP), and
    audit that the image has no flash erase/stream-write callers and a NULL
    reconnect-write hook.
  - Never link or call `rtw_efuse_boot_write`. The stock main calls it, and
    it can program a random MAC into OTP, which is irreversible.
  - `LwIP_DHCP` falls back to a static 192.168.1.80 after 5 tries. Treat
    that as a failure. The SDK's own reconnect gives up after 8 tries, so our
    own bounded loop must own recovery, with no resets or flash writes (unlike
    stock).
- **Stages.**
  - A, offline: our own init task (LwIP, then `wifi_on(STA)`), a small C shim
    for join and status, the `firstboot.py` call-order audit, the platform
    options override, and the flash/eFuse link audit.
  - B, bench: radio on and a scan reporting only whether the configured
    network is present and its RSSI.
  - C, bench: join, DHCP and reconnect after the access point goes away and
    comes back.
- **Credentials.** Build-time injection from `private/` (`private/hotspot.json`
  exists) into an ignored generated source, with only a hash in the manifest.
  Review the SDK's own logs for the SSID.
- **Open decisions.**
  - Settled: KM4 STA needs KM0 `wifi_FW_init_ram()`. It now runs next to
    our park/peer task, and a park masks it along with everything else on KM0.
  - Hidden flash users that bypass `flash_api` (and so the FW06 hooks).
  - Driver interrupt load against the owner's supervision timing.

Build and freeze each candidate, install through the
changed-sector updater after the operator enters loader mode, and capture boots
receive-only. Start the recorder and confirm it holds the port before the
operator powers on; a capture that begins mid-boot cannot be audited.
Do not silently turn a failed input observation into a verified electrical level.

Retain the owner's development preferences:

- Use the fast [changed-sector UART updater](docs/development/uart-development.md).
  Routine iterations do not require a new full-flash read or stock round trip.
  Keep fresh helper/source/guard/status checks, exact plans, and readback verification.
- Preserve the demonstrated conservative recovery path. Stock return through the
  newer development updater remains untested; no automatic retry/replay is allowed.
- OTA can wait because UART iteration is faster now; it remains planned for
  later autonomous development iterations.
- FW03 electrical voltage, infrared-off, measured-reset and transient checks were
  deferred to FW30, which now gates supervised motion; visible LEDs and software STOP readback do not complete them.

These are recorded scope decisions, not authorization to operate hardware or
publish changes. Follow the current user task and repository instructions.
