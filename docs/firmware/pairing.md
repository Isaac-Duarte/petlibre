# Keys, randomness and pairing

This page explains how the feeder decides who may talk to it: its random generator, its identity key, the clients it trusts and the ways a new client gets in. It is for owners who want to know why pairing works the way it does, and contributors working on it. Bluetooth setup has its own page, [Bluetooth](bluetooth.md).

## The one idea

Every client has its own key, and the feeder keeps a short list of the keys it trusts. Getting onto that list always takes someone at the feeder, or a client already on it. The first owner keys a code into the buttons; later clients need an invite from a paired client, or a button hold. Nothing over the network alone can make a stranger an owner.

All of it rests on unpredictable keys, and the chip offers no vetted random generator. So the firmware builds its own from measured noise.

## No hardware RNG

What the pinned SDK offers, from static analysis:

- The AmebaD SDK exposes no hardware random generator.
- `rtw_get_random_bytes` calls `arc4random`, which on the 8721D is ROM `Rand()`: a Tausworthe generator whose four words start at the constant 12345 (`swlib/string/rand.c`).
- The firmware's first generator mixed that output with SysTick and the tick count, which rests on timer values alone. That is fine for a value that only has to differ between sessions, but a long-term key needs real entropy, so it was replaced.

**What replaces it** (`wellfed-core::rng`):

