# Firmwares

Each folder here is one firmware for the PLAF103, built on the Wellfed SDK (`src/sdk/wellfed-sdk`). Pick one at build time:

```sh
make -C src/firmware/vendor firstboot FIRMWARE=blink   # default: reference
```

| Firmware | What it adds |
| --- | --- |
| [`reference`](reference) | The diagnostic image's optics probe, startup chime and speaker stream, its bench probes, and the opt-in FW16 motion and FW11 trial variants |
| [`blink`](blink) | One task that blinks the connection indicator; the smallest firmware and the template for new ones |

## What the SDK runtime guarantees

A firmware adds tasks; it does not own the device. The runtime always runs first and keeps these whatever the firmware does:

- **Startup checks.** Motor STOP and infrared OFF are requested and read back, the synthetic core diagnostic runs, and the indicators are verified LOW before any firmware code starts. A failure stops startup.
- **Recovery.** Booted from bank 0, only the runtime's updater runs, never firmware code, so a broken firmware can always be replaced.
- **Supervision and rollback.** The watchdog feeder stops feeding when a runtime task hangs, outputs drift from stopped, or a trial image fails or never confirms. KM0 counts the reset and rolls a trial back to the previous build.
- **Confirmation, settings and updates.** The runtime confirms a healthy build, runs the settings writer, and checks for update offers, so every firmware stays updatable over the air.
- **Panics.** Interrupts are masked and outputs forced stopped before halting.

Not yet guaranteed: the feeder does not watch firmware tasks' liveness, so a hung firmware task does not reset the device by itself.

## Writing a firmware

A firmware is a `no_std` crate in `src/firmware/wellfed/<name>`, with the package name `wellfed-fw-<name>`. It is a KM4 staticlib with a `km4` feature that enables `wellfed-sdk/km4`. Copy `blink`'s `Cargo.toml` and change the name.

Its `[package.metadata.wellfed]` table is all the builder knows about it (`wellfed-build list` prints it):

```toml
[package.metadata.wellfed]
devices = ["plaf103"]                       # boards under src/firmware/devices/
policies = { optics = "fw13-optics-v2" }    # startup policies of the normal image, if any

[package.metadata.wellfed.variants.loud]   # an opt-in image variant: the Cargo feature `loud`
summary = "What the variant changes"
policies = {}                               # replace the normal image's policies
```

The startup audit checks each policy against the image's markers, so declare only the policies the image really carries. A variant whose policies name a `motion` policy drives the motor (`list` marks it `"motion": true`); motion builds stay a manual, attended step. Build a variant with `--variant <name>` (`VARIANT=<name>` for make); only the `firstboot` profile takes one.

Its target-only module defines the start hook and creates its tasks:

```rust
wellfed_sdk::firmware!(start);

fn start() {
    // Nothing here holds the KM0 flash handshake: the runtime may confirm.
    wellfed_sdk::startup_done(true);

    // SAFETY: the step ignores its context and keeps its state in statics.
    let created = unsafe { wellfed_sdk::spawn(c"wellfed-blink", 1024, 2, blink_task) };
}
```

- `start` runs once in the normal role, after the startup checks passed. It must create tasks and return, never block.
- A task is a step function. It returns the milliseconds to sleep before the next step, or zero to end the task. Keep each task's state in a `wellfed_sdk::Slot` static owned by that task.
- Call `startup_done(true)` once the firmware has finished any startup work that holds the KM0 flash handshake. The runtime does not confirm the build before then.
- Drive hardware through `wellfed_sdk::hal`. Indicator requests are refused unless outputs read back stopped.
- Log records as `PL<NAME>1 …` lines through `hal::startup::Report`, as `blink` does.

The build's stack audit finds every `spawn(c"name", WORDS, priority, entry)` call in the firmware's `src/`, so keep that call shape, with the stack size as a literal or a `const NAME: u16`. Each task's static bound must leave a quarter of its stack free.

Host tests run the firmware's logic against the SDK fake (`wellfed_sdk::hal::sdk::fake`); see `blink/tests/`.
