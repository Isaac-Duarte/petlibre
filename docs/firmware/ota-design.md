# FW10/FW11 over-the-air update design

Status: design, from the FW09 bank policy (simulation and static analysis). Nothing here has run on hardware. Every write it describes is outside the bank-1 development updater's reviewed region, so the first hardware step needs an owner-reviewed procedure.

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

- **Counting:** KM0 runs first on every boot. If bank 1's image is unconfirmed, KM0 increments a trial count in a backup register. Backup registers survive the watchdog reset but not power loss, so a power cut only restarts the count.
- **Fallback:** when the count reaches 3, KM0 clears the first byte of bank 1's marker before releasing KM4, then resets. The loader boots bank 0, and the recovery image reports a failed trial.
- **Confirmation:** a confirmation record, written once per installed image (keyed by the image's SHA-256) in the replacement storage region at `0x340000`. An image is confirmed only when the record names its exact hash.
- **The hang case:** a KM4 hang becomes a counted reset through the FW05 watchdog.

Limits: if the candidate's own KM0 code is broken, nothing counts, and UART recovery is needed. That stays the documented backstop, and the relay bench can do it unattended.

## Authentication

The FW08 channel is plaintext on the shared hotspot, so the transport is not trusted. The update is: the manifest carries the image's SHA-256, version, size and target bank, and is signed by an owner-held key. The private key stays offline under `private/`, and the firmware embeds only the public key at build time.

Candidate algorithm: Ed25519, verified in `wellfed-core` (`no_std`), with the SHA-256 also computed there, so both can be host-tested against known vectors. Replay and downgrade are refused by a monotonic version stored next to the confirmation record. The transport may still be sniffed; it reveals only the image, which is not secret.

## Transfer

The recovery image pulls; it is never pushed:
- It publishes `ota/get {offset, length}` on its own namespace, and the server answers `ota/data` with that chunk.
- Chunks are at most 4 KiB, a whole sector, so each sector is staged in RAM, then erased, programmed and read back in one lease.
- The MQTT codec's packet bound grows for these two topics only.
- A missing or late chunk is re-requested a bounded number of times, then the attempt fails into step 4's safe state.

The server-side broker ACL gains these two topics for the firmware role only.

## What can be built and tested offline

- **Core:** manifest parsing, signature verification, the chunk/sector state machine, hashing, and version checks, all sans-IO with host tests.
- **Fakes:** the bank-role decision and the KM0 trial counter, against the fake SDK.
- **Emulation (done):** `tools/emulate/emulate-ota-markers.py` hands each modeled flash state to the factory loader's own selection code. The states are every step above, a bank-1 marker cut after each of its 8 bytes, and erased or staged bodies. It passed all 14 states for both stock and the replacement build. Only the forbidden step, giving way with no valid recovery in bank 0, leaves nothing to boot. Any single cleared bit in bank 1's marker gives way to bank 0.
- **Server:** manifest signing tooling and the `ota/get`/`ota/data` handling, against the real broker in a loopback test.

## Hardware stages (owner-reviewed)

1. **Recovery in bank 0:** install a recovery-role image in bank 0, and confirm that bank 1 still boots (the FW09 confirmation).
2. **Give way:** clear bank 1's marker, and confirm that bank 0's recovery role starts and reports.
3. **Update:** a signed update of a changed build into bank 1. It must activate and confirm.
4. **Failures:** corrupt, truncated and interrupted transfers, a trial that never confirms (it must fall back after 3 boots), and a power cut mid-sector.

Each stage ends with full-flash reads. The bank-1 UART updater stays the recovery path throughout.
