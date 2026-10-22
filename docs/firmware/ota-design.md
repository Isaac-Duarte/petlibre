# FW10/FW11 over-the-air update design

Status: the recovery-role updater is implemented (`amebad/src/updater.rs`, host-tested in `amebad/tests/updater.rs`). On hardware (October 22, physical), the build booted from bank 0 after a UART give-way. It bound KM0, opened the watchdog, installed the lease provider into the flash-lock hooks and ran 180 s on one boot with the watchdog fed and no network; bank 1 was then restored. **First live update (October 22, physical).** With the hotspot up and `petlibre-local serve` holding an offer, a UART give-way booted bank 0, and the recovery role then:
- joined the hotspot, connected, and verified manifest version 1 (663 552 bytes);
- wrote 162 sectors under 2 703 leases, reading back every page;
- programmed the marker last and logged `activated`;
- let the watchdog reset it. The loader then booted the offered build (`ed15a3ff…`) from bank 1, and its health reached the server.

The whole run, boot included, fit in the 180 s capture. The first attempt HardFaulted on its first manifest: the signature check needs about 12 KiB of stack, more than the task's 10 KiB. The watchdog reset it, the attempt marker skipped the next try, and the normal transport took over, as designed. The task now has 24 KiB, with the manifest step out of line. The normal role's give-way trigger and FW11's trial counter are not implemented.

The updater: booted from bank 0, with a key built in (`PETLIBRE_OTA_PUBLIC_KEY`, the public half of `private/ota-key.json`), it runs in place of the transport and optics tasks. It sets an attempt marker (a boot that finds it set skips the update), and each erase and page program is one FW06 lease. A feeder task feeds the watchdog while the updater keeps stepping (10 s liveness); a hang, a hook fault or the activation stops it, and the watchdog reset boots the new bank 1. Without a key, or after a stop, the normal transport starts so health stays visible. Records are `PLOTA1` lines.

The original design follows. It came from the FW09 bank policy (simulation and static analysis). Every write it describes is outside the bank-1 development updater's reviewed region, so the first hardware step needs an owner-reviewed procedure.

## Constraints

- **Bank 1 always wins when valid.** The factory loader checks bank 1 first and keeps it whenever its signatures pass; bank 0 boots only when bank 1 fails ([bank policy](bank-policy.md)).
- **No writing the running bank.** Code executes in place from the active bank, and rewriting it under execution is unsafe.
- **The loader never falls back on its own.** A valid-marker image that crashes is selected again on every reset. Both banks invalid means a wait loop that only UART recovers.
- **Writes go through the FW06 flash authority,** with outputs quiesced and KM0 parked for each lease. No motion during an update.

## Roles by bank

One firmware build runs from either bank, because both map to the same virtual addresses. Its role follows from the bank it started in, which the startup report already reads from the MMU:

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

Implemented on KM0 (`amebad/src/trial.rs`, `petlibre_trial_step` in `firstboot_km0.c`), before `km4_boot_on`:
- **Running bank:** read from bank 1's physical marker.
- **Confirmation:** keyed by the build identity, not the image hash, since the normal image does not know its own OTA hash. The record lives in storage sector 4 (`0x344000`).
- **Count:** backup register 4. Bits 0–15 hold the peer boot count, bits 16–23 the trial count, and bits 24–31 an identity tag.
- **Give way:** at boot 4, only if bank 0's marker is valid, KM0 clears byte 0 and resets with its own watchdog.

The program step bypasses `FLASH_Write_Lock`. After a watchdog reset, that lock's KM0 branch waits forever for an absent KM4, because `km4_status_on` still reads the HS clock bit set. This was observed on hardware, then fixed.

Physical (October 22): the `fw11-reset` test variant (`build.py --trial-reset`) was counted 1, 2, 3, then gave way and booted bank 0's recovery role. The normal role now confirms itself (`amebad/src/confirm.rs`). Once the startup probe has ended, the transport is ready and outputs are verified stopped for 30 s continuously, it arms the hooks and writes the record to sector 4, then reads it back. The normal role also opens the watchdog at startup. A feeder task feeds it while the transport and confirmation tasks keep stepping, and while outputs are stopped or sensing, so a hang becomes a reset that KM0 counts.