1. **Raw noise** (`wellfed-hal::entropy`): the low byte of a raw ADC conversion of the chip's internal supply, and SysTick's cycle count between 32 kHz timer edges. Both were measured on the feeder ([below](#measured-noise)).
2. **Health tests** (SP 800-90B 4.4):
   - a Repetition Count Test and an Adaptive Proportion Test (window 512) on every sample;
   - cutoffs from the min-entropy the source claims per sample, at a false-alarm rate of 2^-20, matching the standard's table;
   - no output until 1,024 samples have passed;
   - a failure latches until reboot.
3. **Conditioning:** SHA-256 over tested samples. An input is ready only once twice its 256 bits have been claimed.
4. **HMAC-DRBG** (SP 800-90A, SHA-256), checked against NIST's CAVP vector. It demands a reseed after 65,536 requests.
5. **A seed across boots** (`store::seed`, sectors 7–8). Each boot instantiates the DRBG from fresh pool input, with the stored seed as its nonce, and stores the next seed before anything uses the generator. So a boot whose source repeats an earlier boot's input still never repeats an earlier stream.

## Measured noise

We streamed a million one-byte samples from each source over the UART, from a cold boot, and assessed them with NIST's SP 800-90B tool (`ea_non_iid`, non-IID track, 8-bit symbols). As a control, the same tool rates 1,000,000 bytes of `/dev/urandom` at 6.92 bits per sample. Its estimators are conservative, so 8 is never reached.

| Source | Samples assessed | H_original | 8 × H_bitstring | Min-entropy per sample | On-device health tests at 1 bit |
| --- | ---: | ---: | ---: | ---: | --- |
| Clock jitter: SysTick cycles between 32 kHz edges, low byte | 999,712 | 0.189 | 0.310 | **0.19** | passed |
| ADC internal supply channel (10), low byte of a raw conversion | 972,272 | 2.050 | 1.441 | **1.44** | passed |

- **Clock jitter is weak.** Three values make up 90 % of the samples (0x6f 36 %, 0x39 36 %, 0x70 17 %). The CPU clock and the 32 kHz timer are nearly locked together, so the count varies mainly with interrupts.
- **The ADC is better:** spread over 92 values, the most common at 22 %.
- **The health tests** passed both sources at a 1-bit claim. That shows they catch stuck or grossly broken sources, not an over-claim: the jitter source passed with a fifth of that.

**So the generator claims 1 bit per ADC sample** (8 eighths), below the measured 1.44. A 256-bit input then needs 512 samples, after the 1,024-sample start-up test. The jitter samples are hashed into the pool too, but credited nothing.

## The generator in the firmware

**At boot.** `wellfed-hal::rng` starts in the runtime's startup task, before the power monitor or the firmware can use the ADC. It fills the pool from 1,024 ADC samples (the start-up test's length, which also covers the claim) and closes the ADC again. It records `PLRNG1 boot result=ready samples=1024 first=<16 raw samples> check=<hash of one discarded block>`.

**Drawing.** Every task draws from the one DRBG with the scheduler paused. BLE setup's Noise ephemeral keys come from the generator alone, and a session does not start without it.

**The settings writer.** Once armed, the writer mixes the stored seed into the generator and stores the next one. It opens the client store, and creates the identity secret on the first boot that has a generator. It logs `PLCFG1 keys seed=… mixed=… saved=… clients=… enrolled=… identity=…`. The settings task's stack bound is 2,948 of 4,096 bytes: the client store decodes only the winning slot's table, and saves a table by reference.

**After boot** the power monitor owns the ADC, so the generator never reseeds from it. HMAC-DRBG's reseed interval (65,536 requests) is far beyond a boot's needs, and every boot starts fresh.

## Keys and the panel

**Carrying keys between tasks** (`wellfed-feeder-runtime::keys`):
- The settings writer alone writes the client store. The loop task alone reads the panel and runs the handshakes.
- The loop queues one change at a time (enroll, revoke or reset) in a one-slot mailbox. A second before the writer takes the first is `busy`.
- The writer logs each result as `PLCFG1 keys change=… result=… enrolled=…`.
- A reset is the [factory reset](#factory-reset). It needs the generator, or it does not happen (`result=no-generator`). Nothing weaker is ever written.

**The identity in the loop.** The writer publishes the identity secret in RAM with a per-boot generation. The loop derives the public key (one X25519 on the Montgomery ladder) and keeps the key pair for handshakes. It logs `PLID1 identity generation=N fingerprint=<the public key's first 8 bytes>`. The fingerprint tells identities apart; no record carries the secret.

**Gestures.** Pairing sees each gesture before the panel, which gets only the gestures pairing leaves.
- **Recovery:** press Lock first, then add Dispense, and hold both. Dispense pressed alone first would be a feed request. Lock alone does nothing until its own 5 s hold, and that hold toggles the lock only when Lock is alone.
- **Reset:** a Reset hold (about 5 s) on an unlocked panel starts the [factory reset](#factory-reset). A locked panel refuses it, so nothing changes.

## Factory reset

The Reset hold restores the feeder to a new owner's state. Stock's own restore was never decompiled, so the owner decided what it clears (October 28, 2026).

| Cleared | Kept |
| --- | --- |
| Paired clients and the identity | The feed journal: its high water and any uncertain feed's record, so a feed is never re-run and feed numbers never go back |
| Wi-Fi credentials | The portion calibration, a property of the hardware |
| Settings (lights, sound, volume, their windows, and what later joins the record) | The update store and the boot confirmation: the anti-rollback floor and the build in trial |
| Time settings: the zone and the NTP server | The generator's seed |
| Feeding plans, their marks and last claim | The flash erase counts by sector and of bank 1, the hardware's wear (owner, October 28) |
| The event history and the per-feed traces | |
| The meal call's clip | |
| The rest of the lifetime statistics: uptime, feeds, portions, presses, sessions, link figures and boots by cause | |

`wellfed_board_plaf103::storage::FACTORY_CLEARED` lists the erased stores in the order they are erased. The statistics are not erased but rewritten, as one atomic save, with only the erase counts (`Totals::wear_only`).

**How it runs.** A reset erases 33 sectors and rewrites one store, so it is built to survive a power cut at any point:
1. **The hold.** The loop refuses the hold by name while a feed is queued or runs (`PLPANEL1 reset refused=feeding`) or an install is under way (`reset refused=updating`). Otherwise it queues the reset for the settings writer (`reset keys=queued`). From then until the restart, an install offer is refused `busy`, so the reset's boot never becomes an update's trial boot.
2. **The marker.** The writer takes its write turn, so it waits for a feed's outputs to stop. It programs a 16-byte marker in sector 36. From there the reset is committed: the writer forgets every client and ends every session, and answers `done` even when the client store cannot be written, since the next boot erases it anyway (`PLCFG1 keys change=reset result=done`).
3. **The restart.** The loop reads `done` (`reset result=done`) and restarts the feeder once no feed is queued or running and no plan's claimed meal waits for its feed (a claimed meal runs first, so it is never missed). From the moment a restart is asked for (this one, a client's reboot, or setup mode's end) no feed starts: readiness reports `restarting` and a feed is refused `busy`.
4. **The erases.** The boot that finds the marker logs `PLCFG1 reset pending`. It opens and publishes nothing the reset clears. Once armed, it erases each cleared sector, each under its own write turn and attempt marker, and reads it back blank. Then it saves the statistics with only the erase counts (a store this build cannot write, a newer schema, is left as it is). The statistics pair is the region's most written, so if that save fails it erases the pair instead and goes on, losing only the erase counts (`reset wear_lost=1`); only a pair that takes neither the save nor the erase stops the reset. It erases the marker last (`reset done erased=33`). Events the loop records meanwhile (the boot, the supply, a resolved uncertain feed) wait in its queue and land in the new ring.
5. **The fresh boot.** The stores open empty, a new identity is drawn, and the event ring starts under a new epoch.

**Power cuts.**
- A cut before the marker is whole leaves no reset. Nothing was erased, and the owner holds Reset again.
- A cut after it leaves a reset the next boot finishes, however far the erases got. A torn erase is erased again, and a cut statistics save leaves the old totals or the wear-only ones, either of which the next boot rewrites the same way. A cut while a worn pair is erased leaves part of it, which the next boot saves or erases again.
- A torn erase of the marker itself reads either as the marker, so the erases run again, or as no reset, which is right because they are done.
- The Wi-Fi task reads stored credentials only when no marker is down, so an unfinished reset never joins the old network.
- A boot that cannot erase (unarmed, or an erase that failed or left bits behind) logs `reset error stage=… restart=…`, serves nothing (no identity, no stores, no API, no recovery window or BLE setup) and keeps the marker.
- **The escape.** A boot that stops may ask the loop for one restart (`restart=1`), which comes once no feed or claimed meal is pending. A boot's reset flags cannot tell a hang from an intentional restart (both are the watchdog's reset on this board), so the marker sector counts the armed boots that started the erases: each programs an attempt word after the marker, with no erase, before its first erase. A restart is asked for only while fewer than two are down, and only when it can help: after a failed erase, or after an arming skip (the boot before hung inside a write, the skip cleared the attempt marker, and the restarted boot arms again). So the boot the hold restarts into and one escape restart each get a try; a boot that fails after both (`restart=0`), or whose arming failed for another reason, waits for a power cycle rather than restarting for ever. The attempts are erased with the marker.

**After the reset.**
- The feeder joins with the build's credentials, if it has any, until BLE setup provisions new ones. Setup and the LAN recovery window (Lock then Dispense held, unlocked panel) both admit a new owner, since neither needs stored credentials or a client.
- Event numbers restart at 1 under a new epoch. That is how a client tells a reset from a rewind: a cursor counts within one epoch, so a new epoch means a new history, never missing events ([device API](device-api.md#events-and-samples)). Every old client is forgotten and the identity is new, so an old client cannot reconnect anyway; the server sees the reset feeder as a new one.

## Storage sectors

All storage-region sectors are named in one table, `wellfed_board_plaf103::storage`. A compile-time check keeps the ranges disjoint and inside the region, and the other constants read from it. The region is 48 sectors, 0x340000–0x370000 ([resource budget](resource-budget.md#the-storage-region)).

| Sectors | Owner |
| --- | --- |
| 0–1 | Feed journal |
| 2–3 | Settings |
| 4 | Boot confirmation |
| 5–6 | Wi-Fi |
| 7–8 | RNG seed |
| 9–10 | Identity and clients |
| 11–12 | Feeding plans ([schedules](schedules.md#storage)) |
| 13–24 | 7-day event ring ([device API](device-api.md#events-and-samples)) |
| 25–26 | Update store: the anti-rollback floor, the give-way's candidate, update keys, bank 0's image ([OTA design](ota-design.md#keys-and-anti-rollback)) |
| 27–28 | Time settings: the zone's rule and the NTP server ([clock](clock.md#time-settings)) |
| 29–31 | Per-feed trace ring: the last 8 to 12 feeds' traces ([device API](device-api.md#feed-traces)) |
| 32–33 | Lifetime statistics ([device API](device-api.md#statistics)) |
| 34–35 | Portion calibration ([calibration](calibration.md#storage)) |
| 36 | The factory reset's marker ([factory reset](#factory-reset)) |
| 37–39 | Free |
| 40–47 | The meal call's clip ([schedules](schedules.md#meal-call)) |

## Clients

`store::clients` keeps the feeder's X25519 identity secret and up to 8 enrolled clients, in one 584-byte record written whole to the other of two sectors.

- **Atomic:** an enrollment, revocation or reset lands entirely or not at all. A power-cut test confirms this at every flash mutation.
- **Each client** is its 32-byte static key, a label of up to 16 bytes, a role and the generation that enrolled it.
- **Roles:**
  - **Owner:** everything, including managing clients.
  - **Member:** feeding and settings.
  - **Server:** the optional server.
  - **Update signer:** an owner's Ed25519 update-signing key, enrolled with `EnrollUpdateKey` once someone presses Dispense ([OTA design](ota-design.md#keys-and-anti-rollback)). It never opens a session.
- **Reset:** the panel's factory reset forgets every client, then erases the store, and the next boot draws a new identity.

> [!NOTE]
> The identity secret and the RNG seed sit in plain flash. The board has no secure element, so anyone who reads the flash chip can impersonate the feeder until its next reset. The seed matters less, since every boot mixes fresh input in.

## Pairing

`wellfed_device_api::pairing` decides who may become a client. It returns an `Admit` event, which the firmware applies to the client store. There are four ways in:

| Path | Who starts it | Confirmed by | Role |
| --- | --- | --- | --- |
| Setup | The app over BLE in setup mode, which a Lock and Mic hold on an unlocked panel starts | The app's one-time code keyed in on the buttons within 60 s, then proven by the app ([why](bluetooth.md#why-a-code)) | Owner |
| Signer | An owner over the API | A Dispense press within 60 s | Update signer |
| Invite | A paired client: an owner invites any role but a signer, a member invites members | The invite's secret as the PSK of a Noise XXpsk3 handshake, within 10 min, once | As invited |
| Recovery | Lock and Dispense held together for 5 s on an unlocked panel | Within 2 min, a client offers its key over the device API and proves the code the owner keys in; the first offer closes the window | Owner |

- **Presses that answer a pairing.** A pending signer confirmation takes the next Dispense press. A pending setup code takes every gesture until it is complete, even on a locked panel, since those presses answer a pairing someone at the feeder started. They feed nothing. Records count the presses (`pair setup keyed=N`, or `pair recovery keyed=N`) and never name them.
- **Recovery and the lock:** a locked panel cannot open recovery.
- **Invites** are at most two outstanding, and live in RAM, so a reboot cancels them.
- **The invite's id** travels in the first handshake message's payload. That payload is not secret, and the responder needs it to choose the PSK before the third message.
- **Recovery over the LAN** runs the same setup exchange as BLE, code proof included, over the device API's Noise stream (`Pairing::recovery`). It no longer admits a plain XX handshake, so a LAN neighbour cannot slip in while the window is open.
- **Interop:** `noise.rs` has XXpsk3, checked byte for byte against `snow` in both roles. A wrong or missing PSK fails closed.

The timeouts are ours: stock pairs through the vendor cloud and has no equivalent ([stock behavior](../devices/plaf103/stock/README.md)).

## The pieces of BLE setup

The exchange and its refusals are in [Bluetooth](bluetooth.md#setup-exchange). In code:

| Piece | What it does |
| --- | --- |
| `wellfed-proto::setup` | The request and replies (postcard), shared by the feeder and every client |
| `wellfed-session::setup_code` | The owner's code and the press-by-press proof both sides run, bound to the handshake hash |
| `wellfed_device_api::setup::Responder` | The feeder's side of Noise XX under its identity. It returns the client's checked offer (its key and a label the client store accepts), runs the proof, and returns the Wi-Fi from the client's last step, checked against the Wi-Fi store's rules |
| `wellfed-client::setup::Session` | The client's side, with the BLE framing. The owner's `wellfed setup` and the Android app share it |
| `wellfed_feeder::feeder::setup` | The loop's setup mode, over the firmware's `Platform` |

The loop starts the stack on the gesture and offers the key to pairing (`Pairing::setup`), which collects the code from the buttons and runs the proof. Once the client has proven it, the loop queues the enrollment in the keys mailbox and waits for its result (`keys::ticket` and `keys::outcome`). Then it queues the Wi-Fi (`network::save`) and waits for the writer. A client that leaves before the proof cancels its offer (`Pairing::cancel`), so the next press feeds again.

Setup starts on a gesture rather than at boot because every free KM0 backup register is taken, so a "set up on the next boot" flag has nowhere to live. A reboot still ends it.

## What's left

- **Not yet on hardware:** a deliberately mistyped setup code, and the stale-offer fix ([Bluetooth](bluetooth.md#evidence)).
- **Optional confidence:** SP 800-90B's full restart test (samples across about 1,000 cold boots), and the noise measurement on battery and when warm.

## Evidence

- **Observed** (October 24, 2026), the noise: the `feeder` firmware's `entropy` variant (v103) streamed `PLRNG1` records from a cold boot, 1,000,048 samples from each source, on the research unit on USB power at room temperature. The captures are `fw45-rng-05` to `fw45-rng-11`. Segments end at the flash tool's 512 KiB observe limit, so a few records fell between them. The NIST tool was built from its public repository; it and its scripts stay outside this repository.
- **Observed** (October 24), the generator: it runs in every firmware from boot. `ota fw45-keys-02` (v106) started it and logged `seed=empty saved=1 identity=created`. Across 16 cold boots (`fw45-restart-01`–`16`), all 16 started the generator, their 16 raw first samples differed from each other, and so did their output checks. Every one logged `seed=loaded mixed=1 saved=1 clients=loaded identity=loaded`, so the identity survived 16 cold boots unchanged. This is a sanity check that no boot repeats another, **not** SP 800-90B's restart test.
- **Observed** (October 24), keys and the panel (`feeder` v107, owner pressing, no motion): `ota fw45-pair-01` confirmed, and the identity loaded; `boot fw45-pair-02` showed the same fingerprint after a cold boot. In `capture fw45-pair-03`, a locked panel refused the recovery chord (`refusal=locked`); unlocked, it opened the window, with no feed request and no lock change. In `capture fw45-pair-04`, the window closed after 2 min; a locked Reset hold was refused; unlocked, a Reset hold queued the reset, which logged `result=done enrolled=0`, then a new fingerprint. `boot fw45-pair-05` showed the new fingerprint after a cold boot. The loop's stack bound was then 3,912 of 6,144 bytes.
- **Observed** (October 24), setup and recovery: BLE setup mode end to end, first with a Dispense press and then with the setup code, and recovery over the LAN with the code ([Bluetooth](bluetooth.md#evidence), [device API](device-api.md#evidence)). An invite's redemption and a revocation ran on hardware the same day.
- **Not yet covered:** battery power and temperature; the full restart test.
- **Modeled:** the invite, signer and recovery paths, a wrong recovery code, and the client store under a power cut at every mutation are host-tested.
- **Modeled** (October 28), the factory reset: the settings writer against the SDK fake clears exactly the decided stores and keeps the journal, confirmation, update store and calibration byte for byte, with a power cut after every erase (the cut one torn) and after the marker's (`wellfed-feeder-runtime/tests/writer.rs`). The marker is never pending after a cut part-way through its program (`wellfed-feeder/tests/reset_marker.rs`). Both platforms' conformance suites refuse a hold during a feed and mark one at rest. In the simulator, an owner and a member pair, set everything and feed; after the hold, the member is gone, every cleared store writes from generation 0, the history has a new epoch, the calibration stays, feed numbers carry on, and the recovery window admits a new owner (`wellfed-feeder-sim/tests/serve.rs`). **Not yet on hardware:** the hold with the clearing and the restart.
