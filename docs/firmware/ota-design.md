# OTA updates

This page explains how the replacement firmware updates itself over the network, and how a bad update rolls back on its own. It is for owners who want to know why an update can't brick their feeder, and for contributors working on the updater. The loader's bank rules it builds on are in [bank policy](bank-policy.md).

## The one idea

A new build always goes into bank 1, and bank 0 always holds a known-good build that can feed. The new build's marker is written last, so a half-written bank 1 is never booted. The new build then runs on trial. If it crashes, hangs or never proves it can feed, KM0 gives way to bank 0 after three boots, and the feeder keeps working on the old build.

A few constraints shape everything below:

- **Bank 1 always wins when valid.** The factory loader keeps bank 1 whenever its signatures pass; bank 0 boots only when bank 1 fails.
- **No writing the running bank.** Code executes in place from the active bank, and rewriting it under execution is unsafe.
- **The loader never falls back on its own.** A valid-marker image that crashes is selected again on every reset. Both banks invalid means a wait loop that only UART recovers.
- **Writes go through the flash authority,** with outputs quiesced and KM0 parked for each lease. Nothing moves during an update.
- **The update is trusted, not the transport.** The manifest carries the image's SHA-256, version, size and target bank, and is signed. The firmware embeds only public keys. An eavesdropper learns only the image, which is not secret.

## Updates over the device API

Offers, downloads and key changes go over the [device API](device-api.md), behind `Capabilities::UPDATE`. Both banks run the `feeder` build, and both start the firmware, the settings writer and the update stage (`wellfed_hal::ota::update`) in the service task. The bank decides only what the update stage does with an offer:

