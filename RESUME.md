# Resume firmware development

Handoff checkpoint: **October 24, 2026 (late night), Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) (stock parity: [FEATURE_BOARD.md](FEATURE_BOARD.md)). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**October 24, late night: FW45 BLE setup on hardware (physical, owner pressing; no motion).**
- **Runs:** bounded `serve`, stopped afterwards.
  - `ota fw45-setup-01` (v108, `46255eec`) confirmed.
  - `capture fw45-setup-02`: Lock and Mic held, `wellfed setup`, Dispense pressed: enrolled (`enrolled=1`), Wi-Fi saved, `Done`, reboot without BLE, rejoin with `source: stored`; `PLID1 8019237d8eba8b9d` matches the CLI.
  - `capture fw45-setup-03`: locked chord refused; the no-press refusal was sent but the link dropped before the client read it, and the CLI stalled in BlueZ's disconnect.
  - `ota fw45-setup-04` (v109) fixed both (2 s linger, bounded hang-up); `capture fw45-setup-05`: the CLI reported the `unconfirmed` refusal.
  - A signed clear emptied the Wi-Fi store (generation 20); then setup mode's 15-minute idle reboot ran and the feeder came back with `source: build`.
- **Flash now:** **Bank 1:** `feeder` v109 (confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 110. The client store holds one owner: the bench CLI's key in `private/wellfed-owner.json`.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW45 broker credentials, or FW30 pins with the meter. Mains-only advertising is host-tested only (check on batteries).

**October 24, night: FW45 BLE setup mode (host and image builds; nothing run on the bench).**
- **Owner decisions:** a Lock and Mic hold (5 s, unlocked panel) starts setup mode; no automatic setup after a Wi-Fi loss; the `ble` firmware, the feeder's `setup` variant and the HMAC BLE exchange are retired.
- **Code:** `wellfed-proto` (setup messages, postcard), `wellfed-feeder::setup::Responder` (Noise XX under the identity), `wellfed-client` (the client session and BLE framing), the owner CLI `wellfed setup` (BlueZ through `bluer`, key file `private/wellfed-owner.json`), `wellfed-hal::ble` as a plain framed link, the keys mailbox reporting results (`ticket`/`outcome`), `network::save`, and the loop's setup controller (`feeder/src/setup.rs`). `wellfed-formats::private` now holds the 0600 JSON writer and the random source that `wellfed-local` used.
- **Why not at boot:** every free KM0 backup register is taken, so the gesture starts the stack on the running feeder and a reboot still ends it ([Bluetooth](docs/firmware/bluetooth.md)).
- **Checks:** `make check` (904 tests, cargo-deny with `bluer` and `postcard`), all 13 firmware images and variants with their audits (loop stack bound 4,076 of 6,144 B), fresh `qa` builds and `make check-fixtures`.
- **Flash and bench state:** unchanged from the evening entry (`feeder` v107 on bank 1). **Next manifest version:** 108.
- **Next (owner pressing):** OTA `feeder` v108; hold Lock then Mic on an unlocked panel; `wellfed setup --ssid <lab hotspot>`; press Dispense; expect `Done`, `PLCFG1 keys change=enroll result=done enrolled=1`, a reboot without BLE and a join with `source: stored`; then a 60 s no-press refusal and a locked-panel refusal. Clear the Wi-Fi store afterwards.

**October 24, evening: board close-out; FW21, FW25 and FW34 done (FW25 physical, no owner action).**
- **Board:** FW21 closed at sync and jump scope (the long-uptime restart goes to FW46). FW34 closed at revised scope (boot-gated BLE and mains-only advertising go to FW45). FW25 closed at Wi-Fi and options scope (broker credentials go to FW45).
- **Run `fw25-switch-02`:** a bounded `serve` plus a temporary second hotspot profile `Petlibro-Lab-B` on `wlan1` (own SSID and random passphrase, same 10.42.103.1/24, so the firewall rules held and `wlan0` was untouched). A signed `wifi` request stored B, the hotspots were swapped, and the feeder joined B with `source: stored` in 15 s. A signed clear on B, then swapping back, rejoined the lab hotspot with `source: build` in 20 s, with no reboot. The profile was deleted and `serve` stopped. (`fw25-switch-01` stopped before any request: its script expected `source: build`, but FW34's setup run had left the lab hotspot's own credentials stored.)
- **State change:** the Wi-Fi store is now empty (generation 18), where before it held the lab hotspot's credentials from `fw34-setup`. The feeder joins the same hotspot with the build's credentials.
- **Flash now:** unchanged (`feeder` v107 on bank 1, `qa` `ef76b85e…` on bank 0). **Next manifest version:** 108.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW45 BLE setup, or FW30 pins with the meter.

**October 24, afternoon: FW45 keys on the panel, FP15 key reset (physical, owner pressing; no motion).**
- **Code:** `wellfed-hal::keys` (the loop-to-writer key mailbox and the identity handoff), the writer's `keys change=` path, the loop's `PLID1` identity fingerprint, and pairing ahead of the panel in the feeder's owner. Stack bounds: `wellfed-cfg` 2,948/4,096, loop 3,912/6,144.
- **Runs:** bounded `serve`, stopped afterwards.
  - `ota fw45-pair-01` (v107) confirmed, with the identity loaded.
  - `boot fw45-pair-02`: the same fingerprint.
  - `capture fw45-pair-03`: the recovery chord was refused while locked and opened the window once unlocked. The owner's Reset steps landed on Lock, because our notes had the button order reversed.
  - `capture fw45-pair-04`: a locked Reset hold was refused. Unlocked, the keys reset and a new fingerprint appeared.
  - `boot fw45-pair-05`: the new fingerprint persisted.
- **Owner, panel order seen from the front:** the buttons run Lock, Dispense, Mic, Reset from left to right, and the indicators low food, lock, Wi-Fi, error/warning. Recorded in the board notes.
- **Flash now:** **Bank 1:** `feeder` v107 (`e1215de0…`, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 108. The client store holds a new identity, with no clients.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW45 BLE setup (`wellfed-proto`/`wellfed-client` and a BlueZ host command). Alternatively, FW30 pins with the meter and the analyzer.

**October 24, morning: FW45 generator and keys in the firmware (physical, no owner action).**
- **Owner-approved migration (FW51):** `private/feed-ledger.json` renamed to `feed-ledger.json.before-fw51` (every trial finished; `ota_hold` was true). `max_portions` removed from `private/server.json`, with a backup in `server.json.before-fw51`.
- **Runs:** bounded `serve`, stopped afterwards.
  - `ota fw45-keys-01` stopped before the offer: the pre-migration config was refused.
  - `ota fw45-keys-02` (v106, confirmed) logged `PLRNG1 boot result=ready samples=1024` and `PLCFG1 keys seed=empty saved=1 identity=created`.
  - `boot fw45-restart-01`–`16`: every boot was ready, with distinct first samples and checks, and logged `seed=loaded mixed=1 saved=1 identity=loaded`.
- **Flash now:** **Bank 1:** `feeder` `a675ccc5…` (v106, confirmed), carrying the generator and the stored identity. **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 107. Storage sectors 7–10 now hold the seed and the identity.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW30 pins 2–4 with the owner at the meter, then the transients with the logic analyzer, then FW16. FW45 continues with pairing.

**October 24: FW51 done, the research paths out of `wellfed-local` (host only, nothing run on the bench; [tools roadmap](docs/development/tools-roadmap.md)).**
- **Removed:** stock control (`feed`, `resolve`, `new-request-id`, `/api/feed`), the C-probe and modified-stock trials (`probe-query`, the `1.1.16`/C-probe images, `ota-images --profile`), `speak` and `ble-wifi`. The install trial's guards and no-resend rule are unchanged.
- **Before the next `serve` (owner, once):** `wellfed-local serve` now refuses to start beside `private/feed-ledger.json`. Check that no trial in it is submitted, uncertain or failed, then rename it (for example to `feed-ledger.json.before-fw51`); the new ledger is `private/stock-ledger.json`. Also delete the `max_portions` line from `private/server.json`, which the config now refuses as an unknown field.
- **Flash and bench state:** unchanged from the FW45 entry below (`feeder` v104 in bank 1).

**October 24, night: FW45 noise sources measured (physical, no owner action).**
- **Runs:** bounded `serve`, stopped afterwards.
  - `ota fw45-rng-01 --variant entropy` (v102) lost its raw lines: they overflowed the 192-byte record buffer; now 112 samples a line, with a test of the longest line.
  - `ota fw45-rng-04` (v103), then `boot fw45-rng-05` and captures `fw45-rng-06`–`11`. Each observe segment ends at the flash tool's 512 KiB limit, so the bench caps a single run at about 110 s of this stream.
  - `ota fw45-rng-restore-01` put `feeder` back as `76c9013e…` (v104, confirmed, health clean).
- **Result:** NIST `ea_non_iid` (built in the session scratchpad, not tracked) gives the ADC supply channel's low byte 1.44 bits a sample and SysTick-vs-32 kHz jitter 0.19 (control: `/dev/urandom` 6.92). The on-device health tests passed on both at 1 bit. Claim: 1 bit per ADC sample ([pairing](docs/firmware/pairing.md#measured-noise-physical-october-24)).
- **Flash now:** **Bank 1:** `feeder` `76c9013e…` (v104, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 105.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW45 wiring: share the ADC with the power monitor, run the restart test, start the generator at boot, then the client store and identity.

**October 24, early morning: FW45 groundwork (host), FP01 done (static), builds 6× faster; bench restored to `feeder`.**
- **Runs (physical, no owner readings):**
  - After the dock restart (relays and hotspot dropped), the hotspot was brought up again, and `boot fw30-restore-01` confirmed `optics` v99 (`d1184f87…`). `ota fw30-optics-02`/`03` had failed earlier on a serial-adapter I/O error (EPIPE); v99 had already confirmed.
  - `boot fw30-pin2-02` and `fw30-pin2-03` ran the optics windows for CON2 pin 2, but **no readings were recorded: pin 2 is still open**, then pins 3 and 4.
  - Restore: `ota fw30-restore-02` stopped before the offer (no server running), `boot fw30-restore-03` powered the feeder on, and `ota fw30-restore-04 --build --firmware feeder` installed `feeder` `def51900…` (v101, confirmed in 3 min, health clean), with a bounded `serve`, stopped afterwards. The operator script was removed.
- **Code (host and build):**
  - **Builder:** overlays are written only when their bytes change. An unchanged rebuild went from 313 C compiles (35 s) to 7 (about 6 s), with identical images. The write audit no longer reads a literal-pool word as a branch (a `CmdLogBuf` string address had decoded as `bvs.n <cmd_efuse_protect>`).
  - **FW45:** no hardware RNG in the SDK (static), so `wellfed-core::rng` (90B health tests, SHA-256 conditioning, HMAC-DRBG), the seed store (sectors 7–8), the client store (9–10), `wellfed-feeder::pairing` and Noise XXpsk3. None of it is wired into the firmware yet ([pairing](docs/firmware/pairing.md#left-for-fw45)).
  - **Storage:** every sector is named in `wellfed_amebad_sys::storage`.
  - **FP01:** stock's limits and defaults for every feature card ([stock behavior](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)).
  - **Checks:** `make check`, `make check-fixtures` and all 13 firmware images pass.
- **Flash now:** **Bank 1:** `feeder` `def51900…` (v101, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 102.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW45's entropy sampler and a bench capture of raw samples; FW30 pins 2–4 with the owner at the meter.

**October 23, latest: FW44 done, the product resource budget (physical, no owner action; [resource budget](docs/firmware/resource-budget.md)).**
- **Runs:** bounded `serve` throughout, stopped afterwards.
  - `ota fw44-budget-01 --build --firmware feeder` (v94): every trial boot hit STKOF in `wellfed-loop` (4 KiB) and KM0 rolled back to bank 0. Cause: the stack audit skipped `subw` frames; fixed, `wellfed-net` raised to 8 KiB, Noise public keys via the X25519 ladder, loop stack 6 KiB. Every firmware and variant rebuilt under the corrected audit.
  - `fw44-budget-02` (v95, confirmed): `PLAPI1` handshake and heap records; `capture fw44-budget-03` (heap flat at 146,464 free).
  - `fw44-setup-01 --variant setup` (v96, confirmed): BLE stack up, least free heap 123,136. `fw44-budget-04` (v97) restored the normal build (reused v95's review).
- **Static (subagent, Oct 23):** stock 1.1.15 has no fixed or bounded flash access in 0x306000–0x3e8000; it block-erases 0x3f0000–0x400000 on its clock save. Scripts in the session scratchpad, not tracked.
- **Flash now:** **Bank 1:** `feeder` `3edf8bce…` (v97, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 98.
- **Bench state:** feeder on, hotspot up, no server.
- **Owner decision:** extend the storage region to 32 sectors (0x340000–0x360000); stock's clock-save block 0x3f0000–0x400000 added to `layout.json`'s preserve ranges.
- **FW50 (static):** board pins and ADC channels in `wellfed_amebad_sys::board`; [device layer](docs/firmware/device-layer.md) lists the rest.
- **Next:** FW45 on the extended region.

**October 23, late night: FP11 child lock done on hardware (physical, owner pressing and watching PA4).**
- **Runs:** `ota fp11-panel-01 --build --firmware feeder` installed `feeder` `055f9afa…` (v93, confirmed in 3 min 40 s), with a bounded `serve`, stopped afterwards. Boot: `PLPANEL1 start lock=locked`, both firmware tasks created (`wellfed-loop`, `wellfed-key`). `capture fp11-panel-03` (180 s; `fp11-panel-02` caught no presses): Dispense and Mic refused while locked, a 5.5 s Lock hold unlocked, the panel relocked on its own after 30 s idle, then two signed `lock` unlock/lock pairs through the API, each logged `source=remote`. The owner saw PA4 on while locked and off while unlocked at every step.
- **Code:** the `wellfed-feeder` policy crate (`src/product/`), the `feeder` product firmware, the signed `lock` op (core parser, HAL mailbox `panel.rs`, `wellfed-local` API), and `wellfed-bench ota --firmware feeder`. `make check`, `make check-fixtures` (fresh `device` and `firstboot` builds) and the `feeder` image audits pass.
- **Flash now:** **Bank 1:** `feeder` `055f9afa…` (v93, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 94.
- **Bench state:** feeder on, hotspot up, no server.
- **Next:** FW44 (product resource budget) on the `feeder` image, then FW50.

**October 23, night: a complete passing QA run (physical, owner answering).** `ota qa-20261023-01 --build` installed `qa` `645c1cd4…` (v92, with the task budget), with a bounded `serve` that was stopped afterwards. `boot qa-20261023-02-guided`: every check and item passed, and `wellfed-audit qa` graded it `pass`. **Flash now:** **Bank 1:** `qa` `645c1cd4…` (v92, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 93. **Bench state:** feeder on, hotspot up, no server.

**October 23, late evening: FW30 meter work paused; the two-task firmware budget is enforced (`d6c57b9`).**
- **Runs (physical, owner probing):** `ota fw30-optics-01 --build --firmware optics` (v91, confirmed, with a bounded `serve`, stopped afterwards). Then cold boots with an operator script on the bench page (`private/bench-status/script.json`, removed afterwards): `fw30-pin1-01`, where **CON2 pin 1 read 0 V in every emitter state (ground)**, and `fw30-pin2-01`, captured but with the owner's six readings not yet recorded. The `fw30-pb3-01`–`07` captures were aborted restarts.
- **Learned:** the optics run starts within about a second of the first boot output; `boot --off-seconds 15` gives the operator a lead. Its windows are fixed at 5 s by `fw13-optics-v2`, so one cycle (30 s) covers every state for one pin.
- **Code:** `wellfed_sdk::FIRMWARE_TASKS` = 2 budgets `spawn` and `buttons::start`, and the stack audit refuses an image that links more than two firmware tasks.
- **Flash now:** **Bank 1:** `optics` (v91, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 92.
- **Bench state:** feeder off (relay 1 off), hotspot up, no server; the owner's `network apply` rules as before.
- **Next (FW30):** pin 2's readings from the owner, then pins 3 and 4 (one 30 s cycle each), then the transients when the logic analyzer arrives (Saturday). Polarity: carry to FW16's first pulse, or trace it, which is the owner's call.

**October 23, afternoon: FW20 done, the SDK button service on hardware (physical, owner pressing).**
- **Runs:**
  - `fw20-keys-01` (`ota --build --firmware buttons`, with a bounded `wellfed-local serve`): v87 confirmed; `PLKEY1 open inputs=1 pressed=none`.
  - `capture fw20-keys-02` (180 s) and `fw20-keys-03` (30 s): a short press, short of a hold, level-1 and level-2 holds, growing chords and a broken chord, each also read back through the queue (`PLKEY1 read …`), none dropped ([buttons](docs/firmware/buttons.md)).
- **Code:** `wellfed_core::buttons` (gestures), `wellfed_hal::buttons` (poll, queue, `PLKEY1`), `wellfed_sdk::buttons::start/next`; the stack audit reads `SDK_TASK_SOURCES`. Fresh `qa` builds, `make check-fixtures` and `make check` pass.
- **Flash now:** **Bank 1:** `buttons` `5898dc09…` (v87, confirmed). **Bank 0:** `qa` `ef76b85e…`. **Next manifest version:** 88. Back to QA: `ota … --build` (default firmware `qa`).
- **Bench state:** feeder on, hotspot up, no server; the owner's `network apply` rules as before.
- **Next:** FP11 (child lock) is ready on the button service; FW42 setup-frame test.

**October 23, early afternoon: the stock round trip runs UART-free, and FW35, FW37, FW39, FW40 and FW41 are done (physical, owner at the bench for QA, away for the rest).**
- **Runs:**
  - `fw40-stock-ota-07` (`stock-ota --power`): stock 1.1.15, which the owner had put back, installed `qa` `ef76b85e…` in 40 s. It booted from bank 0, reported recovery-role health (first time with stock), and was promoted to bank 1 as v84, which confirmed. On the trial boot: `PLWIF3 hold`, then `PLQA1 check stage=optics status=pass`.
  - `fw39-qa-03-guided`, `fw39-qa-04-guided` (`boot --seconds 180`, at most 180 s): `qa_fail` each. 03 failed PA0 (Lock mis-pressed); 04 failed Reset (no press in 30 s). Between them every check and item passed; the owner accepted the pair.
  - `fw41-roundtrip-01` (`itest --only stock-roundtrip --power --stock fw01-stock-review-20261020-01`): passed in 6 min 31 s. Stock v85 via our updater, then stock installed v84 into bank 0, promoted as v86.
  - `fw35-live-01` (smoke tier, all pass) with the page's `/api/feeder` and `/api/status` sampled every 5 s (`private/verification/fw35-live-01-page.jsonl`).
  - `make check-fixtures` against fresh `device`/`firstboot` builds: 25 tests, none skipped.
- **Code:** the bench page has a guided-QA panel (Overview) that coaches a QA run from its `PLQA1` records; the parser moved to `wellfed_formats::qa`, shared with `wellfed-audit qa`.
- **Flash now:** **Bank 1:** `qa` `ef76b85e…` (last offer v86, confirmed). **Bank 0:** the same build, written by stock. **Next manifest version:** 87.
- **Bench state:** feeder on (relay 1), hotspot up, no server. The owner's `wellfed-local network apply` rules are still in place; `sudo wellfed-local network remove` clears them.
- **Next:** FW42's setup-frame test (owner puts stock in setup mode), or FW43. Owner ideas for QA speed: end the capture at `PLQA1 result`, and run the guided checks before optics.

**October 23, morning to afternoon: stock 1.1.15 installs the replacement over its own updater (FW40, physical; built on branch `fw40-followup` in the `stock-ota-poc` worktree).**
- **Runs:**
  - `fw40-stock-ota-02`: stock accepted the request but could not reach the image port, and reported `OTA_INFORM` error 2003; it stayed on stock. Cause: `network apply` opened only 1883. It now also accepts 8088 from the feeder.
  - `-03`/`-04`: the failed request could never be resolved, for two reasons: stock's ~61 s heartbeat restarted the server's 60 s quiet window, and idle stock reports no version. Fixes: the bound is now 90 s, and a refused resolve sends a read-only `ATTR_GET_SERVICE`. That request was closed by hand at the owner's choice (ledger backup `private/feed-ledger.json.before-fw40-manual-resolve`).
  - **`fw40-stock-ota-05`:** stock downloaded `c33e1e6b…` in about 40 s, and it booted from bank 0. Its recovery role then polled `no_offer` with no health (fixed since).
  - **`fw40-stock-ota-06-promote`** (`itest --only ota-update`): the bank-0 role installed the build into bank 1 (v77). Boot 1 hard-faulted in the optics probe; boot 2 confirmed.
  - **Fault traced** (`fw40-diag-01`, `-02-nowifi`, `-03-wifi`): `PLOPT1 error kind=supervisor fault=7 stall=703` on the trial boot, `stall=702` on a cold boot with the hotspot up, and every window passing with it down. The first WPA3 join starves the QA task past its 300 ms lease.
  - **Fix confirmed** (`fw40-diag-04`, OTA trial boot, v79): `PLWIF3 hold`, all 19 optics windows, `PLQA1 check stage=optics status=pass`, then the join and confirmation.
- **Code** (host-tested, every audit passing):
  - recovery-role health (`"role":"recovery"`) while the updater waits;
  - `stock-ota` promotes by default (`--no-promote`);
  - the Wi-Fi hold until `startup_done` (180 s cap; bank 0 never waits);
  - `PLFLT1` fault registers and `PLOPT1 … stall=`;
  - `wellfed-bench stock-prepare`/`stock-execute` (UART);
  - `itest` `stock-roundtrip --stock <stock review>`;
  - [consumer installation](docs/firmware/consumer-installation.md): stock's setup protocol, install flow, rollback design and architecture.
- **Not yet physical:** recovery-role health with stock; `stock-roundtrip`; FW41's stock install; `stock-prepare`/`stock-execute`; the setup-frame test.
- **Flash now:** **Bank 1:** `qa` with the Wi-Fi hold `96fa5d51…` (OTA v79, confirmed). **Bank 0:** `c33e1e6b…`, written by stock (no recovery-role health). Stock is in neither bank. **Next manifest version:** 80.
- **Bench state:** feeder off, hotspot down, no server. The owner may still have `wellfed-local network apply` rules in place; `sudo wellfed-local network remove` clears them.
- **Next:**
  - `itest --only stock-roundtrip --stock <stock review> --power` with a fresh build (the owner runs `network apply` when it reaches stock);
  - then the Phase 6 setup-frame test (owner present to put stock into setup mode).

**October 23, about 6:30 AM: stock 1.1.15 restored over UART (owner request, physical, owner present).**
- **Runs** (release binaries current):
  - `give-way-prepare/-execute stock-20261023-01-giveway` (fallback `itest-20261022-18-good-review`): one 1 KiB frame at `0x186000`; bank 0 `f244d9a9…` booted.
  - `loader stock-20261023-02-loader`, `wellfed-flash read small`/`read full` (`stock-20261023-02-bounded`, `-full`), `stock-recovery prepare` (204 sectors, all in 0x186000–0x252000, plan `d3e85d06…`) and `execute` (`stock-20261023-02-recovery-execute`): initial and pre-marker 8 MiB reads matched (`snapshot-02` `e236dc09…` = the staged snapshot), every sector read back, marker written. The final full read was interrupted (journal and session end `stopped`), as in FW38.
  - `boot stock-20261023-03-boot`: `app_main:version:1.1.15`, no `PL…` records. Stock prints the unit's secrets; keep that capture private.
  - About 65 minutes end to end; the overnight FW21 expiry run was dropped for this.
- **Also (host):** FW41, `wellfed-bench ota --review <stock review>` back to stock over the air; not yet run on hardware.
- **Flash now:** **Bank 1:** stock 1.1.15 (UART restore). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 73. Back to a replacement: UART (`flash-prepare … --bank 1` from the stock review, as `fw38-stock-04`) or FW40.
- **Bench state:** feeder off (relay off), hotspot down, no server, batteries out.

**October 23, about 5 AM: FW14 done (power in health, physical over the air, owner at the bench).**
- **What changed:** the SDK runtime's power monitor (`wellfed-hal` `power.rs`, task `wellfed-pwr`) samples ADC channel 5 and PA19 every 5 s once the startup probe releases the ADC; health gains `power` (limits now 704 B health, 832 B packet). The `adc` and `qa` firmwares claim the ADC. UART tag `PLBAT1` (`PLPWR1` is KM0's shared-pad record; the first install used it by mistake). Write audit fix: a literal decoded as two 16-bit instructions now counts as a reference (`662686a`).
- **Runs:** `itest-20261023-fw14-01` (v71, `PLPWR1` tags) and `itest-20261023-fw14-02` (v72, review `fw14-power-04-review`, `optics` firmware), both confirmed. Then a bounded session (relay on, hotspot up, `serve` under 5 min): health `power` `null` for about 100 s, then `{"adapter":true,"age_ms":3787,"battery_mv":null}`.
- **With a pack:** the owner fitted three D cells while serving: `"adapter":true,"battery_mv":4862`; adapter pulled: `"adapter":false,"battery_mv":4755` (meter earlier 4.85 / 4.77 V). FW14 is Done (no-load scope); motor current is carried to FW16.
- **Pre-existing, not FW14:** `PLOPT1 error kind=supervisor n=1 fault=7`; the `make check-fixtures` emulation tests refuse the pre-FW37 profile name in the `private/fixtures` reviews.
- **Flash now:** **Bank 1:** `optics` with the power monitor (OTA v72, confirmed). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 73.
- **Bench state:** restored. Batteries out, feeder off, hotspot down, no server.

**October 23, later: BLE Wi-Fi setup replaces the BLE speaker (FW34, physical over the air, by BLE and by UART capture; uncommitted on master).**
- **What changed** ([design](docs/firmware/bluetooth.md)):
  - The audio demo is gone. The `ble` firmware advertises after 120 s offline and takes one signed FW25 `wifi` request per hello over a private GATT service.
  - `wellfed-local ble-wifi --ssid NAME` sends it, with the passphrase on stdin.
  - A credential save mid-boot is now joined at the next attempt with a fresh count of 5 (`network.rs`), for MQTT too.
  - The client polls BlueZ's device list without a discovery filter: `bluetoothd` 5.87 segfaulted twice around a filtered discovery.
- **Runs:**
  - `fw34-setup-01` (`itest --review private/verification/fw34-setup-01-review --only ota-update --power`) installed it as v70, and it confirmed.
  - `boot fw34-setup-02-offline` with the hotspot down: `PLBLE1 ready heap_free=131072`, then `advertising on=1` about 124 s after boot.
  - `capture fw34-setup-03-provision`:
    - `ble-wifi` sent the hotspot's credentials; the reply was `ok:true,pending:true,writable:true`.
    - UART: `request op=wifi ok=1` and `net saved generation=16 stored=1`, with the link up through the flash write.
    - With the hotspot brought up, join attempt 14 connected with source `stored` (`kind=connect value=00000001`); the build's credentials had already failed 13 times. DHCP and DNS followed, then `advertising on=0`.
  - Stored credentials are now the hotspot's own (the same network as the build's); a signed `wifi` clear returns to the build's.
- **Checks:**
  - `make check` passes. The `consumer-installation.md` links master carried without the file are removed.
  - Host tests: `wellfed-hal` `ble`, `network` and `records`; `wellfed-local` `wifi_ble`.
  - Fresh `ble` firstboot with every audit passing (task 1568 of 4096 B, image 832 KB).
- **Flash now:** **Bank 1:** `ble` setup build (OTA v70, confirmed). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 71.
- **Bench state:** restored. Feeder off (relays off), hotspot down, no server.
- **Next:** advertise only on mains power (PA19), as stock does; put BLE behind a settings flag; OTA QA back if needed.

**October 23: BLE bring-up on the feeder with a BLE speaker (FW34, physical over the air and by UART capture, owner listening).**
- **Runs:**
  - `fw34-ble-02` (`itest --review private/verification/fw34-ble-01-review --tier smoke --power`) installed `ble` as v69. It confirmed and passed `ota-update`, `requests` and `request-auth` with BLE up.
  - `boot fw34-ble-03-speaker`, then `fw34-ble-04-speaker`:
    - BlueZ saw `Wellfed` advertising, and the link settled at MTU 247, data length 251, 2M PHY and a 7.5–15 ms interval.
    - A signed 10 s, 16 kHz stream played clean (owner listening): `done bytes=320000 underruns=24 dropped=0`.
    - The host client needed `notify_io()` (the hello raced `notify()`) and write requests (`bluer` writes default to commands).
  - Phones cannot use it: it is LE-only and needs the request key.
- **Flash then:** **Bank 1:** `ble` speaker build (OTA v69, confirmed). **Bank 0:** unchanged.

**October 23, morning: bring-up firmwares and QA (FW39, physical over the air and by UART capture, owner pressing).** Built in the `firmware-split` worktree, applied onto master after FW38.
- **What changed:**
  - The reference firmware is gone. In its place are `optics`, `buttons`, `adc`, `speaker`, `motor` (pulse only in its `fw16-pulse` variant) and `trial` (`fw11-reset`, `fw11-fail`), plus a guided `qa` firmware, now the default for `wellfed-build`, make and the bench ([firmwares](src/firmware/wellfed/README.md), [QA](docs/firmware/qa.md)).
  - The probes moved unchanged to `src/sdk/wellfed-probes`. Policy names, markers and record formats are unchanged.
  - New `wellfed-audit qa`.
  - The bench takes `--firmware`. `ota-recovery` builds `trial`/`fw11-fail`, and `ota-power-cuts` builds `speaker`.
  - The builder refuses a motion policy outside a variant.
- **Checks:**
  - `make check` and `make check-fixtures` pass, with no skips, against fresh `qa` `device` and `firstboot` builds. The emulated factory loader accepts the QA build in both banks.
  - Fresh `firstboot` builds of every firmware and variant pass every audit. Firmware task stack bounds (bytes):
    - `qa` 2596/8192, aud 644/4096;
    - `optics` 2168/8192;
    - `buttons` 704/4096;
    - `adc` 760/8192;
    - `speaker` aud 900/4096, spk 1720/4096;
    - `motor/fw16-pulse` 1860/8192;
    - `blink` 748/4096.
- **Runs:**
  - `itest-20261023-05 --build --tier smoke --power` passed. QA `201e76ff…` installed as v68 (`ota-update` 4 min 00 s), confirmed on its trial boot, then passed `requests` and `request-auth`. Its review `itest-20261023-05-good-review` is the new QA baseline.
  - `boot fw39-qa-01-guided`: the 180 s capture ended during the button prompts. The owner was still learning the layout: Feed was pressed for Reset and Mic.
  - `boot fw39-qa-02-guided`: a complete run. `wellfed-audit qa` graded it `qa_fail` (report `fw39-qa-02-guided-audit-2.json`).
    - Startup, optics, power and all four buttons passed. Each prompted button arrived on its FW20 pin.
    - The four indicators and the tone were all answered "no". The owner says these were mis-pressed: PA4, PB22 and the tone were answered within a second, the tone 0.2 s after it started.
    - So indicators and speaker are **not established** by QA yet.
- **Findings:**
  - Optics passes on a cold boot. On the OTA trial boot it faulted at once (`PLOPT1 error kind=supervisor n=1 fault=7`, as on every reference boot since `itest-20261022-10`), so the fault follows the boot path rather than the probe.
  - The UART frames lines as `\n\r`. The first `wellfed-audit qa` refused every record for that reason; it is fixed and tested.
  - The first button cue is lost when the startup chime is still waiting for the settings.
  - The optics run takes about 110 s on a cold boot, leaving about 70 s of a 180 s capture for the guided checks.
- **Next:**
  - Repeat the guided run with deliberate answers.
  - Make QA ignore answers within about 0.5 s of a prompt or before the tone ends.
  - Queue the first cue after the chime.
- **Flash now:** **Bank 1:** QA `201e76ff…` (OTA v68, confirmed). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 69. Restore the reference build with `--review itest-20261022-19-good-review` if needed.
- **Bench state:** restored. Feeder off (both relays off), hotspot down, no server.

**October 23, about 3 AM: Rust stock recovery on hardware (FW38 done, physical, owner present).**
- **Code first:** `wellfed-flash` again reads reviews frozen before FW37 (`profile: plaf103-firstboot`, no `device`). Bank 0's image and the reference build have such reviews, so the give-way and the reinstall needed this. It also clears the emulated-fixture failures noted below.
- **Runs** (release binaries rebuilt first):
  - `give-way-prepare/-execute fw38-stock-01-giveway`, with `--fallback` naming the `itest-20261022-18-good-review` review: one 1 KiB frame at `0x186000`, no erase. Bank 0 `f244d9a9…` booted (`bank=0`).
  - `loader fw38-stock-02-loader`, then `wellfed-flash read small`/`read full` (`private/flash-backups/fw38-stock-02-*`), then `stock-recovery prepare` (171 sectors, plan `f3879bca…`) and `execute` (`private/verification/fw38-stock-02-recovery-execute`). Every sector was read back, and the pre-marker 8 MiB verification matched. The marker was committed and its sector read back.
  - The final full read was skipped at the owner's request: SIGINT once it began, so the journal ends `stopped`, `mutation_possible`.
  - `boot fw38-stock-03-boot`: stock 1.1.15 from bank 1, no feeds scheduled. The stock log prints the unit's serial and cloud key and secret in plain text; keep that capture private.
  - `flash-prepare/-execute fw38-stock-04-restore --bank 1`, from the `fw01-stock-review` source to the `itest-20261022-19-good-review` target. The inspection matched bank 1 byte for byte against the stock review. 170 sectors written. `f550a0e9…` booted on bank 1, confirmed.
- **Flash now:** **Bank 1:** reference `f550a0e9…` (installed over UART; the last OTA version is still 67). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 68.
- **Bench state:** restored. Feeder off (both relays off), hotspot down, no server.

**October 23: Python tools retired (FW38, offline, nothing run on the bench; built in a worktree, then merged to master).**
- **What changed:**
  - No Python is left ([tools](src/tools/README.md)). `make check` and `make check-fixtures` take no `PYTHON=`, and `cargo xtask check-repository` refuses `.py` under `src/`.
  - New `wellfed-emu` runs the factory loader (mapping, bank selection) and the pinned UART helper under Unicorn (the `unicorn-engine` crate, GPL-2.0; builds need CMake, and Linux links libatomic).
  - `wellfed-build review stock|build` replaces `review-development-image.py`. `wellfed-bench ota --build` runs it, and its `--python` option is gone. `review.json` is typed in `wellfed-formats` and read by `wellfed-flash`; Python-written reviews still read.
  - Dropped as finished research: the write-back acceptance tests, the ROM route, probe-slot, reset-decoder and SDK flash-user reports. The `emulate-ota-markers.py` states are now `wellfed-emu` fixtures.
- **Evidence (emulation and static, not physical):**
  - Python and Rust reviews of the same PLAF103 first-boot build were identical apart from `reviewer_sha256`, including every factory-loader run (`private/verification/python-to-rust-compare/`).
  - The factory and OTA-marker fixtures pass on the baseline.
  - `wellfed-flash`'s emulated CLI sessions pass (`tests/emulated.rs`): update, give-way/restore, status, stock recovery (full read, plan, execute; bank 1 restored, nothing else changed), and a mid-recovery fault that stops without retry.
- **Not yet done (done in the 3 AM session above):** a hardware stock recovery through `wellfed-flash`.
- **Identity:** `wellfed-build`'s sources changed, so build IDs change and cached reviews are rebuilt once. Rebuild the release binaries before bench use.
- **Checks:** `make check` (its three private credential files linked in temporarily for `check-cross`) and `make check-web` pass on the rebased branch. Against the main checkout's `private/` (`WELLFED_FIXTURE_ROOT`): the factory and OTA-marker fixtures and the emulated status, stock-recovery and fault sessions pass. The Python-vs-Rust review comparison and the emulated update and give-way sessions passed before the rebase onto FW37. After it, the update and give-way sessions are refused because `private/fixtures/development-{source,target}` were frozen before FW37 (`profile: plaf103-firstboot`, no `device`), which FW37's reader rejects. The review fixture is refused because the main checkout's first-boot build is stale. Refreeze those reviews from a current build (or decide whether `wellfed-flash` should keep reading pre-FW37 reviews; bank 0's installed image has one), then run `make check-fixtures`.
- **Bench state:** untouched by this work.

**October 23, about 1 AM: buttons mapped (FW20, physical, owner pressing).** Built and run from a worktree, then merged to master.
- **What changed:** variant `fw20-buttons` (`wellfed-build build --profile firstboot --variant fw20-buttons`): probe `probes/buttons.rs` in place of the optics probe; it configures PB4..PB7 as stock does and logs `PLBTN1` records (startup-diagnostics). Host tests in `tests/buttons_probe.rs`. Build: every audit passed (`wellfed-btn` 796 of 4096 B). Review `fw20-buttons-01-review` (`2943029a…`), in `private/verification`.
- **Runs:** `itest-20261023-03 --only ota-update --power` installed it (v66, confirmed); `fw20-buttons-01-boot`; presses in `fw20-buttons-02-press` (free play) and `-03-press` (one at a time, left to right).
- **Results:** Reset = PB4, Mic = PB5, Dispense 1 portion = PB6, Lock = PB7; active LOW; the lines idle high with the pad pulls off (board pull-ups); every edge single, no bounce at 2 ms.
- **Restore:** `itest-20261023-04 --review itest-20261022-19-good-review --only ota-update --power` put the reference build back (v67, confirmed).
- **Flash now:** **Bank 1:** reference `f550a0e9…` (OTA v67, confirmed). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 68.
- **Bench state:** restored. Batteries out, feeder off, hotspot down, no server.

**October 23: config-driven builds (FW37, host only, nothing run on the bench).**
- **What changed:** `wellfed-build build --profile PROFILE [--device] [--firmware] [--variant]`. Devices are `src/firmware/devices/<name>/device.json` (the PLAF103 layout moved to `src/firmware/devices/plaf103/layout.json`). Firmwares declare devices, policies and variants in `[package.metadata.wellfed]`; `wellfed-build list` prints them. The AmebaD code is under the builder's `src/platform/amebad/`.
- **Renamed:** profiles `plaf103` and `plaf103-firstboot` are `device` and `firstboot`; outputs are `private/firmware-build/<device>/<profile>`; make targets `device` and `firstboot` (`DEVICE=`, `FIRMWARE=`, `VARIANT=`). The variant flags are `--variant <name>`. The manifest and identity bind `device`, so every identity changed: rebuild the release binaries, and `--build` makes a new review rather than reusing one.
- **FW14:** `fw14-adc` is now a declared variant of the reference firmware: `--variant fw14-adc` replaces `--adc-probe`.
- **Not yet:** a bench install of a build made through the new paths (FW37's finish line).

**October 23, after midnight: battery inputs measured (FW14, physical, owner at the bench).**
- **What changed:** `wellfed-build plaf103-firstboot --adc-probe` builds the `fw14-adc` variant (policy `fw14-adc-v2`): the reference firmware runs the read-only ADC probe in place of the optics probe, sampling once a second for as long as it runs. The probe never binds the KM0 flash handshake, so it reports `startup_done` at once and the build confirms. `wellfed-bench ota/itest --variant fw14-adc` builds it too. Checks: `make check`; a fresh `--adc-probe` build with every audit passing (`wellfed-adc` stack 1120 of 8192 B); review `fw14-batt-01-review` (`7c085e27…`).
- **Runs:**
  - `itest-20261023-01 --review fw14-batt-01-review --only ota-update --power` installed it (v64, confirmed in 2 min 43 s).
  - `fw14-batt-01-baseline` (cold boot, no batteries) passed `wellfed-audit adc`.
  - The owner then inserted three D cells with the feeder running; later unplugged and replugged the adapter by hand (the bench `off` needs the serial port, so it cannot run during a capture). Receive-only captures `fw14-batt-02-both`, `-03-cut`, `-04-unplug`, `-05-meter`.
- **Results** ([notes](docs/firmware/startup-diagnostics.md)): channel 5 is the pack, through a divider of about 2.06 (2357 mV at 4.85 V with the adapter, about 2306 mV at 4.77 V on batteries). PA19 is 1 with the adapter, 0 without. No reset at either transition. Channel 5 is noisy (about 110 raw spread per sample).
- **Restore:** the owner removed the batteries; `itest-20261023-02 --review itest-20261022-19-good-review --only ota-update --power` put the reference build back (v65, confirmed).
- **Checks after the session:** fresh `plaf103` and `plaf103-firstboot` (reference) builds and `make check-fixtures` with no skips.
- **Flash now:** **Bank 1:** reference build `f550a0e9…` (OTA v65, confirmed). **Bank 0:** unchanged (`f244d9a9…`). **Next manifest version:** 66.
- **Bench state:** restored. Batteries out, feeder off (relay 1 off), hotspot down, no server.

**October 23, early morning: one SDK, many firmwares on hardware (FW36 done, physical over the air, autonomous).**
- **Runs** (release binaries rebuilt first):
  - `itest-20261022-19 --build --tier smoke --power` passed (`ota-update` 4 min 16 s, `requests`, `request-auth`). The SDK-split reference build `f550a0e9…` (v61) confirmed on its trial boot; the UART shows the new order `PLMQT1`, `PLOPT1`, `PLAUD1`, then `PLCNF1 start`.
  - `itest-20261022-20 --review` (review `fw36-blink-01-review`) `--only ota-update --power` installed `blink` (`2ea8035d…`, v62). It confirmed (`healthy_ms=30000`), and health came from bank 1. The UART shows `PLBLK1 start` and four output-only levels on PA0, then the settings writer and the offer check. The owner watched the connection indicator blink.
  - `itest-20261022-21 --review` (review `itest-20261022-19-good-review`) `--only ota-update --power` put the reference build back (v63, trial boot, confirmed). A firmware cannot lock the device in.
  - `PLOPT1 error kind=supervisor n=1 fault=7` on the reference boots is not new: every capture since `itest-20261022-10` has it.
- **Flash now:**
  - **Bank 1:** reference build `f550a0e9…` (OTA v63, confirmed).
  - **Bank 0:** unchanged (`f244d9a9…`, recovery role).
  - **Next manifest version:** 64.
  - **Settings:** generation 3025 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials.
- **Bench state:** restored. Feeder off (both relays off), hotspot down, no server.

**October 22, late night: one SDK, many firmwares (FW36, host only, nothing run on the bench).**
- **What changed:**
  - The workspace is `src/sdk/` (the SDK), `src/tools/` (host tools) and `src/firmware/wellfed/` (one crate per firmware). Renamed: `wellfed-amebad` to `wellfed-hal`, `-amebad-common` to `-hal-common`, `-amebad-sdk` to `-amebad-sys`, `-ffi` to `-sdk-c`, `-firstboot` to `wellfed-fw-reference` in `src/firmware/wellfed/reference`. C symbols are unchanged.
  - New `wellfed-sdk`: the KM4 runtime from `app.rs` (entry, startup, Wi-Fi and transport, feeder, confirmation, settings writer, updater, offer check). Bank 0 never starts firmware code. A firmware defines `firmware!(start)` and creates tasks with `wellfed_sdk::spawn`; it reports `startup_done` before the runtime confirms.
  - The reference firmware's tasks now start before the confirmation task, so `PLAUD1 start` logs before `PLCNF1 start`.
  - The stack audit reads its task table from the runtime and the firmware.
- **Checks:** `make check`; fresh `plaf103` and `plaf103-firstboot` builds with every audit passing; `make check-fixtures` with no skips. Stack bounds unchanged except audio 1040 to 728 and optics 4352 to 4380 (of 8192), both from cross-crate inlining.
- **Not yet physical:** the split images have not run on the feeder.
- **Firmware selection (later the same night):**
  - `wellfed-build --firmware <name>` (default `reference`; `make -C src/firmware/vendor … FIRMWARE=<name>`). The KM4 archive is the firmware crate; KM0's is `wellfed-sdk-c`, which every firmware also links through `wellfed-sdk` for the core C ABI and the panic handler.
  - The firmware name joins the identity and the manifest (`firmware`). Only the chosen firmware's sources are inputs. `audit` and the Python review read it from the manifest, and image variants are refused for any firmware but `reference`.
  - New `src/firmware/wellfed/blink`: one task blinks the connection indicator, verifying each level. A `play` request on blink fills the audio mailbox and later ones answer `busy`.
  - Checks: `make check`; `plaf103` and `plaf103-firstboot` for `reference` (stack bounds as above), and `plaf103-firstboot` for `blink` (blink task 748 of 4096); `make check-fixtures` with no skips against each. `private/firmware-build/plaf103-firstboot` holds the reference build again.
- **Layout (later the same night):** everything first-party is under `src/`: `src/sdk/` (SDK crates), `src/tools/` (Rust host tools beside the Python tools), `src/firmware/wellfed/` (firmwares), `src/firmware/vendor/` (the former `firmware/`: C ports, pins, layout, image build, so `make -C src/firmware/vendor plaf103-firstboot`), `src/web/` and `src/scripts/dev-dashboard`. The Python tools' bytes changed (their `ROOT` depth), so plans that recorded their hashes are stale. Rebuild the release binaries (`cargo build --release`) before bench use; the old ones read the old paths. Checks: `make check`, `make check-web`, fresh `plaf103` and `plaf103-firstboot` builds (stack bounds unchanged), and `make check-fixtures` with no skips.

**October 22, late night: faster integration runs (physical over the air and by UART, autonomous).**
- **What changed:**
  - **Pipelined OTA download:**
    - The recovery role asks for all eight chunks of the sector it is staging at once, each with its own retries.
    - It polls again after 1 ms while answers arrive, instead of after 50 ms.
    - The writer (`wellfed-core::ota::Writer`) accepts a sector's chunks in any order and hashes the sector in order once complete.
    - Sector order, the marker checks, readback, the whole-image hash and the marker-last commit are unchanged, and so is the wire format.
  - **One scenario instead of two:** `trial-fallback` and `ota-faults` are now `ota-recovery`. The fault server takes over while the `fw11-fail` variant fails its trial, so bank 0's recovery role meets it directly, and the clean download is the restore.
- **Checks:** `make check`; fresh `plaf103` and `plaf103-firstboot` builds with every audit passing (updater stack bound 9960 of 24576 B); `make check-fixtures` with no skips.
- **Physical:**
  - **Bank 0 reflashed over UART** (`fw10-pipeline-bank0-20261022-01..03`).
    - Sessions 01 and 02 each stopped on a loader NAK (`0x15`) straight after an erase command: 01 at 0x66000, 02 at 0x68000 after one more sector.
    - Session 03 (`--interrupted`, `--off-seconds 5`) staged the remaining 72 sectors, verified them, committed the marker and booted bank 1 (`69a0720a…`) cleanly.
    - The NAK pattern is new evidence: nothing was retried, and the cause is not established.
  - **`itest-20261022-18 --build --power`** passed all seven standard scenarios in **13.6 min** (`-14`: 27.8 min).
    - The download takes **18–19 s** for 680 KB (about 37 KB/s; before: ~106 s at 6.5 KB/s).
    - `ota-update` took 164 s (389 s before).
    - `ota-recovery` took 336 s, against 961 s for the two scenarios it replaces.
- **Flash now:**
  - **Bank 1:** normal build `f244d9a9e1c0…` (OTA v60, confirmed).
  - **Bank 0:** the same sources, frozen as `fw10-pipeline-review-20261022-01` (OTA SHA-256 `98225789…`).
  - **Next manifest version:** 61.
  - **Settings:** generation 3023 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials (generation 15).
- **Bench state:** restored. Feeder off, hotspot down, no server.
- **Not run:** `ota-power-cuts` (needs `--destructive`) with the new updater.

**October 22, late night: the Rust conventions are enforced by lints (physical over the air, autonomous).**
- **What changed:**
  - Every crate inherits `[workspace.lints]` (`unsafe_op_in_unsafe_fn`, `dbg_macro`, `undocumented_unsafe_blocks`). Host crates forbid `unsafe` at their crate roots.
  - `make check-rust` adds a Clippy pass over libraries and binaries with `PRODUCTION_LINTS`: `unwrap_used`, `cast_possible_truncation`, `needless_pass_by_value` and `unreachable_pub`. Tests are exempt.
  - The 60 sites it found are fixed, with no change to record bytes:
    - typed record constants (`RECORD_LEN`, `COMMIT_OFFSET`, `TAIL_CHUNK`);
    - `c_enum::low_byte()`;
    - `ParkToken::new` replacing three lo/hi splits;
    - `try_from` for bounded values;
    - an `#[expect]` with its reason for deliberate truncation.
  - **One visible change:** health and settings generations now saturate at `u32::MAX` instead of wrapping.
- **Checks:** `make check`; fresh `plaf103` and `plaf103-firstboot` builds with link, RAM-park, hook and stack audits passing on unchanged pins; `make check-fixtures` with no skips.
- **Physical:**
  - `itest-20261022-17 --build --power` passed all eight standard scenarios on the firmware lint changes (OTA v53–v57).
  - Runs `-15` and `-16` failed at the build step before anything reached the feeder. The cause was the harness setup: `wellfed-build` compiles in its repository root (`CARGO_MANIFEST_DIR`), so a worktree's builder builds from that worktree. Run `itest` from the main checkout's binaries, or pass `--build-bin`.
  - The bench's own lint fixes (a borrowed request and two duration casts) landed after `-17`. They are host-checked only.
- **Flash now:**
  - **Bank 1:** normal build `69a0720a8884…` (OTA v57, confirmed).
  - **Bank 0:** `36c9a086…`.
  - **Next manifest version:** 58.
  - **Settings:** generation 3019 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials (generation 13).
- **Bench state:** restored. Feeder off, hotspot down, no server.

**October 22, night: bench console (FW35, host only, nothing run on the bench).**
- **What changed:**
  - `wellfed-bench --ui` is now a Svelte app in `src/web/` (pnpm workspace), embedded only with `--features ui`. Without the feature, `--ui` refuses. `make ui` builds it.
  - New read-only views: `/api/tests` (catalog, runs, per-scenario stats), `/api/tests/{name}` and `/api/feeder` (`wellfed-local` `GET /api/status` via `private/server.json`, plus an SSE `feeder` event).
  - The itest record is a typed `RunRecord`, written and read through one type. The HTTP client is shared in `local_api.rs`, and steps carry start and finish times.
  - The old `assets/` page is deleted.
- **Checks:** `make check` and `make check-web` (Vitest, `svelte-check`, Playwright against the binary on a synthetic tree).
- **Next:** with the owner, watch the Feeder and Tests views through a live `itest` run to finish FW35.

**October 22, evening: idiomatic-Rust pass (physical over the air, autonomous).**
- **What changed:** host tools read and write typed serde structs instead of `serde_json::Value`. Shared file formats live in the new `src/tools/wellfed-formats` (build manifest and identity, layout, locks, serial events, credentials, flash plans). Firmware got only codegen-neutral cleanups: `unsafe fn` contracts, `// SAFETY:` comments with `clippy::undocumented_unsafe_blocks` denied, one `c_enum!`, `TaskCell`, shared constants, and stage enums with identical record text. AGENTS.md now carries the Rust conventions and the owner's compatibility policy (old evidence may break; live interop and pinned checks may not).
- **Bugs fixed (static):**
  - `wellfed-audit startup` rebuilt the identity without the variant or update key, so it refused every current build.
  - The bench's attended-motion gate passed an unparseable `build-manifest.json`.
- **Checks:** `make check`; fresh `plaf103` and `plaf103-firstboot` builds, whose link, RAM-park, hook and stack audits pass with unchanged pins; `make check-fixtures` with no skips.
- **Physical:**
  - `itest-20261022-13 --build --tier smoke --power` passed after the formats crate.
  - `itest-20261022-14 --build --power` passed all eight standard scenarios on the fully merged tree.
  - Between the two runs, the owner had the speaker variant installed (`speaker-20261022-02`, `8356f235…`, OTA v47) and listened to `wellfed-local speak`.
- **Flash now:**
  - **Bank 1:** normal build `196652d97ee9…` (OTA v52, confirmed).
  - **Bank 0:** `36c9a086…`.
  - **Next manifest version:** 53.
  - **Settings:** generation 3015 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials (generation 11).
- **Bench state:** restored. Feeder off, hotspot down, no server.
- **Open follow-ups:**
  - `wellfed_formats::manifest::Audits` is still `Value`; the audit types live in wellfed-build.
  - The bench's `LocalStatus` mirrors wellfed-local's status document instead of sharing a type.
  - xtask's FW08 transport-isolation list blocks core `transport.rs` from the shared hex decoder and `named_enum!`.
  - Deferred firmware items: the `PROVIDER` aliasing, merging the three record stores' framing, and `net_store::save`'s missing verify-before-commit.

**October 22, late night: the firmware build is Rust (physical, autonomous).**
- **`wellfed-build`** (`src/tools/wellfed-build`) replaces `firmware/tools/build.py` and its six audit modules (pad table, RAM park, SDK hooks, first-boot mains, flash/eFuse writers, task stacks), plus the OTA layout audit it ran as a child. `make -C src/firmware/vendor <profile>` and `check-cross` call it; run the executable itself, since `cargo run` sets `RUSTUP_TOOLCHAIN`, which a build refuses.
- **Equivalence shown before deleting the Python:**
  - Static: on the same Python-built speaker tree, all seven Rust audits equal the manifest's.
  - Static: identities equal Python's `source_identity` for the same inputs (bare, image, speaker, fw11-fail).
  - A Rust build of the same variant matches the Python manifest except for content hashes, which follow from the new identity. Overlays (`sdk_overrides`), policies, sizes, symbols and verdicts are identical, and the generated credential headers are byte-identical apart from the banner.
- **Identity inputs changed:** the builder's `src/` replaces the Python tools, so every build ID is new and cached reviews are rebuilt once.
- **Review stays Python** (unicorn factory-mapping emulation); it now asks `wellfed-build inputs` and `wellfed-build audit` for the inputs and the linked-code audits.
- **Real-build fixture tests:** `cargo test -p wellfed-build -- --ignored` (in `make check-fixtures`, never `make check`).
- **Physical:** `itest-20261022-12 --build --tier smoke --power` passed: ota-update in 4 min 45 s, requests, request-auth.
- **Flash now:**
  - **Bank 1:** Rust-built normal image `0f8b3ad851fc…` (OTA v45, confirmed).
  - **Bank 0:** `36c9a086…`.
  - **Next manifest version:** 46.
  - **Settings:** restored by the suite.
- **Bench state:** restored. Feeder off, hotspot down, no server.
- **Next:** port the image review (needs a Rust answer for the unicorn factory-mapping emulation), a run-history page, and more scenarios.

**October 22, night: integration suite scaled (physical, autonomous).**
- **Faster runs:**
  - `--tier smoke|standard|soak`.
  - `--build` reuses a review of the same sources: `wellfed-build source-id --image`, with `--rebuild` to force a build.
  - The UART capture chains on its own thread.
  - After every scenario the suite checks settings, stored Wi-Fi, build and bank, and puts back what it can.
- **New scenarios:** `soak` (soak tier), and `ota-power-cuts` (needs `--destructive`).
- **Shared record parser:** `wellfed_core::record`, used by the bench. `wellfed-hal/tests/records.rs` runs the firmware's `describe` output through it.
- **Firmware finding (physical):** a reinstalled build that is already confirmed skips the trial (confirmation is keyed by build identity), so trial tests install the speaker variant.
- **Runs:**
  - `itest-20261022-09` passed eight scenarios; the power-cut design failed as above.
  - `itest-20261022-10` passed `ota-power-cuts`.
- **Flash now:**
  - **Bank 1:** the normal build `4db3e012…`, re-signed as `itest-20261022-10-cut-restore` (OTA v44), confirmed.
  - **Bank 0:** unchanged (`36c9a086…`).
  - **NOR settings:** generation 3007 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials (generation 9).
  - **Next manifest version:** 45.
- **Bench state:** restored. The hotspot is down, no server is running and the feeder is off.
- **Bench page:** stopped when its 8-hour bound ran out. Restarting it was refused by Claude Code's auto mode, so the owner restarts it. It needs a restart to show the fixed scenario steps.
- **Next (superseded above):** port `build.py` and the review tool to Rust, a run-history page, and more scenarios: transport faults through a proxy, settings power cuts, boot loops.

**October 22, late evening: FW27 integration suite (physical over the air, autonomous).**
- **Built:** `wellfed-bench itest NAME` runs eight real-device scenarios. It installs over the air and asserts on receive-only UART, and it owns the hotspot, `serve` and, with `--power`, the supply relay, restoring all three afterwards ([bench](docs/devices/plaf103/bench.md#integration-suite)). Merged from the `bench-itest` worktree.
- **Result:** `itest-20261022-06 --build --power` passed all eight in 26 minutes. Runs 01–05 found and fixed harness bugs, plus one firmware build bug: the `fw11-fail` variant's constant failure let LTO drop the image's flash writers, so it failed the pinned link audit. The forced failure is now opaque (`black_box`), and the audit pins are unchanged.
- **Bench before the run:** the speaker session's `serve` (2-hour timeout) was still running, and I stopped it with SIGTERM. That session then put its own state back: hotspot down, feeder off.
- **Flash now:**
  - **Bank 1:** the normal build `877b2c9b…`, re-signed as `itest-20261022-06-fault` (OTA v32), confirmed.
  - **Bank 0:** unchanged (`36c9a086…`).
  - **NOR settings:** generation 3001 (lights 1, sound 1, volume 40).
  - **Wi-Fi:** no stored credentials (generation 7).
  - **Next manifest version:** 33.
- **Bench state:** restored. The hotspot is down, no server is running and the feeder is off.

**October 22, evening: FW24 speaker (physical over the air, owner listening).**
- **Finding (static, stock):** the "buzzer" is a real speaker. Stock decodes AAC into the codec's line output by SPORT/GDMA and holds PB23 HIGH while it plays.
- **Built:** a codec shim (16 kHz mono, 8-page DMA ring) and Rust tunes (`wellfed-core/src/tone.rs`). There is a speaker task (`wellfed-hal/src/audio.rs`) with a startup chime, and a signed `play` request. Design and evidence: [audio](docs/firmware/audio.md).
- **Heard on the unit:** the chime, `coin`, `twinkle` and `meal`. A second request during playback gets `busy`, and `sound` off gives `muted`. Volume is now on a decibel scale, after the linear one made 40 and 80 sound alike.
- **Speaker variant (the owner's first "alternate build"):** `wellfed-build plaf103-firstboot --speaker` adds a signed TCP audio stream (port 7700). `wellfed-local speak FILE` decodes any format with ffmpeg and plays it ([audio](docs/firmware/audio.md#speaker-variant-network-audio)).
- **Stock clips:** the 11 voice clips from the stock FAT (`0x4B0000`) are extracted for the owner only, in ignored `private/stock-audio-clips/`.
- **Flash now:**
  - **Bank 1:** `fw24-speaker-ota-20261022-01` (`c286fbac…`, OTA v22, speaker variant, confirmed). Install a normal build to leave it.
  - **Bank 0:** unchanged (`36c9a086…`).
  - **NOR settings:** generation 2985 (lights 1, sound 1, volume 40).
  - **Next manifest version:** 23.
- **Paused:** the watchdog clock spread. Both the FW05 and FW06 builds run the SDK's KM0 app start, which calibrates OSC131K on every cold boot. The SDK documents the watchdog clock as SDM 32k, so the calibration difference does not explain the spread.

**October 22, afternoon: post-FW10 re-verification and FW11/FW12/FW21 bring-up (autonomous, physical through the relay bench and over the air).**
- **Re-verified after the crate split and the Python→Rust ports.**
  - `make check` and `make check-fixtures` (100 tests, no skips) both pass.
  - FW33 passed on hardware.
  - A rebuilt image went into bank 0 over UART: 158 sectors, no NAK.
  - Then OTA. Regressions found and fixed:
    - **Review tool:** `review-development-image.py` shadowed its own `payload` function.
    - **Optics stack overflow:** LTO inlined a 3 KiB probe temporary into the optics task, overflowing its 4 KiB stack. The first trial overflowed and KM0 gave way. The next build (out-of-line init, still 4 KiB) overflowed during init, before the watchdog opened, and hung; that needed a UART give-way. Now: out-of-line slot init, an 8 KiB stack, and a build-time per-task stack audit (now `wellfed-build`'s `audit/stack.rs`).
    - **Build and fixture tools:** `plaf103` and `bringup` hadn't built since FW07 (credential headers). The `fw11-reset` variant no longer passed the link audit. The link audit flagged an instruction whose bytes equal an address (now only literal and data words count). Two fixture tests were stale.
- **FW11:** the feeder task now opens the watchdog before the startup diagnostic, with a 10-minute trial deadline (`amebad/src/supervise.rs`). Over the air: failed start (`fw11-fail`), hang after start (`fw11-reset`) and never-confirms (server stopped) each counted 1-2-3 and gave way to bank 0.
- **FW11 and FW12 are Done.** FW11 adds a lost confirmation (a power cut mid-trial). FW12: `wellfed-bench ota NAME --build` runs build, review, sign, offer, watch and health in 6 minutes.
- **FW25 (in progress):** signed `settings` requests read and change lights, sound and volume in the FW15 store; persistence across a cold boot is proven. **Wi-Fi credentials without a rebuild** are proven on hardware too:
  - stored credentials joined after a cold boot;
  - a wrong passphrase fell back to the build's after 5 failed joins;
  - a signed clear recovered;
  - the passphrase never appeared in any capture.
- **LED startup probe removed (owner decision):** startup now only drives the indicators LOW (`low-only-v1`, routing `read-only-v3`), saving about 15 s per boot (activation to confirmation went from 74 to 57 s).
- **Open:** `wellfed-audit startup` refuses OTA-keyed builds ("Build identity is not source-bound"), seemingly because the identity also covers the OTA public key. This predates the LED change.
- **FW21:** server-set wall clock over the FW08 channel ([clock](docs/firmware/clock.md)).
- **Bench page:** reworked; token removed; served on the Tailscale address on request.
- **Authenticated requests:** server→firmware requests are HMAC-signed with a per-session nonce and sequence ([design](docs/firmware/local-transport.md#authenticated-requests)). Over a bare broker on hardware, an unsigned `time` was refused and a signed one accepted. The key was added with `wellfed-local firmware-request-key`. Backups of both private files from before are `private/server.json.before-request-key` and `private/fw-mqtt.json.before-request-key`.
- **Server fix:** `serve` now handles SIGTERM. Before, `kill` or `timeout` left its broker child orphaned (observed).
- **Flash now:**
  - **Bank 1:** `fw25-noled-ota-20261022-01-review` (`cd26923f…`, OTA v19, confirmed). It adds Wi-Fi credentials and the low-only LEDs. Stored credentials are cleared (generation 3).
  - **Bank 0:** `final-review-20261022-01` (`36c9a086…`), installed by UART. It lacks the Wi-Fi credential store and the low-only LEDs, but its recovery role is fine.
  - **What the build carries:** FW11 supervision, the FW21 clock (2000 ppm bound), signed requests and the FW25 settings writer.
  - **NOR settings:** generation 2979 (lights 1, sound 1, volume 40).
  - **Next manifest version:** 20 (`wellfed-bench ota` picks it).
- **Bench state at this checkpoint:** restored to before the session.
  - The hotspot is down, `serve` is stopped and the feeder is off.
  - The read-only bench page still runs on the Tailscale address at the owner's request (bounded to 8 h).
- **FW27:** DHCP renewal was observed about 30 minutes into a lease (host DHCP log), with no loss.
- **Next (autonomous):** FW25 broker credentials without a rebuild, an owner settings reset, and refreshing bank 0 to the current build over UART.
- **Needs the owner:** FW14 (batteries), FW30 (meter or analyzer, PB3 test points), FW16 and later (motion), FW23 and FW24 (eyes and ears for LEDs and audio).

**October 22, crate split (offline, nothing flashed; merged from the `crate-split` worktree).** `wellfed-hal` is split three ways. `wellfed-hal-common` holds the flash park, hooks, peer and trial, and is all KM0 links. `wellfed-hal` holds the KM4 ports, and the lease `Provider` moved from `probes/flash.rs` to `provider.rs`. `wellfed-fw-reference` holds the app entry, the probes and the `fw16-pulse`/`fw11-reset` features. Static comparison of the firmware archives:
- **C symbols:** the exported set is identical for km0, km4 and both variants.
- **RAM-park code:** it disassembles identically on both cores.
- **Sizes:** only `wellfed_app_start` changed (0x178 to 0x1a6). It calls the same functions; the difference is LTO register allocation.
- **Rebuild needed:** the firmware source ID changed, so every ignored build is stale. Rebuild and review before any install.

**October 22, Python to Rust (offline, nothing run on the bench).** The owner's direction is Rust wherever it makes sense; once a Rust version is shown equivalent, the Python tool and its tests go ([src/tools/README.md](src/tools/README.md) lists what went and what stays).
- **Ported, with equivalence shown before deletion:**
  - `wellfed-audit` replaces the startup, FW14 ADC, FW30 transient and full-flash auditors. Every call the old tests made, plus 227 real captures and 2 real read pairs, gave identical reports.
  - `wellfed-sim`'s `virtual-feeder` replaces the Python harness: identical reports on all 8 scenarios.
  - `cargo xtask check-repository` replaces `check-repository.py`: identical on the tree and on injected faults.
  - `wellfed-flash` is now the only development updater, after its FW32 parity.
- **Removed as finished:** the FW05–FW15 auditors, the stock C-probe research (with the `wellfed-probe` crate) and the standalone diagnostic.
- **Kept in Python on purpose:** stock recovery, the unicorn emulations, the image review and the build helpers.
- **Rebuild before relying on the private builds:** the firmware build inputs changed (`Cargo.lock`, the probe crate removed). The fixture suite skips 14 stale-build checks until the builds are refreshed; frozen reviews stay valid.

**October 22, latest (offline, nothing run on the bench).** FW33: the relay bench is now the Rust `wellfed-bench` (crate `src/tools/wellfed-bench`). It chains `wellfed-flash` as a child process and serves the read-only page with `--ui`. `bench.py`, `bench-dashboard.py`, `relay-demo.py`, `uart-give-way.py` and seven finished one-off Python analyses are removed (list in [src/tools/README.md](src/tools/README.md)). It is host-tested only. **Next:** with the owner, build both binaries (`cargo build --locked --release -p wellfed-bench -p wellfed-flash`), then run `status`, `on`/`off`, `loader` and a no-op `flash-prepare`/`flash-execute` with `--ui` ([bench](docs/devices/plaf103/bench.md)).

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
`src/tools/bench/bench-dashboard.py` shows bench progress on localhost.

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

Per the latest checkpoint at the top of this file (reconfirm before relying on it):

- **Bank 1 (boots):** the `feeder` firmware `e1215de0…`, OTA v107, confirmed (`fw45-pair-01`), with the FW45 generator, identity, key mailbox and pairing gestures, and the FP15 key reset on an unlocked Reset hold. It carries the runtime (supervision, clock, signed requests, settings writer, offer check), the button service and the FP11 child lock, and the FW44 Noise session table. It drives no motor.
- **Bank 0 (recovery):** `qa` `ef76b85e…`, written by stock 1.1.15's updater in `fw40-stock-ota-07`. Only its runtime's recovery role runs from bank 0.
- **Next manifest version:** 107.

Wi-Fi and broker credentials come from private files; images stay under `private/`.

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
- `wellfed-bench` drives the relays and chains `wellfed-flash` ([bench control](docs/devices/plaf103/bench.md));
  ports are `/dev/serial/by-id/` paths in ignored `private/bench.json`. The relay
  board can take `/dev/ttyUSB0`.
- Physical, October 21, 2026: relay-1 boots reached the Rust bring-up banner and
  relay-2-held boots printed `Flash Download Start` and NAKs, 16 of 16 each, off
  times 0.3-2 s, with or without UART break held during the off time. The earlier
  stuck-entry symptom (fixed by replugging the adapter) did not reproduce.
- Adapter power/VCC and flow-control pins remain unwired. Until October 22 no batteries were in use; the owner now has batteries, not yet installed as of this note. With batteries in, relay 1 off no longer means feeder off.
- CON3 remains connected: this motor harness also carries board power.
  The front indicator PCB is difficult to access; no disassembly or wire cutting
  was needed for the successful LED test.

Consult the [UART procedure](docs/devices/plaf103/uart.md) before changing wiring.
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
- A structural no-dispense check in `cargo xtask check-repository` (formerly `src/tools/check-repository.py`).
- `audit-firmware-transport.py` (removed after FW08 closed; its reports remain).

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
