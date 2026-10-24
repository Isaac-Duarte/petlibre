# Keys, randomness and pairing (FW45)

Status: host-tested groundwork, plus a physical measurement of the noise sources (October 24, 2026). The generator is not wired into the firmware yet: the firmware still uses its old entropy mix, and no client can pair. This page records the design the code implements, and what is left before the [FW45 card](../../FIRMWARE_BOARD.md) can close.

## No hardware RNG

**Static analysis of the pinned SDK:**
- The AmebaD SDK exposes no hardware random generator.
- `rtw_get_random_bytes` calls `arc4random`, which on the 8721D is ROM `Rand()`: a Tausworthe generator whose four words start at the constant 12345 (`swlib/string/rand.c`).
- The firmware's current `entropy()` mixes that output with SysTick and the tick count (`example_entry.c`).
- So the FW08 request nonce and FW44's throwaway Noise keys rest on timer values alone.
- The request nonce only has to differ between sessions, which timers give. A long-term key needs real entropy.

**What replaces it** (`petlibre-core::rng`):
1. **Raw noise** (`petlibre-hal::entropy`): the low byte of a raw ADC conversion of the chip's internal supply, and SysTick's cycle count between 32 kHz timer edges. Both were measured on the bench ([below](#measured-noise-physical-october-24)).
2. **Health tests** (SP 800-90B 4.4):
   - A Repetition Count Test and an Adaptive Proportion Test (window 512) on every sample.
   - Their cutoffs come from the min-entropy the source claims per sample, at a false-alarm rate of 2^-20; they match the standard's table.
   - No output until 1,024 samples have passed.
   - A failure latches until reboot.
3. **Conditioning:** SHA-256 over tested samples. An input is ready only once twice its 256 bits have been claimed.
4. **HMAC-DRBG** (SP 800-90A, SHA-256):
   - It is checked against NIST's CAVP vector.
   - It demands a reseed after 65,536 requests.
5. **Seed across boots** (`store::seed`, sectors 7–8):
   - Each boot instantiates the DRBG from fresh pool input, with the stored seed as its nonce.
   - It stores the next seed before anything uses the generator.
   - So a boot whose source repeats an earlier boot's input still never repeats an earlier stream.

## Measured noise (physical, October 24)

**Method:**
- The `feeder` firmware's `entropy` variant (v103) streamed one-byte samples over UART (`PLRNG1` records) from a cold boot: 1,000,048 from each source.
- The research unit ran on USB power at room temperature.
- NIST's SP 800-90B tool assessed the stream (`ea_non_iid`, non-IID track, 8-bit symbols), built from its public repository. The tool and its scripts stay outside the repository.
- The captures are `fw45-rng-05` to `fw45-rng-11`. Segments end at the flash tool's 512 KiB observe limit, so a few records fell between them.

**Control:** the same tool rates 1,000,000 bytes of `/dev/urandom` at 6.92 bits per sample. Its estimators are conservative, so 8 is never reached.

| Source | Samples assessed | H_original | 8 × H_bitstring | Min-entropy per sample | On-device health tests at 1 bit |
| --- | ---: | ---: | ---: | ---: | --- |
| Clock jitter: SysTick cycles between 32 kHz edges, low byte | 999,712 | 0.189 | 0.310 | **0.19** | passed |
| ADC internal supply channel (10), low byte of a raw conversion | 972,272 | 2.050 | 1.441 | **1.44** | passed |

**Clock jitter:**
- Three values make up 90 % of the jitter samples (0x6f 36 %, 0x39 36 %, 0x70 17 %).
- So the CPU clock and the 32 kHz timer are nearly locked together: the count varies mainly with interrupts.
- It is too weak to claim much.

**ADC:** spread over 92 values, the most common 22 %.

**Health tests:** both sources passed at a 1-bit claim. That shows the tests catch stuck or grossly broken sources, not an over-claim: the jitter source passed with a fifth of that.

**Choice:**
- The generator claims **1 bit per ADC sample** (8 eighths), below the measured 1.44. A 256-bit input then needs 512 samples, after the 1,024-sample start-up test.
- The jitter samples are hashed into the pool too, but are credited nothing.

**Not yet covered:**
- **One unit, one condition:** battery power and temperature were not tested.
- **Restart test:** 90B's restart test (samples across many cold boots) has not been run.
- **ADC sharing:** the ADC is shared with the runtime's power monitor, so wiring needs a way for both to use it.

## Storage sectors

All storage-region sectors are now named in one table, `petlibre_amebad_sys::storage`. A compile-time check keeps the ranges disjoint and inside the region, and the existing constants read from it.

| Sectors | Owner |
| --- | --- |
| 0–1 | Feed journal (FW15) |
| 2–3 | Settings (FW15) |
| 4 | Boot confirmation (FW11) |
| 5–6 | Wi-Fi (FW25) |
| 7–8 | RNG seed (FW45) |
| 9–10 | Identity and clients (FW45) |
| 11–12 | Plans (FP03, reserved) |
| 13–24 | 7-day event ring (FW47, reserved) |
| 25–31 | Free |

## Clients

`store::clients` keeps the feeder's X25519 identity secret and up to 8 enrolled clients, in one 584-byte record written whole to the other of two sectors.
- **Atomic:** an enrollment, revocation or reset lands entirely or not at all. The power-cut test confirms this at every flash mutation.
- **Client fields:** each client is its 32-byte static key, a label of up to 16 bytes, a role and the generation that enrolled it.
- **Roles:**
  - **Owner:** everything, including managing clients.
  - **Member:** feeding and settings.
  - **Server:** the optional server.
  - **Update signer:** an owner's Ed25519 key for FW48.
- **Reset:** FP15's reset forgets every client and takes a new identity.

**Known limit:** the identity secret and the RNG seed sit in plain flash. The board has no secure element, so anyone who reads the flash chip can impersonate the feeder until the next reset. The seed is less sensitive, since every boot mixes fresh input in.

## Pairing

`petlibre-feeder::pairing` decides who may become a client. It returns an `Admit` event, which the firmware applies to the client store. There are four ways in:

| Path | Who starts it | Confirmed by | Role |
| --- | --- | --- | --- |
| Setup | The app over BLE in setup mode | A Dispense press within 60 s | Owner |
| Signer | An owner over the API | A Dispense press within 60 s | Update signer |
| Invite | A paired client: an owner invites any role but a signer, a member invites members | The invite's secret as the PSK of a Noise XXpsk3 handshake, within 10 min, once | As invited |
| Recovery | Lock and Dispense held together for 5 s on an unlocked panel | A plain XX handshake within 2 min; the window then closes | Owner |

- **Dispense press:** a pending confirmation takes the next Dispense press even on a locked panel, since that press answers a pairing someone at the feeder started; it feeds nothing.
- **Recovery and the lock:** a locked panel cannot open recovery.
- **Invite lifetime:** at most two invites are outstanding, and they live in RAM, so a reboot cancels them.
- **Invite id:** the invite's id travels in the first handshake message's payload. That payload is not secret, and the responder needs it to choose the PSK before the third message.
- **Interop:** `noise.rs` gained XXpsk3, checked byte for byte against `snow` in both roles; a wrong or missing PSK fails closed.

The timeouts are ours: stock pairs through the vendor cloud and has no equivalent ([stock behavior](../devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)).

## Left for FW45

1. **ADC sharing and restart:** let the generator and the power monitor share the ADC; run the restart test across cold boots.
2. **Wiring:** connect the pool, the seed store and the DRBG at boot. Replace `entropy()` at the transport, BLE and FW44 budget call sites.
3. **Firmware stores:** open the client store in the firmware, generate the identity on first boot, and apply `Admit` events.
4. **BLE setup:** the BLE setup exchange (Wi-Fi in, the app's key in, the feeder's key out), with BLE only in a boot-chosen setup mode.
5. **Broker credentials:** stop treating them as a build input.
6. **Physical checks:** a cold boot keeps keys; the FP15 reset clears them.
