# Device API

This page explains how the feeder serves its encrypted API: the requests, the event history and the live samples every client uses. It is for contributors working on the firmware or a client, and for owners who want to know what happens when they tap Feed. The wire contract is summarized under [The protocol](#the-protocol); the message types themselves are in [`wellfed-proto`](../../src/product/wellfed-proto/src/lib.rs). How the optional server uses the API is in [the server docs](../server/README.md).

## The one idea

The feeder serves the API itself, from its one loop task, with Noise encryption on every session. There is no cloud and no plaintext mode. The CLI, the app and the server all speak the same protocol, and every request lands in the same loop as the Dispense button. That loop also steps the [feeding owner](../glossary.md), so a feed from anywhere reaches the one piece of code that moves the motor.

The server inside the firmware is sans-IO: `wellfed_device_api::Server` never touches a socket. The firmware's driver copies received bytes in and sends what a session slot holds. That lets host tests drive the real server with `wellfed-client` directly.

## The protocol

### Sessions

- **Transport.** TCP port 4747, advertised over mDNS as `_wellfed._tcp` with TXT `v=1` and `id=` the identity key in hex, so a client matches the feeder it paired with. Discovery carries no trust; the handshake does.
- **Frames.** Every frame is a 2-byte little-endian length and its bytes, at most 1 KiB. After the handshake, a frame is one Noise transport message sealing one postcard-encoded message.
- **Opening.** A connection's first frame is a clear `Open`: `Session`, `Invite { id }` or `Recovery`. It only chooses the handshake; tampering with it makes the handshake fail.
- **Why Noise IK.** An enrolled client already knows the feeder's key, and sends its own encrypted in the first message. So the feeder learns which client is connecting without trying each enrolled key. KK would need up to 8 DH operations per first message, over the loop's 80 ms budget. IK is one round trip, mutually authenticated, with no identity in the clear.
- **Other handshakes.** An invite is redeemed with **XXpsk3** and the invite's secret. The recovery window runs the setup exchange (XX and the code proof) over the stream. Both sides run `wellfed-session::noise`, checked byte for byte against `snow`.
- **Hello.** IK's first payload is a `Hello` (`protocol_major`, capabilities). The second is a `Welcome` (major, capabilities, build identity, the client's role, the event history's epoch), or a typed refusal: unknown key, unsupported version, or no free slot. So the app can say "update the app" or "update the feeder". The event cursor travels in `Subscribe`, not the hello, and the client sends it only when it counts in the welcome's epoch.
- **Keys.** A client's identity is its static X25519 key. There are no separate request keys: the session is the security.

### Messages

All messages are serde enums in `wellfed-proto`.

```text
Client → feeder                       Feeder → client
Request { id, op }                    Response { id, result }
Subscribe { after, samples }          Event { seq, at, body }
                                      Gap { from, to }   // events lost to the ring
                                      Sample { at_ms, metric }
                                      Log { seq, dropped, text }   // after TailLog
```

- **`id`** is a client-generated 128-bit request ID. The feeder remembers the last 32 write IDs per client key, across reconnects, with their compact outcomes, so a retried request returns the first answer rather than running again. Reads are not remembered; they simply run again. An ID that has left the ring is no longer recognized, which is safe for everything but feeds, whose IDs are handled [below](#feeds).
- **Results** are typed enums of outcomes and refusals. There is no free-text error.
- **Subscribe** asks for every event after a cursor (`None`: the oldest kept), then each new one, and the live samples if asked.
- **Events** are the durable records described in [data](product-architecture.md#data). `seq` is monotonic across reboots within the ring's epoch. A ring re-created by a storage wipe or the [factory reset](pairing.md#factory-reset) draws a new epoch and counts from 1 again.

**Ops of major version 1:**

| Op | Notes |
| --- | --- |
| `Status` | Behind `READINESS`, answered `FeederStatus { status, readiness, supply }` (`supply` only with `POWER`) |
| `GetSettings`, `PutSettings { base, changes }` | A stale `base` is refused `Stale { current }` |
| `PutWifi { base, wifi }` | `None` clears |
| `SetLock`, `Play`, `TimeSync` | An owner's `TimeSync` may step the clock; anyone else's may only narrow it, and is refused `Time` when it disagrees with a valid clock beyond both error bounds |
| `CreateInvite { role }`, `ListClients`, `RevokeClient { key }` | |
| `Feed { portions }` | 1 to 48 portions. Answered `Feeding { feed }` once the owner has reserved it; the result is a `Fed` event. Any other number is refused `Portions { asked }` |
| `CancelFeed { feed }` | `Done` while that feed runs, `NotFound` once it has ended |
| `Reboot` | |

**Ops behind other capability bits:**

| Bit | Ops |
| --- | --- |
| `FEED` | The feed ops, the `Feeding` answer, the `Fed`, `DispenseRefused` and `Food` events and the `Fault` and `Portions` refusals. A feeder answers a client whose hello lacks the bit `Unavailable`, and never pushes it a `Fed` event |
| `TIME` | `GetTime`, answered `Time(TimeView)`: the zone's rule, the NTP server, the clock with its error bound (or `None` while the feeder needs the time), the learned rate, the last source and the last NTP result. `PutTime { base, changes }`, owners only ([clock](clock.md)) |
| `UPDATE` | `OfferUpdate`, `UpdateChunk`, `UpdateState`, `CancelUpdate`, `EnrollUpdateKey`, `UpdateKeyStatement` |
| `STATS` | `Stats { section }`, and four more metrics |
| `TRACES` with `STATS` | `FeedTrace { feed, offset }`, answered `FeedTrace(TracePage)` |
| `BACKUP` | `ReadBank1 { offset }`, answered `Bank1(Bank1Page)`; `ReadStockPlans`, answered `StockPlans(StockPlanSectors)` |
| `LOG` | `TailLog { after }`, answered with `Log` batches |
| `CALIBRATION` | `GetCalibration`, `PutCalibration` |
| `POWER` | The `Supply` metric and event, the supply in `FeederStatus`, and the `Battery` readiness reason |
| `PLANS` | `GetPlans`, answered `Plans { generation, writable, plans }`, and `PutPlans { base, plans }`, which replaces all ten slots. The `Plans`, `MealSoon` and `Missed` events, and the `Schedule` source of a plan's `Fed` event ([schedules](schedules.md)) |
| `DESICCANT` | `ResetDesiccant`, which records a desiccant replacement today by the feeder's clock, and the `NoClock` refusal it gets while the clock is invalid ([desiccant reminder](desiccant.md)) |

`Fed` carries the feed's number, the portions asked for (`requested`) and those delivered (`completed`, one per rotor mark the feedback sensor confirmed), an outcome and a reason. The outlet beam's breaks are a lower bound on food passing, not a count, so they stay in the log and the statistics ([calibration](calibration.md#delivered-food-estimate)). The outcome is `Done`, `Cancelled`, `Expired`, `Failed`, or `Uncertain`: the owner stopped without a durable result, or a reset interrupted the feed and the next boot resolved it.

A failed feed's reason names what ended it: `Jammed` (a jam three reversals did not free, as stock's code 2031, or one that held while backing off), `Overcurrent` (one reading over the ceiling, or the limit held while the motor is stopped), `OutletBlocked` (the outlet beam blocked: mid-feed the feed ends with the portions delivered so far and is never resumed, where stock pauses up to about 10 minutes; at the start it ends with no motion), `NoFeedback` (no rotor mark), `MarkStuck` (the mark sensor stuck at a mark), `Sensor` (the ADC or the beams could not be read) or `Power`. None is retried. `Status.feed_fault` names the last feed this boot that ended `Failed` or `Uncertain` (number, outcome, reason) until a later feed completes ([jams and sensor faults](jams-and-sensor-faults.md)).

`Fed` also carries `hopper`, what the food beam read during the feed: `Empty` once it read no food, `Present` if it read only food, `Unread` if the feed ended before the beam settled (or a reset interrupted it). A feed runs whatever the beam reads, as stock's does, so a beam that misreads never skips a meal; an empty hopper is recorded, not refused. The beam is filtered as stock filters its `surplusGrain`: food shows at once, and empty only after a second of readings without. Stock counts 20 readings; its poll rate is not recovered, so the second assumes the nominal 50 ms its outlet filter implies. It is lit only during feeds, so `Status.food` is the last feed's reading (`Unread` before the first feed of a boot), and a `Food { level }` event records each change once the feed that read it has ended: at most one a feed, the first reading of a boot included.

**Samples** `Power` (every supply sample, 5 s) and `Rssi` (30 s) are major version 1's. `Heap`, `Loop`, `Gateway` and `Wifi` (30 s) go only to a subscriber whose hello claims `STATS`, and `Supply` only to one that claims `POWER`.

How the protocol may change is under [Evolution](#evolution).

## In the firmware

### The loop and its sessions

- **Two tasks.** The feeder runs the button service and the loop ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)). `wellfed-fw-feeder`'s `api` module steps the API inside the loop, after the panel's owner.
- **Nothing blocks.** Every socket call is non-blocking (`wellfed_api_*` in `example_entry.c`): the loop takes what is ready and moves on.
- **Four session slots.** A fifth connection is closed at accept.
- **One frame per pass.** A slot reads one frame per pass, and writes again only once its last frame is out. So a flooding client costs only its own turn.
- **Sockets:** one listener, four sessions, the mDNS socket and the NTP socket.

### Handshakes, and handshakes during a feed

The loop runs one handshake operation per pass, across all slots. A Noise operation costs one or two X25519s, 25–28 ms each on the KM4 ([resource budget](resource-budget.md#handshakes-and-loop-latency)).

**Sessions are Noise IK** under the feeder's identity. The client's key travels encrypted in the first message, and the feeder checks it against the enrolled clients the settings writer publishes (`wellfed_feeder_runtime::keys::publish_clients`). When the table changes, every session whose key is gone closes.

**During a feed, only IK session handshakes go on.** That holds from the loop's claim on the outputs to its release. So an enrolled client can open a session during a feed, read the status and cancel it. Strangers are bounded:

- Whether a key is enrolled is known only once the client's first message is read, so any LAN host can start a handshake. Building it costs one X25519, and reading that message two.
- A session refused during a feed (unknown key, wrong major, full) is closed without its reply, which saves that reply's two X25519s.
- After one session handshake is refused or fails during a feed, every other waits for the feed's end. An enrolled client opening then may time out and retry after it.
- So a stranger costs at most 3 X25519s per connection. Four strangers filling every slot at once each get their set-up step too: up to 6 X25519s over 5 operations a feed, still at most 2 an operation.

A client that may want to cancel keeps its session open across the feed, as `wellfed feed` does, rather than connecting to cancel.

**Why that is safe for the feed.** While it drives, the owner's observations must stay within 100 ms of each other (`max_observation_gap_ms`, and the feedback edges' 100 ms age). The loop steps the owner just before each pass. When the pass ran a handshake operation during a feed, it steps the owner again at once after it (`Server::handshook`). So the gap is the pass itself, not the pass plus the loop's delay. Overrunning the budget costs a failed feed (`ObservationGap`, stopped and durable), never a stop: stops stay with the 200 ms motor lease and the supervision timer. `PLOWN1 steps id=… max_gap_ms=… handshakes=…` reports each feed's longest gap on hardware.

**What still waits for the feed.** An invite's join and a LAN recovery both end in an enrollment, a store write that would wait for the feed. So the invite is redeemed, and the recovery code taken, only once the key can be written. A handshake held off this way waits; it is not refused. An invite or recovery window can still expire meanwhile. The hold lasts at most the claim's `FEED_BOUND_MS`, as a join's wait does.

### Feeds

**A `Feed` goes to the feeding owner** in the same loop (`feeding::Feeding`), under the next journal id. The answer waits until the owner has reserved the feed, then names it. When the owner reports the result, the loop logs `PLOWN1 fed …` with what the drive measured, records a `Fed` event and acknowledges the result, so the owner takes the next.

**Refusals.** Like every refusal, these are final for that write id. Nothing moved, so trying again is a new feed the user asks for.
- Every feed asks for 1 to 48 portions (`PORTIONS_MAX`, the vendor app's cap on this model), whoever asks: a client, a plan or Dispense. Any other number is refused `Portions { asked }` before anything else is checked, so the refusal says what to change rather than when to try again. Stock takes any byte unchecked.
- One feed at a time: a second is refused `Busy`.
- A faulted owner refuses `Fault`.
- A feed is refused `Busy` while a Wi-Fi join attempt runs (from its scan through association to DHCP) or waits for the outputs. An association holds every task for up to 0.72 s ([resource budget](resource-budget.md#scheduling-gaps-across-wi-fi-rejoins)), so feeds and joins take turns (`wellfed_hal::interlock`).
- The loop logs `PLOWN1 feed portions=… held=…`: 1 a join runs, 2 one waits, 3 a storage write ran past the claim's wait.

**A join during a feed** waits in the Wi-Fi task until the owner is at rest with its outputs stopped. It waits at most `FEED_BOUND_MS`: the owner's own cap on a feed's emitters-on time plus 2 s, about 9 min 40 s for 48 portions. `PLOWN1 interlock …` counts waits, and joins forced past that bound.

**A storage write running** when a feed comes makes the feed wait in the loop, at most `WRITE_WAIT_MS`. That is 3 s: one record's erase and two programs at their whole lease budget, after the writer's 200 ms poll. A write still running then, or a client's change still queued for the writer, refuses the feed `Busy` (`held=3`).

**A feed a reset interrupted** (power cut, watchdog, fault) is resolved when the owner next opens.
- The journal records it as uncertain under its own id.
- The loop records its `Fed` event with outcome `Uncertain`, `completed` 0, reason `Output` and source `Feeder`. A distinct reason would be a later wire change, and the requester is not journaled.
- The owner then takes feeds again, from the next id.
- It is never retried, and its id is never handed out again. A `CancelFeed` for it answers `NotFound`, and its `Fed` event in the history says how it ended.
- Until that event is recorded, a new feed is refused `Busy` (readiness `Resolving`).
- A power cut at any point of the resolution leaves it to resolve at the next boot, once. Only flash corruption of a committed result beside its own reservation (a bit flip that fails the result's checksum) makes the next boot resolve that feed again. Its id can then carry a second, uncertain `Fed` event after its first.

**A feed's write id is remembered from the moment it is queued,** not when its answer is sent.
- A client whose session ended before the owner reserved the feed, and that sends the same id on a new session, waits for that same feed's reservation and gets its `Feeding { feed }`. After the feed it gets the same answer. Nothing is queued twice.
- The window is the boot. Ids live in the server's RAM (32 per client key, 4 keys). The journal keeps the feed's number but not the request id, so after a restart a resent id is a new request.
- Clients never resend a feed whose answer they did not get: an unanswered feed is uncertain, and `Connection` reports it `Unanswered`. An interrupted feed resolves at boot as uncertain under its own number. So nothing resends across a reboot.
- Making the id durable would change the journal's record, and need a bank-0 refresh first.

**Readiness** (`Capabilities::READINESS`) is one decision that says whether a feed can start now (`wellfed_feeder::feeder::feeding::readiness`).

| Reason | Means | Feed refused as |
| --- | --- | --- |
| `Starting` | The owner hasn't opened, or a trial build hasn't confirmed | `Busy` |
| `Journal` | The journal failed | `Fault` |
| `Fault` | The owner faulted | `Fault` |
| `Resolving` | An interrupted feed is being recorded | `Busy` |
| `Feeding` | A feed is in flight | `Busy` |
| `Joining` | A Wi-Fi join runs or waits | `Busy` |
| `Battery` | On the pack alone below its floor, or after a brownout until the adapter returns or the pack has read charged for 3 minutes | `Busy` |
| `Setup` | BLE setup mode, which ends in a restart that would leave a feed uncertain | `Busy` |
| `Updating` | An install | `Busy` |
| `Unserved` | No owner at all (the simulator, the `gap-probe` image) | `Unavailable` |

- It starts from the owner's own reason (`Feeding::blocked`), then adds `Setup` and `Updating`.
- A session that claims the bit gets `Status` answered as `FeederStatus { status, readiness, supply }` (the model's `Status.readiness`). `supply` is the judged supply with the pack's charge, for a session that also claims `POWER`, and `None` otherwise.
- A `Feed` is asked of the owner only while it is `Ready`. The owner's own checks stand behind that, for a feed that lost a race.
- A change of a lasting reason (`Journal`, `Fault`, `Updating`, `Setup`) is recorded as a `FeedingBlocked { reason }` event, and `FeedingBlocked { reason: None }` once it clears. Passing reasons (a feed, a join) only go to the log (`PLFEED1 readiness …`), so they never fill the history. Each boot starts with none recorded.
- `Battery` sits behind `Capabilities::POWER` ([power](power.md)); a session without that bit reads it as `Starting`. A running feed whose pack falls under the floor ends `Failed`/`Power`.
- The panel lock is not a reason: it holds only the Dispense button.

**The Dispense button** asks for one portion, as stock's does. It goes through the same readiness and the same owner as a client's `Feed`, and its `Fed` event says `by: Panel`. A locked panel ignores it, and a pending pairing confirmation takes the press first. The loop logs `PLFEED1 dispense feed=N` or `dispense refused=…`.

A press while a feed cannot start, a feed running among the reasons, starts nothing and is not queued, as stock's (its record code 2022). Such a press is recorded as a `DispenseRefused { reason }` event when its reason differs from the last one recorded since the feeder was last ready; a repeat of that reason is only logged, so presses during a long feed cannot fill the event queue, whose appends wait for the feed's end.

**Feeds are silent.** A feed, by a press or a client, asks nothing of the speaker. The loop still steps the speaker outside setup mode for a client's `play` request, after the owner and the device API. So a tune that is playing, will not open or stalls leaves a feed, its cancel and its `Fed` event alone. An owner-chosen call before a scheduled meal belongs to the schedule feature.

**No restart into a feed.** `Reboot` is refused `Busy` while a feed is queued or runs. Once a reboot is accepted, a `Feed` is refused `Busy` until the restart. The restart itself waits for a feed still in flight at its deadline (a Dispense press in that second). BLE setup mode's closing restart waits for a feed in flight too. So none leaves a feed uncertain.

**A server never re-asks a feed across a feeder boot.** The id memory lasts one boot, so a resent id after a restart is a new feed, and a server cannot be sure no restart came between its check and its resend. So the owner's server never re-asks a feed whose answer it lost. It settles the feed from the history's `Fed` events, matched by the feed's number or, without one, by the one unclaimed `Fed` event after the feed was sent. It reports the feed `unrecorded` once the feeder has restarted with no record of it ([server design](../server/design.md#commands-and-feeds)).

**Two paths, one write.** An owner's or member's phone holds a key of its own beside the server's, and the two send different write ids. A write whose answer was lost on one path and resent on the other would run twice. So an uncertain write never switches path. It is retried on its own path under its own id, or resolved from the history (a feed by its `Fed` event), never resent through the other. The server's half answers an unknown write `outcome_unknown`, keeps it unknown under its key, and settles a feed from its `Fed` event, readable at `GET /api/v1/commands/{key}` and on the stream ([two paths, one feed](../server/README.md#two-paths-one-feed)).

### No flash write while the motor may move

Every flash lease checks that the outputs are stopped, and fails closed when they are not: the lease hook halts and the watchdog resets the board mid-feed. So every store write outside the feed journal first takes the write turn in `wellfed_hal::interlock`. That covers the event ring, settings, the client and key stores, the Wi-Fi store, the seed, the build confirmation, and the update stage's records and bank-1 writes. The turn is granted only while the loop holds no claim on the outputs.

- **A write that comes during a feed waits.** The settings writer leaves it queued and asks again every 200 ms, still answering event reads. It counts itself among the waiting writers, so the next claim lets it go first. It runs once the owner is at rest with its outputs stopped.
- **The forced path.** Only a claim that outlives `FEED_BOUND_MS` lets a write go ahead with the outputs claimed. The bound is timed from when the writer first saw that same claim; a writer that misses the gap between two feeds starts again on the second. Forced writes are counted (`PLOWN1 interlock … writes_forced=`), and run only while the outputs read stopped. A write never starts into moving outputs.
- **The update stage never takes the forced path.** An update request waits in its mailbox, unanswered, until the outputs are free. An install holds the turn from its first write until it stops, so every feed meanwhile is refused (`held=3`). A give-way or an activation keeps the turn until the restart.
- **Marker order.** The confirmation takes the turn before arming the hooks, and the update stage before it sets the attempt marker. So the marker is never left set through a wait.
- **Order is kept.** Queued writes complete after the feed in their usual order: the events in the order queued, and each mailbox's single change once.
- **The journal takes no turn.** The owner reserves and finishes a feed only after `quiet()` has stopped the outputs. It opens (creating the journal, or resolving an interrupted feed) before any feed is claimed.
- **Trial boots.** No feed is claimed before the confirmation is written. The owner opens only once the hooks are armed, the confirmation is that boot's first arming (the settings writer starts after it), and it holds the write turn from arming to its write. So a feed cannot push the confirmation towards the trial deadline.
- **Counters.** `PLOWN1 interlock …` counts `writes_deferred`, `writes_forced`, `write_waited_max_ms` and `feeds_delayed` (claims that waited for a write).

**A client's store write during a feed is refused `Busy`.** It would wait for the feed's end, longer than a client waits for its answer (10 s). A slot waiting on an answer reads no frames, so the client would end the session unsure of the write's fate.

- While the outputs are claimed, the loop refuses a settings change, a Wi-Fi change and a revocation `Busy` before queueing anything. Nothing changed, and the client may send it again after the feed. Reads still answer.
- An enrollment asks again each pass until the feed ends, as it does when the keys mailbox is taken.
- A change queued before the claim goes first. The claim also waits, within its `WRITE_WAIT_MS`, while a settings, Wi-Fi or keys change waits in its mailbox, so the writer writes it and the client is answered before the feed starts.
- A change still in its mailbox at the end of that wait (a stalled writer) refuses the feed `Busy` (`held=3`) instead. So a change is never written after a feed it was queued before.
- BLE setup's Wi-Fi save, which waits for its own answer, waits out a feed.

### Writes, roles and generations

**Writes are remembered per client key:** the last 32 write ids and their compact outcome, so a retry after a reconnect gets the first answer. A settings or Wi-Fi write names the generation it was based on, and is refused `Stale { current }` when the store has moved. The loop is the only requester, so the check and the queueing cannot interleave with another writer.

**Roles.**
- An owner does everything, and may invite any role but an update signer.
- A member, or the server, may feed, change settings, the plans and the lock, play, sync the clock, read the time view and list clients. Not Wi-Fi, the time settings, revocation or reboot.
- A member may invite members. The server may invite no one (`pairing::Pairing::invite`). So an owner's server that makes its phones' invites, revokes their keys or changes Wi-Fi is adopted with an owner's invite.
- An update-signer key never opens a session.
- A clock sync from an owner may step the clock; a member's or the server's may only narrow it ([clock](clock.md#the-clock)).

### Settings, Wi-Fi and the lock

**Device settings** are lights, sound, volume (0–100) and a window each for lights and sound, with a record generation. A window is `Always` or `Between { start, end }`: minutes of the local day, both included, wrapping past midnight when the end is before the start, as stock's "aging type" windows compare them. Lights show while `lights` is on and inside the lights window; a sound plays while `sound` is on, the volume is above 0 and the time is inside the sound window. While the clock is invalid or has no zone, every window is open, so a fault light is never hidden by a lost clock. The `wellfed settings` command takes them as `--lights-window` and `--sound-window` (`always` or `HH:MM-HH:MM`). The settings also hold the desiccant reminder's period (`desiccant_days`, 1–90, `--desiccant-days`) and the local day of its last replacement (`desiccant_reset`), which only `ResetDesiccant` sets; the status reports the count (`Status::desiccant`: `Unset`, `Unknown` without a clock, or `Remaining { days }`, due at 0 or below) ([desiccant reminder](desiccant.md)). They also hold the meal reminder, `Off` or `Before { minutes }` with 1–60 minutes (`--meal-reminder off|N`), which sends a `MealSoon` event ahead of each scheduled meal ([schedules](schedules.md#meal-reminder)). The record is version 4 (the meal reminder's lead at byte 31, the windows at bytes 34–44, the desiccant reminder at 44–52); a record of an older version reads as erased, so the store opens writable on defaults. The settings writer (`src/product/wellfed-feeder-runtime/src/writer.rs`), which the runtime's service task steps, owns the store in storage sectors 2 and 3.
- It starts after the boot is confirmed, and arms the flash-lock hooks if the confirmation did not.
- Each save sets the arming attempt marker and clears it after.
- One change waits at a time; a second while one is queued is refused busy, and an unwritable store is refused.
- Records: `PLCFG1 start`, `open status=… generation=… lights=… sound=… volume=… lights_window=… sound_window=… meal_reminder=… desiccant_days=… desiccant_reset=…` (`off` or the minutes; a day number, or `none`), the arming records, `saved …` and `error stage=unarmed|save status=…`.

**Wi-Fi credentials** are stored for the next join: the next boot, or the next attempt while the feeder is offline. A clear returns to the build's credentials.
- **Store:** `wellfed-feeder/src/store/net.rs`, two 128-byte records in storage sectors 5 and 6, written by the settings writer inside a marked attempt. An SSID is 1–32 bytes; a passphrase 8–63 printable ASCII.
- **Join:** before each attempt the C shim takes the build's credentials, and Rust substitutes the stored ones. After 5 failed attempts in a row with no success, the build's credentials take over until the next boot or the next save.
- **Saving while offline:** the save is read at the next attempt, with a fresh count of 5. BLE setup relies on this. While the feeder is online, nothing rejoins.
- The connect event names the source: `build`, `stored` or `fallback`. Nothing ever reports the SSID or passphrase themselves.

**The child lock.** The device API reaches the lock only through a one-slot box the loop's panel owner reads (`wellfed_feeder::feeder::lock`). An unlock from the network relocks after the same 30 s idle time as a button unlock.

**The speaker.** A client's `play` request queues one built-in tune (`chime`, `meal`, `coin`, `alert`, `test`, `twinkle` or `key`) through `audio::request`, which reads the settings and fills a one-slot mailbox. It is refused (`Muted`) when `sound` is off, the volume is 0 or the time is outside the sound window, or busy while a tune plays or the settings are not loaded yet ([audio](audio.md)).

### Time and calibration

- **Time** (`Capabilities::TIME`). `GetTime` answers the time view. `PutTime` queues the zone's rule and the NTP server for the settings writer (storage sectors 27 and 28). It is answered `Done` once the store holds the settings asked for (or already held them), and refused `NotWritable` when the writer could not write them, with no event. The loop applies a new generation on its next pass, and runs the NTP exchange on one more UDP socket ([clock](clock.md#ntp)).
- **Plans** (`Capabilities::PLANS`, bit 1024). `PutPlans` is checked against the plans' generation like a settings write, refused `Portions { asked }` for a plan asking for other than 1 to 48 portions and `Invalid` for one with another field outside its limits, before anything is queued, and `Busy` while the outputs are claimed. It is answered `Done { generation }` once the store holds the table, and records a `Plans` event with its client. The writer saves the table in storage sectors 11 and 12 ([schedules](schedules.md)).
- **Desiccant** (`Capabilities::DESICCANT`, bit 2048). `ResetDesiccant` takes no base: a replacement is a fact about today, whatever the settings hold. It is refused `NoClock` while the clock is invalid, `Busy` while a settings change is queued or the outputs are claimed, and `NotWritable` for a store that cannot be written. Otherwise it queues today's local day for the settings writer, is answered `Done { generation }` once the store holds it, and records a `Settings` event with its client. Owners, members and the server may send it, as they may change the plans ([desiccant reminder](desiccant.md)).
- **Calibration** (`Capabilities::CALIBRATION`, bit 256). `GetCalibration` answers the stored weight of one portion, the bowl mode the build runs and the rotor's phase. `PutCalibration` (owners only) queues a new weight for the settings writer (storage sectors 34 and 35). It is answered `Done` once stored, `Invalid` outside 1–50 g, and `Unverified` for a bowl mode the build was not verified for. The statistics' `Portions` section needs the same bit. There is no event: the calibration has its own generation ([calibration](calibration.md)).

### Events and samples

**Events.** The loop's event recorder (`wellfed_feeder::feeder::events`) queues records in `wellfed_feeder_runtime::events`.
- The queue holds 16 in RAM, the last kept for a feed's `Fed` event, since appends wait out a feed and events pile up meanwhile. Overflow is counted and recorded as a `Lost` event.
- The settings writer appends them to the ring in storage sectors 13–24 (`wellfed_feeder::store::events`: 32-byte records, at least 1,408 kept) and answers the API's reads. It polls every 5 ms while either waits. While a feed holds the outputs, an append waits at the head of the queue.
- A ring created empty draws a random epoch from the generator, and every record repeats it in bytes 4–7, the high half of the old 64-bit sequence field. So a ring written before epochs reads as epoch 0, and the layout is unchanged. The welcome carries the epoch, and the feeder welcomes no session until the writer has opened the ring and published it.
- Recorded today: boot, lock changes (panel, idle relock, or which client), settings, plans and Wi-Fi writes with their client, clock syncs, invites redeemed, recovery and revocations, feeds, refused Dispense presses, meal reminders, missed meals, and blocked feeding.

**Samples** go only to subscribers: power every 5 s, Wi-Fi signal every 30 s, the statistics' metrics with `STATS` ([below](#statistics)), and the judged supply beside the power sample with `POWER`. A sample not sent before the next of its kind is replaced.

**Supply** (`Capabilities::POWER`). `Metric::Supply { adapter, battery, below_floor }` names an unread, stale or implausible reading rather than passing it off as a value. A plausible reading is `Battery::Reading { mv, percent }`, its charge on stock's curve. On its batteries the feeder turns Wi-Fi off 5 s after the adapter goes, so sessions end until it returns, and it refuses an update offer `Update(Battery)` ([battery mode](power.md#battery-mode)). `EventBody::Supply` records the adapter's changes and the pack crossing its floor, at most one a minute ([power](power.md)).

### mDNS

`wellfed_core::mdns` answers `_wellfed._tcp` on one UDP socket in the loop: PTR, SRV (port 4747), TXT (`v=1`, `id=` the identity key) and A, for the host `wellfed-<first 4 key bytes>.local`.
- It waits for a DHCP address before opening its socket.
- It announces twice on every address change, and answers legacy unicast queries.
- The vendor responder (33.5 KB and its own task) stays unlinked. `wifi_register_multicast_address` is not needed.

> [!NOTE]
> A host firewall that drops UDP 5353 hides the feeder from discovery. Pass `--address` instead, or allow UDP 5353 on the interface the feeder uses.

### Updates, backups, invites and recovery

- **Updates** (`Capabilities::UPDATE`, owner sessions only): `OfferUpdate`, `UpdateChunk`, `UpdateState`, `CancelUpdate`, `EnrollUpdateKey` and `UpdateKeyStatement`, answered `Outcome::Update` or `Refusal::Update`.
  - The loop never verifies a signature. It copies the request into the update mailbox and answers from what the service task's update stage publishes ([OTA design](ota-design.md#updates-over-the-device-api)).
  - An offer is refused `Busy` while a feed is queued or driven, and a feed while an install holds the flash.
  - A chunk is idempotent by its offset, so it is not a remembered write and is not logged one by one.
  - An update key is enrolled only once someone at the feeder presses Dispense. The session's answer waits for it (the client's answer time is longer for this op), and a session that leaves cancels the offer.
- **Debug log** (`Capabilities::LOG`, owner sessions only): `TailLog { after }` streams the firmware's record lines from a RAM tail ([below](#debug-log)).
- **Feed traces** (`Capabilities::TRACES` with `STATS`): `FeedTrace { feed, offset }` reads what a feed's drive measured, a page at a time, from RAM or storage sectors 29–31 ([below](#feed-traces)).
- **Stock backup** (`Capabilities::BACKUP`, bit 512, owner sessions only): `ReadBank1 { offset }` reads bank 1 a page at a time, and `ReadStockPlans` stock's plan sectors, both served only from bank 0 ([below](#stock-backup)).
- **Invites.** `CreateInvite` mints a secret, which is the PSK of the new client's XXpsk3 handshake. Its third message carries the label, and the key enrolls through the keys mailbox.
- **Recovery.** The recovery window runs the same setup exchange as BLE (XX plus the code proof) over the stream, so the owner keys the client's code in before it becomes an owner. A LAN neighbour cannot slip in while the window is open, and the window closes at the first offer. Recovery takes no Wi-Fi ([pairing](pairing.md)).

## Debug log

An owner can read what the feeder's UART prints without opening the feeder: `wellfed logs` (`--follow` for new lines, `--after N` to resume).

- **The tail.** Every record line goes through `wellfed_amebad_sys::log` (`wellfed_sdk::log`, the `Record` derive and the runtime's own lines). It prints the line as before, then copies it into a 4 KiB RAM ring, `LOG_TAIL` (`wellfed_core::log_tail`): about 70 lines of the usual 56 bytes.
  - The copy happens inside the same scheduler suspension as the print, after it. So the UART sees each line at the same moment as before, and tasks never contend for the ring.
  - Vendor C prints (`_rtl_printf`, `DiagPrintf`, the panic record) do not go through `log` and are not in the tail.
  - Nothing is written to flash. The tail starts empty each boot and numbers its lines from 1.
- **Safe from an interrupt, never waiting.** Only an interrupt can find another writer mid-copy, since a task logs with the scheduler suspended. It drops its line from the tail (the UART still prints it) and counts it, rather than wait for a writer that cannot run until it returns. The dropped line still takes a number. No interrupt logs today.
  - Readers never block writers. A reader copies a line, then checks the ring's oldest line has not moved past it (a seqlock), starting over a few times at most.
  - A line too long for the ring (over 255 bytes; the record buffer is 192) is counted the same way.
- **Overrun drops the oldest.** A line that needs room drops whole lines from the old end. A session whose cursor fell behind learns how many it missed: each batch's `dropped` counts the numbers after its cursor that the batch does not hold.
- **Batches fit the frame, only when the session has room.** `TailLog { after }` is answered `Done`. `None` starts from the oldest line kept, and asking again moves the cursor. The session then pushes `Log { seq, dropped, text }`: as many whole lines as fit `LOG_TEXT_MAX` (992 bytes), each ending in `\n`, with `seq` the last line's number.
  - Like an event or a sample, a batch goes only when the session's last frame is out, after any event and sample waiting, one frame per pass. So a session that cannot keep up costs only its own lines.
  - Text is printable ASCII; any other byte reads `?`.
  - A cursor past the newest line can only be an earlier boot's, and reads from the oldest. One below it cannot be told from this boot's own, so a client resumes with a cursor only within one boot (the status's `boot_count`).
  - Lines dropped after the newest written one show in `dropped` only once the next line is written.
- **Ships in release images.** The `feeder` firmware claims `LOG` in every build, not only behind a debug feature.
  - A shipped bit is served for the life of its major ([rule 5](#evolution)). That costs little here: the op's contract is text with no schema, so any later build can keep serving it from whatever it logs.
  - An owner's feeder has no UART wired, so the tail is the only way to see a field fault's lines.
  - It is owner-only, and no record carries a key, a secret or a credential (each record type's rule). The stream shows an owner nothing they could not already change.
  - The other firmwares (`blink`, `bringup`, `qa`) serve no API and build without the `log-tail` feature, so they keep no tail.
- **Cost.** `.bss` grows by 4,112 bytes (the ring and its state) and flash-resident code by 1,920 bytes. The copy runs in its own 56-byte frame under `log`, and a batch reuses the server's buffers, so every task's stack bound stays within noise. Reading a batch scans at most the ring's headers and copies once, then seals like any frame (about 0.9 ms a kilobyte).

## Evolution

**While Wellfed is in active development, none of the rules below is kept for backward compatibility** ([AGENTS.md](../../AGENTS.md#compatibility)): the research unit is the only feeder running it, and the firmware and every client are built from this repository together, so a message changes in place and its fixture lines are rewritten by hand. The rules describe how the API is meant to evolve once outside clients or other feeders exist; the prologue and the epoch (rules 1 and 4) still hold, since they are about correctness rather than old peers.

The intended rules:

1. **The prologue never changes.** `wellfed-api-v1` is bound into every handshake. A new major is negotiated in `Hello.major`. A feeder that does not serve it answers with the typed `OpenRefusal::Version { supported }`, so a client can say which side to update. A changed prologue would instead fail the handshake with no reason. Enforced by `the_prologue_never_changes` (`wellfed-proto/tests/fixtures.rs`) and `a_major_2_hello_gets_the_typed_version_refusal` (`wellfed-client/tests/api.rs`).
2. **Every new op, event body, outcome or refusal is gated by a `Capabilities` bit both sides claim,** as `FEED` gates feeding and `UPDATE` updates. `Op::capability` and `EventBody::capability` name each variant's bit with no wildcard, so a new variant does not compile without its decision. The feeder serves an op only when the session's hello and the feeder both carry its bit (otherwise `Unavailable`). It pushes an event only when the hello carries its bit (otherwise the session's cursor moves past it). Enforced by `a_v1_only_session_never_receives_a_body_outside_v1`, which records one event of every body, an exhaustive match keeping the list whole.
3. **Existing messages never gain, lose, reorder or retype a field.** Postcard is positional, so an old peer would misread every byte after a new field. New data is a new variant behind a bit. Enforced by `tests/fixtures/v1.txt`, which pins every v1 message's bytes. `WELLFED_BLESS=1` only appends fixtures for new variants; it refuses to rewrite a pinned line. For now, see the note above.
4. **The event history carries an epoch.** `Welcome::Open.epoch` is drawn at random when the ring is created, and kept with it. Event numbers count within an epoch. So a cursor from a re-created ring (a storage wipe, which restarts the numbers at 1) is recognized, rather than freezing a mirror that waits for numbers it already has. `wellfed-client`'s `Connection::subscribe` resumes after a cursor only in the feeder's current epoch, and otherwise starts from the oldest event kept (`Resume::Restarted`). The server keys events and gaps by (feeder, epoch, seq), and keeps an older epoch's history readable (`?epoch=`). Enforced by `a_cursor_resumes_in_its_epoch_and_restarts_from_another`, the event ring's epoch tests, and the store's `a_new_epoch_keeps_its_own_history_and_restarts_the_cursor`.
5. **Clients adapt to the feeder's capabilities indefinitely.** An owner may hold an update forever, so there is no "N and N−1" window. A client claims every bit it knows (`Capabilities::KNOWN`), runs on the intersection the welcome gives it (`Opened::capabilities`), and keeps speaking every older major it ever spoke. A feeder keeps serving every bit it ever shipped within its major.

## Statistics

`Capabilities::STATS` (bit 16). The loop keeps the figures in RAM as it runs (`wellfed_feeder::stats::Keeper`): no task, no wait and no flash write, a few additions per pass. The lifetime totals are the settings writer's, in storage sectors 32–33.

**`Stats { section }`** answers one section in one frame, for any role that opens a session (`Memory` for owners only). The widest of each is checked against `MESSAGE_MAX` (`the_largest_messages_fit_a_frame`, and the device API's property test through the server).

| Section | Holds |
| --- | --- |
| `Counters` | Why this boot started (`ResetCause` from the ROM's reset flags: power-on, watchdog, software, brownout, other); this boot's uptime, counted past the millisecond clock's 49.7-day wrap; feeds, and those not done; confirmed portions; panel gestures; sessions; reconnects (link and check losses); DHCP renewals; outages and seconds on battery; the longest and mean handshake (accept to open) |
| `Memory` | The heap's free bytes and low-water mark, and each RTOS task's fewest free stack bytes (up to 16, with `tasks_total` the number the RTOS runs, so a truncated list shows) |
| `Flash` | Each storage sector's erases and bank 1's this boot, counted where every erase goes (`storage_erase` and `bank1_erase`), and the event ring's records kept against its capacity |
| `Power` | The adapter now, its changes and outages, time on battery, the battery's least and most, and its voltage at the end of each of the last 24 hours |
| `Food` | The food beam's last reading, its empty and refill transitions, and feeds since a refill |
| `Feeds` (with `TRACES` too) | This boot's feeds as their traces measured them ([feed traces](#feed-traces)) |

- **Memory's walk** (`uxTaskGetSystemState` and the name copies, with the scheduler paused) runs at most every 5 s, its result reused meanwhile, and never during a feed: the request is refused `Busy` while one is queued or runs.
- **Food stays empty for now.** The beam is read only while the emitters are on, and the feeding owner does not publish it yet (`Feeds::food` answers `None` on the PLAF103).

**Lifetime figures** (`Counters::lifetime`, `resets`, `Flash::lifetime`) are the stored totals plus this boot's figures, with no wire change.

- **The store** (`wellfed_feeder::store::stats`, sectors 32–33 of the 48-sector region) is one 288-byte dual-slot record (`PLFSTA01`). It holds the ten counts, boots by reset cause, erases of every storage sector (all 48, though the v1 `Erases` lists the first 32) and of bank 1, and the early-save budget. Newest valid wins, so a cut save loses only that save. Its layout covers the region as it is; growing the region again takes a new record version.
- **At boot** the writer loads it, adds this boot to its reset cause (from the ROM's flags, as `Counters::reset`) and publishes it for the loop (`PLCFG1 stats open status= generation= early= missed=`). The loop hands this boot's counts to the writer every 30 s, beside the signal sample, and again just before it asks for a restart. The loop never writes.
- **Saves** take the write turn like every store write, but never one forced past a claim's bound, so they never run while a feed holds the outputs. They come only when nothing else is queued, never per event (`PLCFG1 stats saved generation= early=`):
  - **early:** the boot's own as it starts, so a reset is counted however short the boot, and one in an intentional restart's grace (below). Each takes one of 6 tokens kept in the record. The save that takes the last sets a sticky flag (`missed=1`: a boot may have been missed), and boots after it go uncounted until a settled save gives a token back;
  - **settled:** every hour of a boot's uptime, the first an hour in. Each gives one token back. A power cut loses at most the last hour of a boot.
- **Wear.** Early saves never outnumber settled ones by more than 6. So the store erases at most 6 times plus twice per hour of uptime, whatever the crash pattern: about 17,500 erases a year, 8,800 of each sector, far inside NOR endurance.
- **A restart's save** happens in the restart's grace. `wellfed_sdk::request_reset` (the device API's reboot, setup's restart) opens it (`supervision::begin_restart_grace`). The watchdog is still fed and writes still run. The writer saves (an early save) and says it is done, or has nothing to save. The runtime restarts then, or after 2 s (`RESTART_GRACE_MS`) if the save is held, say by a feed.
  - Once the feeding stops, no write turn is granted, so no save starts into the reset. A cut write there would leave the arming marker set and the next boot without its stores.
  - An update's activation or give-way restarts at once, without a grace; it holds the write turn until the reset anyway.
  - A supervision stop (a hung task, unverified outputs) saves nothing.
  - Each save counts its own erase. A failed save is not retried that boot.
- **Lower bounds.** The lifetime counts and `resets` are at least the truth, never more. A power cut loses up to an hour of a boot's counts, and while no token is left, short boots go uncounted. The `missed` flag that says so is in the log, not on the wire.
- **`None`** comes from a build that keeps no totals, and from a store this build cannot write (a newer record version, or a read error).

**Metrics** go every 30 s, beside the signal, to subscribers whose hello claims the bit (`Metric::capability`):
- `Heap { free, min }`;
- `Loop { passes, max_us, p99_us }`: the loop's passes since the last, timed on the 32 kHz system timer around the feeding owner's step and the pass, the percentile from buckets an eighth of an octave wide;
- `Gateway { round_trip_ms }`: the latest answered probe since the last, `None` if none;
- `Wifi { channel, ap_changes }`.

A session holds one sample of each kind and sends the oldest first, so a kind is replaced only by its own next sample.

**The Wi-Fi shim** reports two more events in `PLWIF3`: `kind=ap`, after each join's DHCP, with the channel and the BSSID's last four bytes; and `kind=renew`, a lease renewal. A renewal is seen as lwIP's lease clock going back while the client is bound. A release on expiry also zeroes it, and is not counted.

**Records:** `PLAPI1 session … state=open` ends `handshake_ms=`.

**Host:** `wellfed stats [--section …]` prints each section as JSON. It asks only a feeder whose welcome claims the bit, since a feeder that predates the op would end the session on it.

## Feed traces

`Capabilities::TRACES` (bit 64), served only beside `STATS`: an op needs both bits. The feeder keeps what each feed's drive measured, poll by poll, in RAM and then in its own storage. So an owner can read it without a UART or a server: `wellfed trace <feed>` (`--json` for the whole trace).

**Recorded after each owner step, never inside it.** The feeding owner keeps one `Observation` (`wellfed_core::feed::observe`) per poll of a feed being prepared or driven:
- the current channel (PB1, ADC channel 4);
- the battery pack in millivolts;
- the outlet and food beams, read only while the emitters are on;
- PA18's edge counts, and the latest edge's time, from the feedback interrupt;
- the controller's motor command.

The loop takes it once the step is over (`Owner::take_observation`, only from the task that steps the owner) and adds it to the trace (`wellfed_feeder::trace::Recorder`, held in `wellfed_feeder_runtime::traces`). So the trace runs after the step's stop and motor decisions: no task, no wait, no flash, and the motor lease and the supervision timer never depend on it. The poll is the owner's own, so a reading comes about every 30 ms on the PLAF103 (903 polls in a 27.2 s drive), not every 10 ms.

**Format** (`wellfed_proto::trace`). A body of entries, each timed in milliseconds from the drive's start and coded against the one before:
- current readings, two bytes at a steady poll;
- marks: PA18 edges at the interrupt's millisecond, with the level after;
- beam changes;
- battery moves of 20 mV or more;
- motor command changes (`Drive0`, `Drive1`, `Stop`), which line up with the logic analyzer's D2 and D3.

At most 864 bytes. The whole recorder is under 1 KiB of RAM, a compile-time check.

**Overflow: decimated, then truncated.** A full body halves its current readings in place: the second of each pair goes, and the first reading, a drive's start-up surge, stays. From then on it keeps every second poll's reading (`stride` doubles). Marks, beams, battery and motor entries are always kept; only once they alone fill the body does it end there (`truncated`). A one-portion feed keeps every poll; three portions keep every 4th (about 120 ms); 48 portions every 64th or so, with every mark. The summary never depends on the body.

**Summary** (`TraceSummary`):
- the feed's result (requested, completed, outcome, reason);
- the owner's own figures, the same that `PLOWN1 fed` logs: drive time, polls, peak and mean current, rising and falling edges, the first edge's time;
- beam breaks: a poll that found the outlet beam blocked after finding it clear;
- reversals: the owner's count of its reverse attempts on a jam, the same `PLOWN1 fed … reversals=` logs. A double bowl's turn between portions is none;
- the battery just before the drive, and its lowest during it;
- the stride and `truncated`.

Current is in stock's scaled unit (`CurrentUnit::Scaled`) until its scaling to milliamps is calibrated. A later build switches the unit without a wire change.

**Stored once the feed is over.**
- The loop seals the trace when it records the feed's `Fed` event.
- The settings writer appends it to the ring under the write turn, so never while a feed claims the outputs, and under the arming attempt marker like every store write (`PLCFG1 trace saved feed= seq=`, or `trace error feed=`).
- A sealed trace counts as a queued write, so the next feed's claim lets it go first (at most `WRITE_WAIT_MS`).
- The next feed takes the RAM trace back. One that never reached storage is counted (`unsaved`). A feed that starts while the writer is still appending goes untraced rather than wait.
- A feed a reset interrupted loses its trace with the RAM; its `Fed` event, `Uncertain`, remains.

**The ring** (`wellfed_feeder::store::traces`, storage sectors 29–31) holds four 1 KiB slots a sector.
- Each slot is a 96-byte header (`PLTR`, version, the lengths, a sequence number, the summary, a CRC-32 over header and body) programmed first, then the body. A cut before both are in leaves a slot that fails its checksum and is skipped.
- When the newest sector is full, the oldest is erased. So the ring keeps the last 8 to 12 traces, and each sector is erased once every 12 feeds.
- A power cut at any mutation of an append, or of its erase, loses at most the trace being written.

**`FeedTrace { feed, offset }`** (op `0x17`) answers `Outcome::FeedTrace` (`0x0b`): `TracePage { summary, stored, offset, total, bytes }`, with at most 432 bytes of body. So a three-portion trace is two pages.
- Any role may read it.
- The loop answers from the latest feed's trace in RAM, otherwise from the ring through mapped flash. A read takes no lease, and a lease holds every other task off the flash.
- `Busy` while that feed is still recorded, `NotFound` when neither holds it, `Invalid` for an offset past the body.
- Each page repeats the summary, and `wellfed_client::trace::Pages` refuses a trace that changed between pages, rather than splice two.

**Statistics.** `Stats { section: Feeds }` (section `5`, both bits) sums this boot's traces: traced feeds, drive time, polls, peak and mean current, edges, beam breaks and reversals. It adds the ring's fill, the unsaved count and the latest summary.

**Cost.** About 1.4 KB of `.bss`: the recorder, under 1 KiB, and the page an answer borrows. Recording costs a few comparisons and a two-byte append a poll, and a halving one pass over under a kilobyte. A page read scans twelve slot headers and checks one trace's CRC from mapped flash.

**Why a bit of its own.** A feeder that serves `STATS` but predates traces cannot decode `FeedTrace` (or the new section). An undecodable request ends the session, so a client asks only a feeder whose welcome claims both.

## Stock backup

After stock installs the replacement into bank 0, bank 1 still holds stock, with only its marker's first byte zeroed. Promotion would overwrite it. So the installer reads it first and keeps the owner's own copy ([rollback](consumer-installation.md#rollback)).

- **`ReadBank1 { offset }`** (op `0x1a`) answers `Outcome::Bank1` (`0x0d`): `Bank1Page { offset, total, bytes }`, with at most `BANK1_PAGE_MAX` (768) bytes. The bank's 1,376,256 bytes (`BANK1_BYTES`, stock's payload size) are 1,792 pages. It sits behind `Capabilities::BACKUP` (bit 512): a feeder that predates the op cannot decode it, and an undecodable request ends the session.
- **Who and when.**
  - Owners only; a member or the server is refused `Forbidden`. The copy is the owner's to keep.
  - Read-only and not a remembered write, so any page may be asked again.
  - Served only from bank 0 (`Unavailable` from bank 1, whose own image is the running build). The SDK's read also refuses unless the flash MMU maps bank 0.
  - `Busy` while an install holds the flash, since it rewrites the bank under the read. `Invalid` at or past the bank's end.
- **How.** The loop copies the page through the mapped flash window (`sdk::bank1_read`, a sector at a time; a read takes no lease) into the 768-byte page buffer, which the trace answers also borrow. Nothing vouches for the bytes: `wellfed_client::install::StockBackup::from_bank` restores the marker byte, checks the payload against the pinned 1.1.15 hash, and rebuilds the vendor's container, whose hash must match its own pin. Page answers are not logged one by one, like update chunks.
- **Stock's plans.** `ReadStockPlans` (op `0x1e`) answers `Outcome::StockPlans` (`0x0f`): `StockPlanSectors { count, entries }`, stock's count byte at flash `0x7d0000` and the first 440 bytes (`STOCK_PLAN_ENTRIES_LEN`, ten 44-byte entries) at `0x7d1000`, everything stock's boot reads ([plan records](../devices/plaf103/stock/README.md#plan-records)). The same bit, roles and refusals as `ReadBank1`: owners only, bank 0 only, `Busy` during an install. The SDK reads the two fixed spans through the flash window (`sdk::stock_plans_read`) into the page buffer; nothing in Wellfed writes those sectors, so a rollback finds stock's plans as it left them. The feeder judges none of the bytes: `wellfed_client::install::plans` decodes and converts them.

## Resources

From the `feeder` builds' manifest audits and `PLAPI1` records:

| | Before the device API | With the device API | Latest build |
| --- | ---: | ---: | ---: |
| Payload | 868,352 | 942,080 | 1,015,808 |
| Loop stack bound | 3,912 of 6,144 | 4,552 of 8,192 | 5,412 of 8,192 |
| Heap free, steady | 146,464 | 125,088 | — |

- **Stack.** The loop's stack is 8 KiB. At 6 KiB the bound sat 56 bytes under the audit's 25 % margin. The recovery responder's 1.2 KB frame runs out of line (`rescue_op`), so it never nests under the handshake step's 1.1 KB. The deepest path in the latest build is still a feed's submission under a session's request.
- **Heap.** Flat at 125,088 free over minutes, and through a garbage flood. The difference from the first product budget is not yet attributed.
- **Other tasks' bounds:** `wellfed` 2,808 of 4,096, `wellfed-key` 1,248 of 2,048, `wellfed-svc` 11,744 of 16,384, `wellfed-wdg` 1,420 of 2,048.

## What went away

No build opens an MQTT session; the broker in `wellfed-stock` serves only stock firmware. Requests, offers and health all ride the device API.

- The firmware's MQTT transport is deleted with its request parser, reply writers, request signing and health nonce. The normal role starts no network task of its own.
- The request and reply types the API fills live in `wellfed_device_api::mailbox`. Those the SDK's own services carry are in `wellfed_core::settings` and `wellfed_core::tone`.
- The builder no longer reads broker credentials or a request key (`private/fw-mqtt.json`).
- The startup audit requires no transport policy and no `PLMQT1` start record.
- A trial build confirms on a live link: associated, leased, and its gateway answering, or never having answered this join. No internet is involved ([link liveness](startup-diagnostics.md#link-liveness-without-internet-dns)).
- A stock-to-replacement install closes on the bench's device-API check of the new build on bank 0 (`POST /api/ota/trial/answered`).

## Evidence

- **Observed** (October 24, 2026; no owner action; each install used a bounded `serve`):
  - `fw47-api-02` (v114): `PLAPI1 listen socket=api open=1`. `wellfed status` opened an IK session as Owner. Settings writes moved the generation 3031 → 3032 → 3033. Lock, unlock and a clock sync were recorded, each with the client's fingerprint, and `events` read all six records.
  - **Invite and revoke:** an owner minted a member invite, and `wellfed join` redeemed it (XXpsk3) with a scratch key, which then opened a session as Member. With that member's `events --follow` session open, `revoke` closed the session at once, and the key was refused afterwards (`OpenRefusal::Unknown`). The history showed `Enrolled { path: Invite }`.
  - **Samples:** a subscriber received power samples every 5 s.
  - **Client limit:** with four sessions held open, a fifth connection was closed at accept.
  - **Flood** (`fw47-flood-01`, about 35 s): 112 connections of 4,000 random bytes each were closed as `malformed` one by one. An owner's `status` kept answering in about 0.1 s. There was no supervisor stop, and the heap was unchanged. The Wi-Fi driver logged `Cannot allocate pbuf` four times during the burst.
  - **History across a cold boot** (`fw47-mdns-01`): `PLCFG1 events open=1 last=10 oldest=1`.
  - **mDNS:** the first socket opened on the SDK's static pre-DHCP address (192.168.1.80) and never saw a multicast query, so the responder now waits for DHCP. `wifi_register_multicast_address` returned -95. The feeder then answered every query (`mdns_in=15 mdns_out=13 mdns_failed=0`), and a legacy unicast query got a correct A record. Discovery from the bench host failed until the owner allowed UDP 5353 on the hotspot interface; then `wellfed status` found the feeder without `--address`.
  - **The bench on the API** (`itest`, the owner key, the address from the neighbour table): `fw47-itest-01` (smoke tier, v119) passed `ota-update`, `requests` (the clock 3 ms from the host's after a sync) and `request-auth`. `fw47-itest-02` (standard tier, v120) passed `ota-update`, `requests`, `request-auth`, `settings-persist` and `wifi-fallback`; `network-bounce` failed because the outage showed as a DNS loss `Status` did not report, so `dns_losses` was added. `fw47-itest-03` (v121) then passed `ota-update` and `network-bounce`. `qa-20261024-01` passed all six; `qa-20261024-03` and `-04` passed `ota-recovery` and `ota-power-cuts`.
  - `Status.boot_count` once reported KM0's whole register 4, trial bits included (`2533425155` = `0x97010003`). It now reports bits 0–15.
- **Observed** (October 24), recovery with the owner at the panel:
  - `fw47-recover-05`, passed: the owner unlocked the panel and held Lock and Dispense (6.3 s): `pair recovery state=opened`. `wellfed recover` connected, the window closed at its offer (`pair confirming path=recovery`), and the owner keyed in the six presses: `pair recovery state=entered`, `PLCFG1 keys change=enroll result=done enrolled=2`, and the session ended `joined`. The new key opened an Owner session and listed both clients; the bench key then revoked it.
  - `fw47-recover-04`, refused as it should be: only five presses registered (one of two Dispense presses in a row never reached the button service, `dropped=0`), and 60 s after the offer the feeder refused it as `unconfirmed`, enrolling nobody.
  - A 4.9 s hold, under the 5 s threshold, opened no window, and the connection was turned away (`ended reason=recovery`), as for a closed window.
  - Fixed after these runs: presses taken by a code count as panel activity, so the panel no longer relocks 30 s after the unlock while the code is keyed in. Code presses are logged under their path (`pair recovery keyed=N`).
- **Observed** (October 27), writes and a cancel during a feed: the sustained run's first cycle passed on hardware with writes during a feed and a cancel during the next. Power cuts with a feed reserved, and 4 s into a drive, each resolved as uncertain once at the next boot and never re-ran.
- **Observed** (October 22), settings: a change from generation 2978 to 2979 (lights on, volume 40) was saved and read back after a cold boot.
- **Observed** (October 22), Wi-Fi credentials: stored credentials joined after a cold boot with `source: stored`. With a wrong passphrase, stored attempts 1 to 5 failed, the build's credentials took over from attempt 6, and the feeder came online on attempt 7 (`source: fallback`). A clear returned to the build's credentials. Neither passphrase appeared in any capture.
- **Observed** (October 24), a live change to a different network: with a second hotspot on the bench host, a stored change took effect at the next join after the lab hotspot went away (`source: stored`, 15 s). A clear, then the lab hotspot's return, rejoined with the build's credentials (`source: build`, 20 s), all without a reboot.
- **Modeled** (October 26), feeds during handshakes: the virtual feeder completes a three-portion feed with a 70 ms hold every half second (`handshake_sized_holds_during_a_drive_leave_the_feed_to_complete`) and with 80 ms holds back to back, the owner stepped once between each (`back_to_back_handshake_holds_leave_the_feed_to_complete`). A 150 ms hold ends the feed `ObservationGap`, stopped and durable. The timing argument is static analysis plus simulation.
- **Modeled:** feeds staying silent while a tune plays or stalls, on the board's platform against the fake codec (`wellfed-fw-feeder/tests/manual_feed.rs`).
- **Modeled** (October 26): statistics, lifetime figures, feed traces, the debug log and the stock backup are host-tested. The trace ring and the stats store are tested under a power cut at every mutation. In simulation, `wellfed-feeder-sim`'s bank 0 served an owner the whole of bank 1 byte for byte over TCP, once bank 1 was left as stock leaves it, and bank 1 refused the read. The device conformance suite checks both platforms refuse it from bank 1. The stock backup has not yet run on the feeder.
- **Modeled** (October 26), build costs, from the `feeder` firstboot builds with every audit passing:
  - the debug log, against the same build without it: payload unchanged at 946,176 bytes; the loop's bound 5,140 → 5,108 of 8,192 (5,116 once rebased onto the SysTimer clock work); `wellfed` 2,808 → 2,808 of 4,096, `wellfed-key` 1,272 → 1,248 of 2,048, `wellfed-svc` 11,744 → 11,744 of 16,384, `wellfed-wdg` 1,452 → 1,420 of 2,048;
  - traces: `.bss` against `.ram_image2.bss` of 61,668 bytes in all, payload 995,328 bytes, the loop's bound 5,360 of 8,192 (its deepest path a feed's submission, not the trace), `wellfed-svc` at 11,744 of 16,384;
  - the stock backup: the page buffer grew from 432 to 768 bytes (336 bytes of `.bss`), payload 1,015,808 bytes, the loop's bound 5,412 of 8,192 (5,460 before the clock moved to the SysTimer).
- **Observed** (October 24), the resources table's first two columns: `feeder` v109 (before the device API, with the heap figure from the first product budget) and v119 (with it).
- **Observed** (October 24): `qa-20261024-01` ran from commit `ccf3e2d`. `ota-power-cuts` installs the `speaker` firmware, which serves no API, so the bench takes its UART confirmation.
- **Modeled:** the bench's `itest` scenario `feed-writes` (`--feeds`) writes from a session opened before the feed, against the scripted feeder. Its hardware run is the sustained-run entry above.
- **Modeled**, the rest of the host suite: a wrong recovery code is refused and enrolls nobody, one owner per window (`wellfed-client/tests/api.rs`); stale writes and retried ids; role limits; one handshake operation per pass, and during a feed only session handshakes, a stranger's refused unanswered and the rest held after it; a feed's id remembered from its queueing across sessions; readiness in the status and obeyed by a feed; no reboot during a feed; idle and handshake timeouts; the event ring's wrap, gaps, epoch and a power cut at every mutation (`wellfed-core/tests/event_log.rs`); the client's `Connection` (keepalive, silence, one answer or `Unanswered` per request, the event cursor across epochs, the welcome waiting for the ring); mDNS against `simple-dns`; IK against `snow`; the protocol's byte fixtures (`wellfed-proto/tests/fixtures`); the Wi-Fi store under a power cut at every mutation, reopening to the old or the new value and staying writable.