Physical (October 22), over the air:
1. The first build with this code had a feeder bug: it demanded all outputs stopped and tripped during the optics windows. KM0 counted three resets and gave way. The recovery role then installed the fixed build (version 3) over the air.
2. The fixed build confirmed after 30 s healthy (`PLCNF1 confirmed=1 healthy_ms=30000`).
3. On the next cold boot KM0 reported `PLTRL1 confirmed=1`.

Caution, observed: while an offer stays loaded, a candidate that keeps failing is re-downloaded after every fallback. The server should drop an offer once its version has been activated or has failed.

The original design:

- **Counting:** KM0 runs first on every boot. If bank 1's image is unconfirmed, KM0 increments a trial count in a backup register. Backup registers survive the watchdog reset but not power loss, so a power cut only restarts the count.
- **Fallback:** when the count reaches 3, KM0 clears the first byte of bank 1's marker before releasing KM4, then resets. The loader boots bank 0, and the recovery image reports a failed trial.
- **Confirmation:** a confirmation record, written once per installed image (keyed by the image's SHA-256) in the replacement storage region at `0x340000`. An image is confirmed only when the record names its exact hash.
- **The hang case:** a KM4 hang becomes a counted reset through the FW05 watchdog.

Limits: if the candidate's own KM0 code is broken, nothing counts, and UART recovery is needed. That stays the documented backstop, and the relay bench can do it unattended.

## Authentication

The FW08 channel is plaintext on the shared hotspot, so the transport is not trusted. The update is: the manifest carries the image's SHA-256, version, size and target bank, and is signed by an owner-held key. The private key stays offline under `private/`, and the firmware embeds only the public key at build time.

Candidate algorithm: Ed25519, verified in `petlibre-core` (`no_std`), with the SHA-256 also computed there, so both can be host-tested against known vectors. Replay and downgrade are refused by a monotonic version stored next to the confirmation record. The transport may still be sniffed; it reveals only the image, which is not secret.

## Transfer

The recovery image pulls; it is never pushed:
- It publishes `ota/get {offset, length}` on its own namespace, and the server answers `ota/data` with that chunk.
- Chunks are at most 512 bytes, so an answer fits the FW08 codec's existing 640-byte packet bound with its topic, leaving the running transport's buffers unchanged. A compile-time assert keeps it that way.
- The writer stages a whole sector in RAM, then the port erases, programs and reads it back in one lease.
- A missing or late chunk is re-requested a bounded number of times, then the attempt fails into step 4's safe state.

The server-side broker ACL gains these two topics for the firmware role only.

## What can be built and tested offline

- **Core (done):** `petlibre-core::ota`, using `ed25519-dalek` and `sha2` with default features off, so it stays `no_std`. It holds manifest parsing and signature verification, the bank-1 sector writer, the trial decision and the confirmation record. The manifest refuses tampering, the wrong key, a bank other than 1, a size outside one sector to 0x150000, and downgrades. The writer withholds the marker until every sector reads back and the whole-image SHA-256 matches, and it refuses out-of-order chunks, a missing marker, failed readback, corruption and truncation. Host tests include RFC 8032 test 1.
- **Fakes:** the bank-role decision and the KM0 trial counter, against the fake SDK.
- **Emulation (done):** `tools/emulate/emulate-ota-markers.py` hands each modeled flash state to the factory loader's own selection code. The states are every step above, a bank-1 marker cut after each of its 8 bytes, and erased or staged bodies. It passed all 14 states for both stock and the replacement build. Only the forbidden step, giving way with no valid recovery in bank 0, leaves nothing to boot. Any single cleared bit in bank 1's marker gives way to bank 0.
- **Server:** manifest signing (done: `petlibre-local ota-keygen` and `ota-sign`, round-tripped through the firmware's own parser and writer in a test). Delivery is done too: `ota-offer`, the broker ACL and the server's answers. A loopback test has the firmware's own codec, parser and writer pull a signed image through the real broker and server.

## Hardware stages (owner-reviewed)

1. **Recovery in bank 0:** install a recovery-role image in bank 0, and confirm that bank 1 still boots (the FW09 confirmation).
2. **Give way:** clear bank 1's marker, and confirm that bank 0's recovery role starts and reports.
3. **Update:** a signed update of a changed build into bank 1. It must activate and confirm.
4. **Failures:** corrupt, truncated and interrupted transfers, a trial that never confirms (it must fall back after 3 boots), and a power cut mid-sector.

Each stage ends with full-flash reads. The bank-1 UART updater stays the recovery path throughout.
