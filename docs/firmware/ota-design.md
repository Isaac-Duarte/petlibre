# FW10/FW11 over-the-air update design

**Status (October 25, 2026): updates over the device API, host-tested.** Offers, downloads and key changes now go over the [device API](device-api.md) behind `Capabilities::UPDATE`, and both banks run the `feeder` build: bank 0 is the fallback a failed update lands on, and it feeds. No build opens an MQTT session. The bench and the simulator drive the same client steps (`wellfed_client::update`). Not yet on hardware: the first install of this bank-0 build is a UART install, then the no-motion round trip in the [bench procedure](#bench-procedure). The sections from [Updates over the device API](#updates-over-the-device-api) to [Storage-format rule](#storage-format-rule) describe it; the rest of this page is the history it builds on, the MQTT-era updater included. That updater's code, the broker offers, the version 1 manifest and its signer, and the broker credentials in the build are deleted (in Git history); only stock 1.1.15 still speaks MQTT, to `wellfed-local`'s stock broker.

## Updates over the device API

**Roles.** The bank decides only what the update stage does; both banks start the firmware, the settings writer and the update stage (`wellfed_hal::ota::update`) in the service task:

| Bank | What it does with an offer |
| --- | --- |
| 1 | Checks it, then checks that bank 0 still holds exactly the image bank 0 recorded at its last boot (length from the bank's headers, SHA-256), records the offer as pending in the update store, answers `GivingWay`, and once that answer is out clears its own marker's first byte and restarts. It never writes bank 1. Until the build confirms its hooks are unarmed and every offer is refused `Busy`; a bank 1 in trial writes nothing. It also refuses (`Fallback`) an offer signed by a built-in key that bank 0's record does not name. |
| 0 | Records its own image at boot (only when it changed). Installs only the pending manifest, or the offered one when none is pending (fresh from stock, or after a fallback). Refuses a manifest that was activated and failed its trial, and every offer after a reset mid-install until power is lost. Never confirms. |

**Flow.** An owner's client offers a signed version 2 manifest to bank 1 (`OfferUpdate`); bank 1 gives way; the client reconnects (the same identity answers from bank 0) and offers the same manifest again; bank 0 answers `Receiving` and names the chunk offsets it wants; the client pushes them (`UpdateChunk`, 512 bytes, idempotent by offset); bank 0 erases, programs and reads back each sector, checks the whole image's SHA-256, records the activation in the update store, then writes bank 1's marker last and reads it back, answers `Activated` and restarts. The new build's trial and confirmation are unchanged; confirming raises the floor (below) before the confirmation record.

**The loop never verifies.** The device API runs in the loop task on an 8 KiB stack; an Ed25519 check needs about 8 KiB. The loop copies requests and chunks into a mailbox (`wellfed_hal::ota::mailbox`) and answers from what the service task's update stage publishes. Feeder build stack bounds (static, the image audit, October 25): service task 11,504 of 16,384 bytes, its deepest path the install's start (the offer's `verify_strict` is shallower); loop 5,016 of 8,192, a handshake. Nothing on the loop's side calls a signature check.

**No store or bank write while a feed may move.** Every update request (offer, cancel, key statement) takes the install lease as it is queued, in the loop that alone starts feeds: each is refused `Busy` while a feed is queued or driven, and a feed is refused `Busy` while the lease is held, at the device API, in `Feeding::submit` and in the HAL feeding owner's `submit`. The stage releases the lease once it answers, except for an accepted offer: the install keeps it until it stops, a give-way or an activation until the restart; a cancel during an install rides on the install's lease. Every update-store save and bank-1 write also takes the write turn (`wellfed_hal::interlock::WriteTurn`, as the settings writer and the confirmation do), never by the forced path: a request waits in the mailbox, unanswered, while the outputs are claimed; bank 0's boot record of its image waits the same way; an install holds the turn from its first write until it stops, so every feed meanwhile is refused (`held=3`); a give-way or an activation keeps it until the restart, so no feed starts before the reset. The floor the confirmation raises is written under the confirmation's own turn. The attempt marker is set only once the turn is held, around each write, as the settings writer's are, so a hang inside one is skipped once after the reset. The install and the settings writer share the service task, so their flash writes never overlap; the journal's only come with feeds.

**Bootstrap.** Bank 1 gives way only to a bank 0 that recorded its own image (length, SHA-256 and built-in key id) in the update store, which bank 0 does at its first boot. So after a UART install of bank 0, bank 1 refuses every offer `Fallback` until bank 0 has booted once: the reviewed UART install boots bank 1, so the bank-0 record comes with the first give-way through UART (`give-way-execute --fallback`), or a failed trial. Both banks must carry the same built-in keys: bank 1 checks the offer's key against bank 0's recorded key id before it gives way, so an offer bank 0 could not admit never strands the feeder on bank 0.

**Stops.** Every failure before activation leaves bank 1's marker clear, so the loader keeps booting bank 0, which keeps answering and feeding: a flipped byte (the whole-image hash), a chunk the install did not want or of the wrong length (order), a failed program or readback (write), a marker that does not read back, a minute without a chunk (timeout), a cancel, an update store that cannot be written, a manifest or key the checks refuse. A stop leaves the candidate recorded, so bank 0 refuses any other manifest (`Pending`) until a `CancelUpdate` forgets it (`wellfed-bench ota NAME --cancel`); a cancel answers `Busy` when the store cannot be written then, rather than claiming it forgot. A dropped connection loses nothing: the install waits a minute for the next chunk, and `UpdateState` says where it stands. A power cut mid-download boots bank 0 again, which takes the offer again from the start. A reset mid-install sets the update-attempt bit (backup register 1, bit 9, beside the flash attempt bit): offers are refused until power is lost, but the hooks still arm, so bank 0 still saves and feeds. Host-tested: `wellfed-hal/tests/update.rs` (every FW10 stop against the fake SDK), `wellfed-client/tests/update.rs` and `wellfed-feeder-sim/tests/update.rs` (the client end to end, over the fake and over TCP against the simulator, which runs the real stage on two banks with the loader and KM0's trial).

## Keys and anti-rollback

**Manifest version 2** (`wellfed_core::ota::offer`): `PLOTA2\0\0`, version, image length, target bank, image kind (1 replacement, 2 stock) and three zero bytes, the signing key's id (the first 8 bytes of SHA-256 over its public key), the image's SHA-256 and the build ID: 96 bytes, then the Ed25519 signature. Version 1 (`PLOTA1`, the MQTT era's) is deleted; an offer carrying its magic is refused as the wrong version, and the bench still reads the versions of the ones on disk when it picks the next.

**Trusted keys:** the key built into the firmware (the bench key, `WELLFED_OTA_PUBLIC_KEY`); the release slot, empty in the build and filled once by a `PLKEY1` statement a built-in key signed (`UpdateKeyStatement`), so a release key arrives without reinstalling bank 0; and owners' update keys, enrolled over the device API (`EnrollUpdateKey`) only once someone presses Dispense at the feeder, and removed like any client. A `PLREV1` statement retires any key id for good. Built-in keys and the release slot's key sign key statements, so retiring the bench key leaves rotation to the release key; an owner's key never signs one, and a retired key signs nothing. The bench key stays in `private/ota-key.json`, bench only; an offline release key is required before any public release.

**The floor** (`min_version`): the highest version a build confirmed at. An offer at or below it, and every stock image, needs a `PLDWN1` downgrade statement naming this feeder's identity key, the exact manifest's digest and the current floor, signed by a trusted key; a replayed old manifest, a statement for another feeder or made before the floor last rose is refused. Confirmation raises the floor after arming and before it erases the confirmation sector; a store that cannot be written leaves the build unconfirmed. Sector 4's version field carries the version bank 0 activated, and 0 for a build installed over UART, or after a torn confirmation rewrite (the store's candidate was already confirmed, so the retry finds nothing naming the build). Nothing decides on that field; the floor lives in the update store.

**The update store** (`wellfed_core::store::update`, storage sectors 25–26): one 256-byte record A/B, newest valid wins, holding the floor, the candidate (pending, then activated), the release slot's key, up to four retired ids and bank 0's recorded image. A record that would lower the floor or forget a retired id is never written; a torn write leaves the older, consistent record. It is not the confirmation sector, which is erased before each write. The factory reset leaves it alone.

Signing: `wellfed-build sign` (version 2), `downgrade`, `revoke` and `add-key`, all with the bench key.

## Storage-format rule

Storage formats stay readable by the bank-0 build, or a format change ships a bank-0 refresh first. A store bank 0 cannot read (a newer schema) has no floor it can trust, so it refuses every offer: a format change that skipped the refresh strands the unit on bank 0 until a UART install.

## Bench procedure

Not run yet; the orchestrator's, with no motion. `wellfed-bench ota` and `itest` push updates over the device API ([bench](../devices/plaf103/bench.md)). In the push model a truncated download is a stall: the bench simply stops sending, and the install times out (`stage=timeout`). Between failure drills, `ota NAME --cancel` forgets the candidate a stop left recorded. The hands-off round trip offers bank 0's build back from bank 1, so the build in bank 1 must have another build ID: R1 is the feeder's `twin` variant (`--variant twin`), R0 the plain feeder. The first install of this bank-0 build is a reviewed UART install (`flash-prepare/flash-execute --bank 0`); bank 1 keeps running until a give-way. Bring-up firmwares (`qa`, `optics` and the rest) serve no device API, so a unit left on one in bank 1 takes no offer: leaving it needs a UART install or a failed trial.

## History: the MQTT updater

Status: the recovery-role updater is implemented (`wellfed-hal/src/ota/updater.rs`, host-tested in `wellfed-hal/tests/updater.rs`). On hardware (October 22, physical), the build booted from bank 0 after a UART give-way. It bound KM0, opened the watchdog, installed the lease provider into the flash-lock hooks and ran 180 s on one boot with the watchdog fed and no network; bank 1 was then restored. **First live update (October 22, physical).** With the hotspot up and `wellfed-local serve` holding an offer, a UART give-way booted bank 0, and the recovery role then:
- joined the hotspot, connected, and verified manifest version 1 (663 552 bytes);
- wrote 162 sectors under 2 703 leases, reading back every page;
- programmed the marker last and logged `activated`;
- let the watchdog reset it. The loader then booted the offered build (`ed15a3ff…`) from bank 1, and its health reached the server.

The whole run, boot included, fit in the 180 s capture. The first attempt HardFaulted on its first manifest: the signature check needs about 12 KiB of stack, more than the task's 10 KiB. The watchdog reset it, the attempt marker skipped the next try, and the normal transport took over, as designed. The task now has 24 KiB, with the manifest step out of line. The normal role's give-way trigger and FW11's trial counter are not implemented.

The updater: booted from bank 0, with a key built in (`WELLFED_OTA_PUBLIC_KEY`, the public half of `private/ota-key.json`), it runs in place of the transport and optics tasks. It sets an attempt marker (a boot that finds it set skips the update), and each erase and page program is one FW06 lease. A feeder task feeds the watchdog while the updater keeps stepping (10 s liveness); a hang, a hook fault or the activation stops it, and the watchdog reset boots the new bank 1. Without a key, or after a stop, the normal transport starts so health stays visible. Records are `PLOTA1` lines.

**Health while waiting (FW40).** With no offer, the updater asks again every 10 s and never hands over to the transport. A build that stock just installed, which boots from bank 0 with bank 1 invalid, would then report nothing. So the updater publishes health from its own MQTT session after a `no_offer` answer, at most every 30 s. It reports bank 0 and `"role":"recovery"`. It is the only client: the transport shares its client ID. Nothing else changes in the recovery role: no flash access, no `Watch`, no firmware tasks. Only this role writes `role`, so normal health is unchanged. Host-tested (`tests/updater.rs`), with fresh builds passing every audit (`wellfed-ota` stack 10024 of 24576 B).

**Hands-off update (October 22, physical).**
- **The offer check:** a confirmed normal build runs `amebad/src/watch.rs`. Every minute it opens a short MQTT session of its own, as `<client_id>-ota`, publishes `ota/check`, and the server answers `ota/offered`.
- **On hardware:** build W checked and found its own build on offer (`same=1`). After the owner side loaded signed v5 for another build, the next check made W clear its bank-1 marker (`sdk::bank1_give_way`, refused unless bank 0 is valid) and restart. Bank 0 installed v5, which confirmed itself after 30 s. Nothing went over UART.

**Failure modes (October 22, physical).** `src/tools/wellfed-local/examples/ota_fault_server.rs` answers `ota/get` as the server role, beside `wellfed-local broker` only, with one injected fault:

| Fault | Result |
| --- | --- |
| One bit flipped in the chunk at 8192 | Full download, then `error stage=hash` |
| `Range` from 200 000 | `error stage=range` after 48 sectors |
| No answers from 300 000 | Five asks 3 s apart, then `error stage=timeout` |
| Broker and server restarted at 300 032 | `closed`, three refused connects, then resumed at 300 544 and activated |
| Relay power cut at 201 728 | Next boot on bank 0, re-downloaded from 0, activated |

After each stop, the marker was never written: the loader kept booting bank 0, and the recovery role handed over to the normal transport. The transfer ran at about 6.5 KB/s, a 672 KB image in about 100 s. Most of that was one 512-byte ask per round trip, each followed by the task's 50 ms poll delay.

**Pipelined transfer (October 22, physical).** The recovery role now asks for all eight chunks of the sector it is staging at once, each with its own retries. It polls again after 1 ms while answers keep arriving. The writer accepts a sector's chunks in any order and hashes the sector in order once it is complete. Sectors are still written strictly in order, and the marker checks, readback, whole-image hash and marker-last commit are unchanged. It runs in bank 0, which was reflashed over UART for it. In `itest-20261022-18`, each 680 KB download took 18–19 s from manifest to activation, about 37 KB/s.

The original design follows. It came from the FW09 bank policy (simulation and static analysis). Every write it describes is outside the bank-1 development updater's reviewed region, so the first hardware step needs an owner-reviewed procedure.

## Constraints

- **Bank 1 always wins when valid.** The factory loader checks bank 1 first and keeps it whenever its signatures pass; bank 0 boots only when bank 1 fails ([bank policy](bank-policy.md)).
- **No writing the running bank.** Code executes in place from the active bank, and rewriting it under execution is unsafe.
- **The loader never falls back on its own.** A valid-marker image that crashes is selected again on every reset. Both banks invalid means a wait loop that only UART recovers.
- **Writes go through the FW06 flash authority,** with outputs quiesced and KM0 parked for each lease. No motion during an update.

## Roles by bank (MQTT era)

Superseded by [Updates over the device API](#updates-over-the-device-api), where both banks run the feeder. One firmware build runs from either bank, because both map to the same virtual addresses. Its role followed from the bank it started in, which the startup report already reads from the MMU:

| Bank | Role |
| --- | --- |
| 1 | **Normal:** feeding, health and transport. Never writes its own bank. |
| 0 | **Recovery and updater:** health and transport only, motor and emitters held off. It downloads, verifies and activates candidates into bank 1. |

The bank-0 image is the confirmed known-good fallback. It changes rarely, and only by a reviewed procedure.

## Update flow

1. **Announce:** the server announces a signed update manifest. The normal image (bank 1) checks it: signature, version, target bank 1, size within the reviewed bound.
2. **Give way:** the normal image verifies that bank 0 holds a valid, confirmed recovery image. It then clears the first byte of its own bank-1 marker (one page program under the flash authority) and resets. The loader boots bank 0.
3. **Download:** the recovery image pulls the candidate in chunks over the local transport and writes bank 1 sector by sector, with readback, leaving the 8 marker bytes erased. It hashes as it writes.
4. **Activate:** after the whole image matches the manifest's SHA-256, it writes bank 1's marker last and resets. Bank 1 now wins.
5. **Trial:** the candidate boots unconfirmed. KM0 counts its boots (below). Once the candidate's startup report, stopped outputs, Wi-Fi and transport are healthy, it records itself confirmed and reports its build ID.

If anything fails before step 4, bank 1's marker is still clear, so the loader keeps booting the recovery image, which reports the failure. Corrupt, truncated and interrupted transfers all land there.

## Boot confirmation and fallback (FW11)

Implemented on KM0 (`wellfed-hal-common/src/trial.rs`, `wellfed_trial_step` in `firstboot_km0.c`), before `km4_boot_on`:
- **Running bank:** read from bank 1's physical marker.
- **Confirmation:** keyed by the build identity, not the image hash, since the normal image does not know its own OTA hash. The record lives in storage sector 4 (`0x344000`). So reinstalling the build the record already names skips the trial. KM0 logs `PLTRL1 confirmed=1` straight after activation, and the build logs `PLCNF1 confirmed=1 already=1`. The integration suite saw this on hardware (October 22). A test that needs a trial must therefore install a build the record does not name.
- **Count:** backup register 4. Bits 0–15 hold the peer boot count, bits 16–23 the trial count, and bits 24–31 an identity tag.
- **Give way:** at boot 4, only if bank 0's marker is valid, KM0 clears byte 0 and resets with its own watchdog.

The program step bypasses `FLASH_Write_Lock`. After a watchdog reset, that lock's KM0 branch waits forever for an absent KM4, because `km4_status_on` still reads the HS clock bit set. This was observed on hardware, then fixed.

Physical (October 22): the `stall` test variant (now `wellfed-build build --profile firstboot --firmware trial --variant stall`) was counted 1, 2, 3, then gave way and booted bank 0's recovery role. The normal role now confirms itself (`amebad/src/confirm.rs`). Once the startup probe has ended, the transport is ready and outputs are verified stopped for 30 s continuously, it arms the hooks and writes the record to sector 4, then reads it back. Without the MQTT session, "the transport is ready" became the Wi-Fi task's network check, and since October 25 that check is the [link's liveness](startup-diagnostics.md#link-liveness-without-internet-dns): associated, leased, and the lease's gateway answering an ICMP echo, or never having answered this join. It needs no internet DNS, so a candidate confirms on a LAN without a route out; a gateway that goes silent after answering restarts the 30 s. The normal role also opens the watchdog at startup. A feeder task feeds it while the transport and confirmation tasks keep stepping, and while outputs are stopped or sensing, so a hang becomes a reset that KM0 counts.

**Supervision from startup (October 22, physical).** The normal role used to open the watchdog only after its startup probes, so a fault before then hung with nothing to count it. That happened once: a crate-split build overflowed a task stack in early startup and needed a UART give-way. The feeder task now starts with the application and opens the watchdog on its first step (`PLSUP1 open watchdog=1 trial=N`), before the startup diagnostic. The decision is `amebad/src/supervise.rs`:
- **Startup:** feed while the startup task keeps stepping.
- **Determined startup failure:** a trial image stops feeding, so the failure is a counted reset. A confirmed image keeps feeding and holds STOP, so there is no reset loop.
- **Running:** feed while every running task steps and outputs stay stopped or sensing.
- **Trial deadline:** a trial image that has not confirmed within 10 minutes of boot stops feeding.

Each stop logs `PLSUP1 stop …` once. On hardware, over the air, each of these was counted 1, 2, 3 and then gave way to bank 0:
- a failed-start variant (`--variant fail`), stopped 253 ms into each boot;
- the hang variant (`--variant stall`);
- a normal build whose server was stopped after activation, which reset at `ms=600192` on each boot.

Physical (October 22), over the air:
1. The first build with this code had a feeder bug: it demanded all outputs stopped and tripped during the optics windows. KM0 counted three resets and gave way. The recovery role then installed the fixed build (version 3) over the air.
2. The fixed build confirmed after 30 s healthy (`PLCNF1 confirmed=1 healthy_ms=30000`).
3. On the next cold boot KM0 reported `PLTRL1 confirmed=1`.

Caution, observed (MQTT era): while an offer stayed loaded, a candidate that kept failing was re-downloaded after every fallback, so the server then served each offer to completion only once. The device API's client pushes each offer once and never on its own again.

The original design:

- **Counting:** KM0 runs first on every boot. If bank 1's image is unconfirmed, KM0 increments a trial count in a backup register. Backup registers survive the watchdog reset but not power loss, so a power cut only restarts the count.
- **Fallback:** when the count reaches 3, KM0 clears the first byte of bank 1's marker before releasing KM4, then resets. The loader boots bank 0, and the recovery image reports a failed trial.
- **Confirmation:** a confirmation record, written once per installed image (keyed by the image's SHA-256) in the replacement storage region at `0x340000`. An image is confirmed only when the record names its exact hash.
- **The hang case:** a KM4 hang becomes a counted reset through the FW05 watchdog.

Limits: if the candidate's own KM0 code is broken, nothing counts, and UART recovery is needed. That stays the documented backstop, and the relay bench can do it unattended.

## Authentication

The FW08 channel is plaintext on the shared hotspot, so the transport is not trusted. The update is: the manifest carries the image's SHA-256, version, size and target bank, and is signed by an owner-held key. The private key stays offline under `private/`, and the firmware embeds only the public key at build time.

Candidate algorithm: Ed25519, verified in `wellfed-core` (`no_std`), with the SHA-256 also computed there, so both can be host-tested against known vectors. Replay and downgrade are refused by a monotonic version stored next to the confirmation record. The transport may still be sniffed; it reveals only the image, which is not secret.

## Transfer

MQTT era, deleted: updates now go over the device API, where the client pushes the chunks bank 0 asks for ([above](#updates-over-the-device-api)). The recovery image pulled; it was never pushed:
- It publishes `ota/get {offset, length}` on its own namespace, and the server answers `ota/data` with that chunk.
- Chunks were at most 512 bytes, so an answer fit the FW08 codec's 640-byte packet bound with its topic. The device API's download keeps the 512-byte chunk (`wellfed_core::ota::MAX_CHUNK`), which divides a sector.
- The writer stages a whole sector in RAM, then the port erases, programs and reads it back in one lease.
- The current sector's chunks are asked for together. A missing or late chunk is re-requested a bounded number of times, then the attempt fails into step 4's safe state.

The server-side broker ACL gave these two topics to the firmware role only.

## What can be built and tested offline

- **Core (done):** `wellfed-core::ota`, using `ed25519-dalek` and `sha2` with default features off, so it stays `no_std`. It holds manifest parsing and signature verification, the bank-1 sector writer, the trial decision and the confirmation record. The manifest refuses tampering, the wrong key, a bank other than 1, a size outside one sector to 0x150000, and downgrades. The writer withholds the marker until every sector reads back and the whole-image SHA-256 matches, and it refuses out-of-order chunks, a missing marker, failed readback, corruption and truncation. Host tests include RFC 8032 test 1.
- **Fakes:** the bank-role decision and the KM0 trial counter, against the fake SDK.
- **Emulation (done):** `every_ota_step_and_marker_cut_keeps_a_bootable_bank` in `src/tools/wellfed-emu/tests/factory.rs` (formerly `emulate-ota-markers.py`, ported October 23, 2026, and rerun on stock) hands each modeled flash state to the factory loader's own selection code. The states are every step above, a bank-1 marker cut after each of its 8 bytes, and erased or staged bodies. It passed all 14 states for both stock and the replacement build. Only the forbidden step, giving way with no valid recovery in bank 0, leaves nothing to boot. Any single cleared bit in bank 1's marker gives way to bank 0.
- **Server:** manifest signing (done: `wellfed-build keygen` and `sign`, which signs only a frozen review and passes the firmware's own offer check in a test; the key is the bench key, and a public release needs an offline release key first). Delivery is the device API's (`wellfed_client::update`); the MQTT delivery (`ota-offer`, the broker ACL and the server's answers) is deleted.

## Hardware stages (owner-reviewed)

1. **Recovery in bank 0:** install a recovery-role image in bank 0, and confirm that bank 1 still boots (the FW09 confirmation).
2. **Give way:** clear bank 1's marker, and confirm that bank 0's recovery role starts and reports.
3. **Update:** a signed update of a changed build into bank 1. It must activate and confirm.
4. **Failures:** corrupt, truncated and interrupted transfers, a trial that never confirms (it must fall back after 3 boots), and a power cut mid-sector.

Each stage ends with full-flash reads. The bank-1 UART updater stays the recovery path throughout.
