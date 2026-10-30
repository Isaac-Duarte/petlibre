# Buttons

The feeder's panel has four buttons. The firmware turns raw presses into gestures (a press, a hold, a two-button chord) and hands each one to the single task that decides what it does. That task is also the one that asks the feeding owner for a feed, so a button press and an app request can never feed twice.

## The panel

| Button | Pin |
| --- | --- |
| Reset | PB4 |
| Mic | PB5 |
| Dispense | PB6 |
| Lock | PB7 |

Every button is active LOW, with pull-ups on the board and no pad pull. Seen from the front they run Lock, Dispense, Mic, Reset from left to right. What each button does is product policy; see the [roadmap](../roadmap.md).

## Gestures

`wellfed_core::buttons::Engine` takes the set of pressed buttons on each poll and reports four kinds of event:

- **Down.** A button joined the gesture. The chord is every button pressed since the gesture began, so pressing a second button while holding the first grows the chord.
- **Hold.** The whole chord has stayed pressed past threshold `level` (counted from 1), measured from when the chord last grew. Each threshold fires once.
- **Up.** Every button is released. It carries the chord, the highest level reached (0 for a short press) and the time since the first press.
- **Debounce** happens first: each line changes only after reading its new level for `debounce_ms` (30 ms) without a break.

Letting go of part of a chord ends its holds, so a two-button hold never turns into a one-button one. A late poll reports only the highest level reached.

A configuration has up to three hold thresholds, in rising order, each longer than the debounce. `Config::BENCH` (2 s, 5 s) is for bench checks. Stock's durations and each feature's own thresholds are product policy.

## The service

A firmware opts in from its start hook:

```rust
wellfed_sdk::buttons::start(Config::BENCH);
// In the one task that owns what the buttons do:
while let Some(gesture) = wellfed_sdk::buttons::next() { /* … */ }
```

- **Task.** `start` creates `wellfed-key`, which polls every 5 ms (stack bound 728 of 2048 B). A firmware that never calls it links none of it. It is one of the four tasks a firmware gets ([task slots](../../src/firmware/wellfed/README.md)).
- **Queue.** Gestures go to an eight-entry queue with one reader. A full queue drops new gestures and counts them; it never overwrites unread ones.
- **UART.** Each gesture is also a `PLKEY1` record, each with `dropped=N`: `open inputs=1 pressed=none`, `down chord=dispense+lock`, `hold chord=reset level=2`, `up chord=reset level=2 held_ms=6671`. The `virtual-panel` bench variant adds `virtual press=dispense hold_ms=150` for each line it reads from the UART and `virtual refused` for one it does not ([virtual panel](../devices/plaf103/bench.md#virtual-panel)).
- **Ownership.** The reader is the task that owns what a gesture does. A Dispense press is submitted to the feeding owner by that task, the same way a network feed request is.

The stack audit reads its task table from the runtime and from every SDK service that creates a task (`RUNTIME_SOURCE` and `SERVICE_SOURCES` in the builder). A test fails if one is missed.

## The raw probe

The `buttons` bring-up firmware, and the [QA firmware](qa.md)'s guided checks, read the lines without the service. They configure PB4–PB7 as stock does (inputs, no pad pull) and log `PLBTN1` records:

- `open`, with PB4..PB7 pad control before and after, the input direction and the first levels;
- one `change` per settled change: levels before and after as port B bits 4..7, edges, bounce from first to last edge, and `settled=0` if still changing after 1 s;
- `idle` every 30 s without a change.

Every record carries `stopped`.

## Evidence

- **Pins and levels.** Observed (October 23, UART, owner pressing): the raw probe confirmed over the air (v66). With the pad pulls off the lines idle high, so the board has its own pull-ups. Pressing one at a time gave the pins above, each LOW while pressed. All 62 edges were single, with no bounce at the 2 ms poll, and presses lasted 0.1–0.7 s. Two-button presses read as both lines low. The left-to-right order is the owner's observation (October 24).
- **Gestures.** Observed (October 23, owner pressing): the `buttons` firmware (v87, `fw20-keys-01`) ran the service beside the raw probe. Two captures showed every gesture the service makes, each read back through its queue, with none dropped:
  - `fw20-keys-02`: a short press; a 1.7 s press with no hold; a 3.9 s hold at level 1; two chords that grew from one button to two (`dispense+lock`, `mic+dispense`); and a chord whose Lock was let go 2.6 s in, after its level-1 hold, with Dispense held 2.5 s longer and no further hold.
  - `fw20-keys-03`: a 6.7 s hold reached level 2.
  - Two buttons pressed "together" landed 6 ms apart and still read as one chord.
- **Dispense through the owner.** Observed (October 26, owner pressing): Dispense on an unlocked panel fed one portion; on a locked panel it did nothing (`fw17-dispense-20261026-01`).
- **Host.** Modeled: the service against the SDK fake, and the raw probe against it (bounce, unsettled lines, idle records).