| Bank | What it does with an offer |
| --- | --- |
| 1 | Checks it, then checks that bank 0 still holds exactly the image bank 0 recorded at its last boot (length from the bank's headers, SHA-256). It records the offer as pending in the update store and answers `GivingWay`. Once that answer is out, it clears its own marker's first byte and restarts. It never writes bank 1. |
| 0 | Records its own image at boot, only when it changed. Installs only the pending manifest, or the offered one when none is pending (fresh from stock, or after a fallback). Never confirms. |

Some refusals:

- Until bank 1's build confirms, its hooks are unarmed and every offer is refused `Busy`. A bank 1 on trial writes nothing.
- Bank 1 refuses (`Fallback`) an offer signed by a built-in key that bank 0's record does not name.
- Bank 0 refuses a manifest that was activated and failed its trial.
- Bank 0 refuses every offer after a reset mid-install, until power is lost.
- A build with battery mode refuses an offer (`Battery`) while the adapter reads absent: the feeder runs on its batteries and turns Wi-Fi off 5 s later ([power](power.md#battery-mode)). It also stops an install that is receiving when Wi-Fi goes off. Bank 0 does so only once it runs such a build: an older bank 0 takes the offer.

The bench and the simulator drive the same client steps (`wellfed_client::update`).

### One update, step by step

1. An owner's client offers a signed version 2 manifest to bank 1 (`OfferUpdate`).
2. Bank 1 gives way and restarts into bank 0.
3. The client reconnects; the same identity answers, now from bank 0. It offers the same manifest again.
4. Bank 0 answers `Receiving` and names the chunk offsets it wants.
5. The client pushes them (`UpdateChunk`, 512 bytes, idempotent by offset).
6. Bank 0 erases, programs and reads back each sector, then checks the whole image's SHA-256.
7. Bank 0 records the activation in the update store, then writes bank 1's marker last and reads it back.
8. Bank 0 answers `Activated` and restarts.
9. The new build boots on trial. Confirming raises the floor ([below](#keys-and-anti-rollback)) before the confirmation record is written.

The device API's client pushes each offer once, and never again on its own.

### The loop never verifies

The device API runs in the loop task, on an 8 KiB stack. An Ed25519 check needs about 8 KiB. So the loop copies requests and chunks into a mailbox (`wellfed_hal::ota::mailbox`) and answers from what the service task's update stage publishes. Nothing on the loop's side calls a signature check.

In the feeder build, the service task's stack bound is 11,504 of 16,384 bytes, its deepest path the install's start; the offer's `verify_strict` is shallower. The loop's is 5,016 of 8,192, a handshake.

### No flash write while a feed may move

Every update request (offer, cancel, key statement) takes the install lease as it is queued, in the loop that alone starts feeds.

- Each request is refused `Busy` while a feed is queued or driven.
- A feed is refused `Busy` while the lease is held: at the device API, in `Feeding::submit`, and in the HAL feeding owner's `submit`.
- The stage releases the lease once it answers, except for an accepted offer. The install keeps it until it stops; a give-way or an activation keeps it until the restart. A cancel during an install rides on the install's lease.

Every update-store save and bank-1 write also takes the write turn (`wellfed_hal::interlock::WriteTurn`), as the settings writer and the confirmation do, and never by the forced path:

- A request waits in the mailbox, unanswered, while the outputs are claimed. Bank 0's boot record of its image waits the same way.
- An install holds the turn from its first write until it stops, so every feed meanwhile is refused (`held=3`).
- A give-way or an activation keeps the turn until the restart, so no feed starts before the reset.
- The floor the confirmation raises is written under the confirmation's own turn.
- The attempt marker is set only once the turn is held, around each write, as the settings writer's are. So a hang inside one is skipped once after the reset.

The install and the settings writer share the service task, so their flash writes never overlap. The journal's writes come only with feeds.

### Bootstrap

Bank 1 gives way only to a bank 0 that recorded its own image (length, SHA-256 and built-in key id) in the update store. Bank 0 does that at its first boot.

So after a UART install of bank 0, bank 1 refuses every offer `Fallback` until bank 0 has booted once. The reviewed UART install boots bank 1, so the bank-0 record comes with the first give-way through UART (`give-way-execute --fallback`), or with a failed trial.

Both banks must carry the same built-in keys. Bank 1 checks the offer's key against bank 0's recorded key id before it gives way, so an offer bank 0 could not admit never strands the feeder on bank 0.

### Stops

Every failure before activation leaves bank 1's marker clear. The loader keeps booting bank 0, which keeps answering and feeding. The failures are:

- a flipped byte (caught by the whole-image hash);
- a chunk the install did not want, or of the wrong length (order);
- a failed program or readback (write);
- a marker that does not read back;
- a minute without a chunk (timeout);
- a cancel;
- an update store that cannot be written;
- a manifest or key the checks refuse.

A stop leaves the candidate recorded, so bank 0 refuses any other manifest (`Pending`) until a `CancelUpdate` forgets it (`wellfed-bench ota NAME --cancel`). A cancel answers `Busy` when the store cannot be written, rather than claiming it forgot.

A dropped connection loses nothing. The install waits a minute for the next chunk, and `UpdateState` says where it stands.

A power cut mid-download boots bank 0 again, which takes the offer again from the start.

A reset mid-install sets the update-attempt bit (backup register 1, bit 9, beside the flash attempt bit). Offers are then refused until power is lost. The hooks still arm, so bank 0 still saves and feeds.

**A failed write says why.** A sector write that fails stops the install with:

```
PLOTA1 error stage=write sector=<bank-1 sector> step=erase|program|read|readback offset=<byte> expected=<word> read=<word> reread=<word> erase_ms=…
```

On a readback mismatch, the record holds the first differing word as programmed, as read back, and as read a second time with no write in between. The second read first invalidates the data-cache lines over the word, so it reads the flash rather than the line the first read filled.

- A 0 bit where a 1 was programmed means the erase did not finish.
- A word that reads `ffffffff` means the program did not take.
- A second read that matches means the cells are right and the read path is at fault.

The install still stops; nothing is rewritten.

**Lease timing during an install.** Each `PLOTA1 progress` line is followed by `PLOTA1 leases count=… park_max=… release_max=… masked_max_ms=… masked_long=… feed_gap_max_ms=…`. Those are the leases so far, the slowest KM0 park and release exchanges (in 32 768 Hz ticks), the longest time IRQs were held masked (from the acquire's masking, request and quiesce included), how many leases held them past 100 ms, and the feeder task's longest gap between checkpoints on the system timer. A lease's acquire masks IRQs before it reads the lease clock, so a preemption between the request and the park can no longer count against the park budget and halt the board.

## Keys and anti-rollback

**Manifest version 2** (`wellfed_core::ota::offer`) is 96 bytes, then the Ed25519 signature:

- `PLOTA2\0\0`, the version, the image length, the target bank;
- the image kind (1 replacement, 2 stock) and three zero bytes;
- the signing key's id (the first 8 bytes of SHA-256 over its public key);
- the image's SHA-256 and the build ID.

Version 1 (`PLOTA1`) is deleted. An offer carrying its magic is refused as the wrong version. The bench still reads the versions of the version 1 manifests on disk when it picks the next.

**Trusted keys.**
- The key built into the firmware: the bench key, `WELLFED_OTA_PUBLIC_KEY`.
- The release slot, empty in the build and filled once by a `PLKEY1` statement a built-in key signed (`UpdateKeyStatement`). So a release key arrives without reinstalling bank 0.
- Owners' update keys, enrolled over the device API (`EnrollUpdateKey`) only once someone presses Dispense at the feeder, and removed like any client.

**Enrolling an owner's key.** `wellfed update-key new --output FILE` writes an owner's key. It is the bench key's file format, marked `"scope": "owner"`, and it refuses to replace a file. `wellfed update-key enroll --key FILE` enrolls a key on the feeder:
- Only an owner's session that claimed updates may enroll one. The CLI refuses a member before anyone is asked to press.
- If the feeder already lists the key as an update signer, nothing is sent.
- If the feeder holds the key as a client that opens sessions, the CLI refuses it. An update key is a key of its own.
- Otherwise the CLI sends `EnrollUpdateKey` and asks you to press Dispense within 60 s. Unlock the panel first if it is locked. No press in time is refused `Unconfirmed`, and nothing is enrolled.
- If no answer comes, the outcome is uncertain: a press the feeder took before the session ended still enrolls the key. The CLI says so and never sends the request again. Check with `wellfed update-key show`.
- `wellfed update-key show` lists the enrolled keys with their key ids. They also appear in `wellfed clients`.

The bench key enrolls the same way. A feeder whose image embeds no key then still takes the updates `wellfed-bench ota` signs. An owner's key signs manifests and downgrade statements, which the firmware checks against every trusted key. It never signs a key statement, and no image embeds it: `wellfed-build revoke`, `add-key` and the build itself refuse it. The client side is `wellfed_client::update_key`, which the installer will reuse right after it pairs.

A `PLREV1` statement retires any key id for good. Built-in keys and the release slot's key sign key statements, so retiring the bench key leaves rotation to the release key. An owner's key never signs one, and a retired key signs nothing.

The bench key stays in `private/ota-key.json`, for the bench only. An offline release key is required before any public release.

**The floor** (`min_version`) is the highest version a build confirmed at.
- An offer at or below it, and every stock image, needs a `PLDWN1` downgrade statement signed by a trusted key. The statement names this feeder's identity key, the exact manifest's digest and the current floor.
- A replayed old manifest, a statement for another feeder, or one made before the floor last rose is refused.
- Confirmation raises the floor after arming, and before it erases the confirmation sector. A store that cannot be written leaves the build unconfirmed.
- Sector 4's version field carries the version bank 0 activated. It is 0 for a build installed over UART, or after a torn confirmation rewrite (the store's candidate was already confirmed, so the retry finds nothing naming the build). Nothing decides on that field; the floor lives in the update store.

**The update store** (`wellfed_core::store::update`, storage sectors 25–26) is one 256-byte record, A/B, newest valid wins. It holds the floor, the candidate (pending, then activated), the release slot's key, up to four retired ids, and bank 0's recorded image.
- A record that would lower the floor or forget a retired id is never written.
- A torn write leaves the older, consistent record.
- It is not the confirmation sector, which is erased before each write.
- The factory reset leaves it alone.

**Signing** uses `wellfed-build sign` (version 2), `downgrade`, `revoke` and `add-key`, with the bench key. `sign --key` and `downgrade --key` also take an owner's key, for the feeders that enrolled it; `revoke` and `add-key` refuse one. `wellfed-build` signs only a frozen review.

## Storage-format rule

Storage formats stay readable by the bank-0 build, or a format change ships a bank-0 refresh first.

A store bank 0 cannot read (a newer schema) has no floor it can trust, so bank 0 refuses every offer. A format change that skipped the refresh would strand the unit on bank 0 until a UART install.

## Boot confirmation and fallback

KM0 runs the trial before it starts KM4 (`wellfed-hal-common/src/trial.rs`, `wellfed_trial_step` in `firstboot_km0.c`).

- **Running bank:** read from bank 1's physical marker.
- **Confirmation** is keyed by the build identity, not the image hash, since the running image does not know its own OTA hash. The record lives in storage sector 4 (`0x344000`). So reinstalling the build the record already names skips the trial: KM0 logs `PLTRL1 confirmed=1` straight after activation, and the build logs `PLCNF1 confirmed=1 already=1`. A test that needs a trial must install a build the record does not name.
- **Count:** backup register 4.

  | Bits | Holds |
  | --- | --- |
  | 0–15 | The peer boot count |
  | 16–19 | The trial count on a trial boot; with bit 19 set, the reset storm's backoff level on any other boot |
  | 20–23 | The reset-storm count ([below](#reset-storms)) |
  | 24–30 | An identity tag |
  | 31 | The next boot may forgive no brownout ([below](#reset-storms)) |

  The trial count never exceeds 3, so a register written by an older KM0, whose trial count took bits 16–23, reads the same.
- **Give way:** at boot 4, and only if bank 0's marker is valid, KM0 clears byte 0 of bank 1's marker and resets with its own watchdog.

The program step bypasses `FLASH_Write_Lock`. After a watchdog reset, that lock's KM0 branch waits forever for an absent KM4, because `km4_status_on` still reads the HS clock bit set.

**How a build confirms.** Once the startup probe has ended, the link is live, and outputs have been verified stopped for 30 s continuously, the build arms the hooks, writes the record to sector 4 and reads it back (the service task in `wellfed-sdk`'s `runtime.rs`). A live link means associated, leased, and the lease's gateway answering an ICMP echo, or never having answered this join ([link liveness](startup-diagnostics.md#link-liveness-without-internet-dns)). It needs no internet DNS, so a build confirms on a LAN with no route out. A gateway that goes silent after answering restarts the 30 s.

**Supervision from startup.** The watchdog opens with the application, before the startup diagnostic: the feeder task starts first and opens it on its first step (`PLSUP1 open watchdog=1 trial=N`). So a fault in early startup is a counted reset, not a hang. The decision lives in `wellfed-hal`'s `supervision` module:

- **Startup:** feed while the startup task keeps stepping.
- **A determined startup failure:** a trial image stops feeding, so the failure is a counted reset. A confirmed image keeps feeding and holds STOP, so there is no reset loop.
- **Running:** feed while every running task steps and outputs stay stopped or sensing.
- **Trial deadline:** a trial image that has not confirmed within 10 minutes of boot stops feeding.

Each stop logs `PLSUP1 stop …` once.

### What confirmation proves

A healthy network proves the update path still works, not that the build can feed. So a trial build of a firmware with a feeding owner confirms only once all of these hold. (`feeder` says it has one with `wellfed_sdk::claim_feeding` in its start hook.)

1. **The runtime's own health has held for 30 s:** the startup probes finished, outputs verified stopped, and the link live.
2. **The confirmation has armed the flash hooks.** The owner opens only on armed hooks, and on a trial boot this is the first arming, so the confirmation arms and then waits. Arming and the record's write each take the write turn, and set the attempt marker for themselves only, so the wait between them leaves the marker clear.
3. **The owner has proved itself** (`feeding::Proof`, reported each loop pass with `wellfed_sdk::report_feeding`). It has opened on its journal, resolved any feed a reset interrupted as uncertain, and stayed at rest for 2 s: Idle, no owner or supervisor fault, and motor STOP and the emitters off read back. That is about a hundred of its 20 ms samples, each of which reads the sensors and the motor current.
4. **The link is still live at that moment.**

Then the floor rises and the record is written. A firmware with no feeding owner (`blink`, `qa`, the bring-up areas) has nothing to prove, and confirms on step 1 alone.

**A build whose owner can't open never confirms.**
- If the owner fails determinedly (its journal cannot be opened, or it faults in setup, recovery or a check), the runtime stops feeding the watchdog at once on a trial boot, as for a failed startup diagnostic. The `PLSUP1 stop` line carries `feeding=3`. KM0 counts the reset, and after three gives way.
- If the owner just never gets there (arming skipped, a hang), the 10-minute trial deadline resets the boot.
- A confirmed build, or bank 0, whose owner fails holds STOP instead.

**Feeds wait for confirmation.** While a trial build is unconfirmed, the feeder refuses feeds (readiness `Blocked::Starting`), from clients and the Dispense button alike. So the confirmation never waits on a feed towards the deadline.
- The owner proving itself takes about 2 s after arming, so a healthy trial confirms a few seconds later than it would on health alone.
- The window runs from the activation's reset to the confirmation: typically 2–3 minutes (the boot, the Wi-Fi join, 30 s of health).
- If the network is down, a trial cannot confirm. Each trial boot runs to its 10-minute deadline, and after the third (about 30 minutes) KM0 gives way to bank 0, which feeds.
- **For schedules:** a meal whose time falls in this window is refused like any feed, so the schedule must not drop it silently. It should retry once readiness turns `Ready`, if that is still within a grace period after the meal's time, and otherwise record it as missed. It must never feed it twice; the feed's write id stays the same across the retry.

**A journal that stays unreadable** (corrupt in flash, not just a bad build) makes every trial fall back, since the owner fails to open on every trial boot. Bank 0 then runs with `Blocked::Journal`. It never confirms, so a failed owner holds STOP there rather than resetting. No update can confirm until the journal is repaired or recreated.

The `owner-fail` variant of `feeder` exercises this. Its owner never opens (the journal reads as failed and is left untouched), so each trial boot stops at once and the third gives way to bank 0.

### Reset storms

Confirmation proves one boot. A confirmed build can still fault into a reset soon after every boot, and KM0 runs no trial for a confirmed build. So KM0 also counts, in bits 20–23 of backup register 4, every boot that runs no trial: a confirmed bank 1, bank 0, or a trial that ran out with no bank 0 to fall back to. KM4 gives the count back:

- once a boot that is not held has run 10 minutes (`trial::STABLE_MS`, the trial's deadline; `PLSUP1 storm settled=1`);
- before every restart it asks for (a reboot over the device API, an activation, a give-way, setup), since those are not faults.

**Brownouts are forgiven, within a bound.** A brownout is the supply, not the build. A pack weak enough to brown out at idle would otherwise make a storm within minutes, and give a good bank 1 away to an older build.

- On a boot whose `BOOT_Reason` carries the brownout bit (bit 18), KM0 removes the previous boot's pending count before deciding (`trial::decide_after`; `PLTRL1 brownout=1 reg=…`).
- The brownout neither counts nor ends a burst, so a fault's resets still reach the limit with brownouts between them.
- A trial's count (the low nibble) is never touched. A brownout on trial still counts toward its three boots, so no trial is shortened or skipped.
- **The bit may be stale.** The loader that backs the ROM's flags up into `BOOT_Reason` returns early when it finds none set, leaving the previous value. So after one real brownout, a fault reset that sets no flag can read as a brownout too.
- So a boot that forgave sets bit 31 (`trial::FORGIVEN`), and the next boot forgives nothing. A halt's own reset sets it too, so its held count stands.
- Of any two early resets in a row, at least one counts. A fault reaches the storm limit within 10 early resets, and the halt within 16, however the bit reads. The breaker's bounds hold, at worst twice as slow.
- Whether a real brownout on this board sets the bit is unverified ([power](power.md)).

So the count KM0 finds at boot is the number of boots in a row that ended in a reset nobody asked for, within 10 minutes of starting. Every boot reports it: `PLTRL1 confirmed=1 resets=N` or `PLTRL1 bank=0 resets=N`, and KM4's `PLSUP1 storm resets=N level=L mode=…` when either is not zero.

**At 5 resets** (`trial::STORM_LIMIT`):

- **A confirmed bank 1 with a valid bank 0 gives way,** exactly as a failed trial does (`PLTRL1 storm=5 limit=5`, then `PLTRL1 give_way=1 …`). Bank 0 is the full feeder, and a build that faults this often is the likeliest cause. It was confirmed, so the update floor stays at its version, and a fixed build must carry a higher one. The give-way leaves no count for bank 0.
- **Otherwise** (bank 0 itself, or a bank 1 with no valid bank 0), KM4 boots **held** (`PLTRL1 storm=N held=1`, `PLSUP1 storm resets=N level=L mode=held`).
  - The feeding owner stays closed, so the motor and emitters stay as startup left them, stopped. Readiness reports `Blocked::Fault`.
  - The device API, the settings writer and the update stage run, so the owner can read why (the log tail carries the lines), install a fix or reboot.
  - **A hold is time-bounded.** A held boot that runs 10 minutes × 2^level (10, 20, 40, 80, at most 160 minutes; `trial::hold_ms`) opens the owner and feeds again in place (`PLSUP1 storm released=1 level=L`), with the count cleared and the level one up. A fault that keeps coming back holds twice as long each time. One that has cleared (a run of association hangs, a battery brownout) costs at most one hold.
  - Every stable boot takes a level back: a normal boot after 10 minutes, a released boot once it has fed for its new level's hold.

**At 8 resets** (`HALT_LIMIT`), when held boots keep resetting too (a fault in the network stack, say), KM4 starts nothing but a timer for an hour (`trial::HALT_MS`; `PLSUP1 storm resets=8 … mode=halted`). No network, no owner, and outputs as `stopped::initialize` left them. Then it leaves KM0 a held count a level up and lets the watchdog reset the board (`PLSUP1 storm halt_over=1 reset=1`), so the next boot is held again, not halted. The `held-stall` variant reaches it on the bench: a priority-3 task starves the service task about a minute into every boot, held boots included, which `owner-stall` cannot do (a held boot owes no owner beat).

**A trial that runs out with no bank 0** (`PLTRL1 error fallback=invalid boot=3`) runs on as a build in no trial: held at once, like a storm, then released, with no trial deadline (`trial::past_trial`). Its resets count as a storm's. It can still confirm itself later if its owner proves itself. Its state is honoured only while bank 0 stays invalid. Once bank 0 is valid again, it is back on trial, counted from 0, and gives way after three boots. The same rule keeps a count another build left under the same tag (a reinstall of the same identity, or one identity byte in 128) from ever letting a new build skip its trial. An activation or give-way restart also clears the register's counts outright.

**Nothing is permanent.** A build that faults at every boot resets at most 14 times in its first burst (6 on bank 1, then 8 on bank 0). After that, only the held boots' few resets around each hour-long halt: a handful an hour.
- Every count is in a backup register, so there is no flash record to tear.
- The registers survive a watchdog reset and a brownout. They clear only on **power loss, which means the adapter and the D cells both out.** With batteries in, pulling the adapter is no power loss.
- One case the count does not see: a build that crashes just after 10 minutes on every boot settles each boot first. It resets about every 11 minutes indefinitely, feeding between.

The simulator models the breaker: `wellfed-feeder-sim`'s banks run the firmware's own `trial::decide`. On hardware, the `owner-stall` variant is a storm: once confirmed, it resets about 3 minutes into every boot, so KM0 gives way at the sixth boot after the one that confirmed.

### Recovering a held or halted feeder

What you see, and what to do:

- **Held.** The app and server reach the feeder, but every feed is refused with a fault (`Blocked::Fault`), and the log tail shows `PLSUP1 storm … mode=held`. It feeds again by itself once the hold ends: 10 minutes the first time, doubling for each storm in a row, at most 160 minutes. To retry at once, reboot it from the app; a requested reboot clears the count. If it keeps coming back, install a fixed build. Updates install as usual while held.
- **Halted.** The feeder does not answer at all for up to an hour, then comes back held. Waiting is enough. To start over at once, cut power fully: unplug the adapter **and** take out the D cells for a few seconds.
- **Bank 1 gave way.** The feeder answers as its bank-0 build (the update view shows bank 0) and feeds. Install a build with a higher version than the one that stormed.

### The attempt marker and decided resets

Every store write sets the flash attempt marker for its duration. A boot that finds it set skips arming once per power cycle, so a write that hangs cannot reset the board over and over.

A reset the runtime decides never lands in a write it let start. Without that rule, a supervision reset could land inside a store write that started in the seconds before it, leaving the marker set. The next boot would skip arming, and the feeding owner would not open until a power cycle.

- From the first feeder pass that does not feed the watchdog, transient or certain, `supervision::writes_held` holds every new write. No write turn is granted, no arming starts, and an install (which holds its turn across sectors) goes no further.
- A transient stop holds writes for under a second; a certain one until the reset.
- A write already running when the feeding stops has the rest of the watchdog's expiry, at least 2.4 s less a feed period, and takes tens of milliseconds.

So the marker stays set only after a reset the runtime did not decide (a hang, a fault, a lease that halted), which is what it is for. A withheld reset does not clear it. The storm breaker bounds the rest.

## Resets that tell you why

Some resets used to leave no line on the UART. Each now leaves a breadcrumb for the next boot.

- **A failed flash lease.** The hook halts in RAM with IRQs masked, so nothing reaches the UART before the watchdog resets the board. The provider writes the failure into the attempt register's top bits (bits 31–10; the attempt and update bits keep 9–0), and backup registers survive a watchdog reset. The next boot reports it before anything arms, then clears those bits:

  ```
  PLSUP1 lease_fault stage=<LeaseStage> phase=<FlashPhase> fault=<FlashFault> reply=<FlashReply> elapsed_ms=<…>
  ```

  `elapsed_ms` counts from this lease's request, in 16 ms steps; 1008 means 1008 or more, and 0 means a request-stage failure. The skip rule still reads only the attempt bits.
- **A refused watchdog checkpoint.** A gap over the 1 s progress limit, an altered register or a clock fault latches, and the hardware is never refreshed again. The feeder logs `PLSUP1 stop watchdog_fault=<WatchdogFault> gap_ms=… fed_ms=… ms=…`. It shares one latch with `PLSUP1 stop startup=…`, so a boot logs whichever stop comes first, once.
- **A hang that stops the feeder task itself** (a spin above its priority, or interrupts masked). The tick hook leaves the interrupted task, and the time since the feeder last ran, in backup register 2. The next boot after a watchdog reset prints `PLSUP1 hang kind=spin|masked|stopped task=… since_ms=… suspended=…` ([silent watchdog resets](startup-diagnostics.md#silent-watchdog-resets)).

**A restart leaves the network first.** Every intentional restart (activation, give-way, a requested reboot) asks the Wi-Fi task to disconnect while the watchdog runs out. The access point then drops the association, instead of rejecting the trial boot's join with status 30. After such a rejection, the vendor's `wifi_connect` can hang the board into a silent reset and spend a trial boot. The feeding stops at once regardless ([leaving before a restart](startup-diagnostics.md#leaving-before-a-restart)).

## Bank-0 download faults

Bank 0 used to fault now and then while downloading an update: hard faults at random places, imprecise bus errors, and once a single-bit read error caught by readback. Lowering the flash clock from 80 to 50 MHz stopped them. The working explanation is that the board's flash read margin at 80 MHz was too thin under download load, so the KM4 sometimes misread its flash. That is not yet established.

The builder sets KM0's `Flash_Speed` to 50 MHz. The read mode stays Dual IO, since the Quad check fails at the slow boot clock.

## How it's tested

- **Core.** `wellfed-core::ota` uses `ed25519-dalek` and `sha2` with default features off, so it stays `no_std`. It holds manifest parsing and signature verification, the bank-1 sector writer, the trial decision and the confirmation record.
  - The manifest check refuses tampering, the wrong key, a bank other than 1, a size outside one sector to 0x150000, and downgrades.
  - The writer withholds the marker until every sector reads back and the whole-image SHA-256 matches. It refuses out-of-order chunks, a missing marker, failed readback, corruption and truncation.
  - Host tests include RFC 8032 test 1.
- **Stops.** `wellfed-hal/tests/update.rs` runs every stop against the fake SDK. `wellfed-client/tests/update.rs` and `wellfed-feeder-sim/tests/update.rs` run the client end to end, over the fake and over TCP against the simulator. The simulator runs the real stage on two banks, with the loader and KM0's trial.
- **Emulation.** `every_ota_step_and_marker_cut_keeps_a_bootable_bank` in `src/tools/wellfed-emu/tests/factory.rs` hands each modeled flash state to the factory loader's own selection code. The states are every update step, a bank-1 marker cut after each of its 8 bytes, and erased or staged bodies. It passes all 14 states for both stock and the replacement build. Only the forbidden step, giving way with no valid build in bank 0, leaves nothing to boot. Any single cleared bit in bank 1's marker gives way to bank 0.
- **Signing.** `wellfed-build keygen` and `sign`; a test checks a signed manifest against the firmware's own offer check.

## On the bench

`wellfed-bench ota` and `itest` push updates over the device API ([bench](../devices/plaf103/bench.md)). No motor moves.

- In the push model, a truncated download is a stall: the bench simply stops sending, and the install times out (`stage=timeout`).
- Between failure drills, `ota NAME --cancel` forgets the candidate a stop left recorded.
- The hands-off round trip offers bank 0's build back from bank 1, so the build in bank 1 must have another build ID. R1 is the feeder's `twin` variant (`--variant twin`), R0 the plain feeder.
- A bank-0 build is first installed by a reviewed UART install (`flash-prepare/flash-execute --bank 0`). Bank 1 keeps running until a give-way.
- Bring-up firmwares (`qa`, `optics` and the rest) serve no device API, so a unit left on one in bank 1 takes no offer. Leaving it needs a UART install or a failed trial. After installing one, the bench has only the UART confirmation (`wellfed-bench ota` says so).
- If a new build's own KM0 code is broken, nothing counts its boots, and UART recovery is needed. That stays the backstop, and the relay bench can do it unattended.

## Evidence

- **Observed** (October 27, 2026): both banks run the same `feeder` build at a 50 MHz flash clock. Bank 0 was written over UART and booted by a give-way that recorded its image. Then 8 of 8 installs, alternating a `twin` and the baseline through bank 0's download, passed with no fault, each a full download and a confirmed trial (`otaloop-20261027-n01`…`n08`). That is one evening's runs, not yet the sustained run.
- **Observed** (October 27), the faults before the change: at 80 MHz, bank 0's downloads faulted in 8 of 10 installs in one series (`otaloop-20261027-d01`…`d10`) and 6 of 8 after a full power-off (`g01`…`g08`). The dumps differed each time: a SecureFault on an instruction fetch in `FlashCoord::request`; a SecureFault in the SysTick handler through a corrupted ready-list pointer; an imprecise BusFault on `wellfed-loop`; a fault in `__aeabi_memclr4`. One install failed `stage=write` on a transient single-bit read error: `sector=237 step=readback offset=604 expected=bee707f1 read=bee707f5 reread=bee707f1`. At 50 MHz the boot's flash calibration window reads 29–31 (`calibration_ok:[2:31:17]`, 15 boots), against 11–17 at 80 MHz. Flash wear is ruled out by the feeder's own erase counts.
- **Observed** (October 24), earlier bank-0 faults: 3 crashes and 1 silent watchdog hang in 157 bank-0 boots that started Wi-Fi, and none in 199 bank-1 boots, including the same build. The crashes were in `cmd_thread` (`fw47-itest-05`), and twice in `rtw_recv` (a backpower run, then `warm-20261024-05`). The last, symbolized with its ELF, was in the SDK's `report_survey_event` under `OnProbeRsp`, 11 s after boot, before any download. Static analysis: two of the faulting PCs hold valid instructions, so the CPU fetched wrong bytes from flash. Ruled out then: a power cut, UART back power (relays 3 and 4 now isolate it), the old `qa` image, and our flash leases, which held IRQs masked for 41–44 ms at most (`masked_max_ms`), none past 100 ms. The Wi-Fi driver takes no leases (17 per written sector, exactly), and both cores hold `PMU_OS`, so nothing sleeps.
- **Observed** (October 26–27), what confirmation proves: an `owner-fail` build stopped each of three trial boots with `owner=0`, and KM0 gave way to bank 0. An `owner-stall` build confirmed, then reset about 190 s into every boot (`hang kind=stopped`); after `storm resets=1…4`, KM0 logged `storm=5 limit=5 give_way=1` and bank 0 started.
- **Observed** (October 27), a held boot: the `owner-stall` build written to bank 0 and given way to reset five boots in a row (`PLTRL1 bank=0 resets=1…4`). The sixth booted `PLTRL1 storm=5 held=1`, readiness `blocked reason=fault`, and the device API answered from bank 0. The held boot ran its stalls without a reset, and 10 minutes in logged `PLSUP1 storm released=1 level=0`, opened the owner on its journal and went `ready`.
- **Not yet run on hardware:** the halted state, which needs a variant that resets while held; a real brownout setting the forgiveness bit; enrolling an owner's update key (`wellfed update-key enroll`) and installing an update it signed.
- **Simulated** (October 29, 2026), enrolling an owner's key: in `wellfed-feeder-sim`'s update tests, a Dispense press enrolled the key and a feeder that embeds no key then installed an update it signed. A key nobody enrolled was refused `Key`, no press was refused `Unconfirmed` with nothing enrolled, and a member was refused `Forbidden`. A session cut after the press left the key enrolled, and the client reported the outcome uncertain.
- **Modeled:** the update stage, the device-API flow, the breaker and the decided-reset rule are host-tested and simulated, as listed under [How it's tested](#how-its-tested).
- **Observed** (October 22), the trial: the `stall` test variant (`wellfed-build build --profile firstboot --firmware bringup --variant stall`) was counted 1, 2, 3, then gave way and booted bank 0. Over the air, each of these was counted 1, 2, 3 and then gave way: a failed-start variant (`--variant fail`), stopped 253 ms into each boot; the hang variant (`--variant stall`); and a normal build whose server was stopped after activation, which reset at `ms=600192` on each boot. A fixed build then confirmed after 30 s healthy (`PLCNF1 confirmed=1 healthy_ms=30000`), and the next cold boot reported `PLTRL1 confirmed=1`. Reinstalling a confirmed build skipped its trial, as designed.
- **Observed** (October 22): the `FLASH_Write_Lock` hang after a watchdog reset, then its fix.
- **Observed** (October 24), stops on hardware: a flipped bit, a short range, a stalled server, a restarted server and a power cut mid-download each stopped before activation, or resumed and activated, with bank 1's marker never written early. A power cut mid-download made bank 0 start over, and a cut early in a trial counted that boot again before the build confirmed (`qa-20261024-03`, `-04`). The earlier transport these ran over is retired.
