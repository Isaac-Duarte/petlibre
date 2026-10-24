# Buttons

Status: physical (October 23, 2026), owner pressing. The `buttons` firmware (v87, `fw20-keys-01`) ran the service beside the raw probe. Two captures showed every gesture the service makes, each read back through its queue, with none dropped:
- `fw20-keys-02`:
  - a short press;
  - a 1.7 s press with no hold;
  - a 3.9 s hold at level 1;
  - two chords that grew from one button to two (`dispense+lock`, `mic+dispense`);
  - a chord whose Lock was let go 2.6 s in, after its level-1 hold, with Dispense held 2.5 s longer and no further hold.
- `fw20-keys-03`: a 6.7 s hold reached level 2.

Two buttons pressed "together" landed 6 ms apart and still read as one chord. The service is also host-tested against the SDK fake.

## The panel

Four buttons on PB4–PB7, active LOW, with board pull-ups and no pad pull (FW20, physical): Reset = PB4, Mic = PB5, Dispense = PB6, Lock = PB7. With a 2 ms poll, none of 62 edges bounced, and presses lasted 0.1–0.7 s.

## Gestures

`wellfed_core::buttons::Engine` takes the pressed set each poll:
- **Debounce:** each line changes only after reading its new level for `debounce_ms` (30 ms) without a break.
- **Down:** a button joined the gesture. The chord is every button pressed since the gesture began, so a second button pressed while the first is held grows the chord.
- **Hold:** the whole chord has stayed pressed past threshold `level` (1-based), counted from when the chord last grew. Each threshold fires once. Letting go of part of a chord ends its holds, so a two-button hold never turns into a one-button one. A late poll reports only the highest level reached.
- **Up:** every button is released. It carries the chord, the highest level reached (0 for a short press) and the time since the first press.

Up to three hold thresholds, in rising order and each longer than the debounce. `Config::BENCH` (2 s, 5 s) is for bench checks. Stock's durations are FP01's to record, and the feature cards set their own.

## The service

A firmware opts in from its start hook:

```rust
wellfed_sdk::buttons::start(Config::BENCH);
// In the one task that owns what the buttons do:
while let Some(gesture) = wellfed_sdk::buttons::next() { /* … */ }
```

- **Task:** `start` creates `wellfed-key`, which polls every 5 ms (stack bound 728 of 2048 B). A firmware that never calls it links none of it. It is one of the firmware's two tasks ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)).
- **Queue:** gestures go to an eight-entry queue with one reader. A full queue drops new gestures and counts them, never overwriting unread ones.
- **UART:** each gesture is also a `PLKEY1` record: `open inputs=1 pressed=none`, `down chord=dispense+lock`, `hold chord=reset level=2`, `up chord=reset level=2 held_ms=6671`, each with `dropped=N`.
- **Ownership:** the reader is the task that owns what a gesture does. Once feeding exists (FW17), a Dispense press is submitted to the feeding owner by that task, the same way a network feed request will be, so the two cannot double-feed.

The stack audit reads its task table from the runtime and every SDK service that creates a task (`RUNTIME_SOURCE` and `SERVICE_SOURCES` in the builder), and a test fails if one is missed.
