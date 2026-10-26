# Firmwares

Each folder here is one firmware for the PLAF103, built on the Wellfed SDK (`src/sdk/wellfed-sdk`). Pick one at build time:

```sh
make -C src/firmware/vendor firstboot FIRMWARE=bringup VARIANT=buttons   # default: qa
```

| Firmware | What it adds |
| --- | --- |
| [`qa`](qa) | The default: a guided self-test of optics, power, buttons, indicators and speaker, ending in one `PLQA1 result` line ([QA guide](../../../docs/firmware/qa.md)) |
| [`bringup`](bringup) | One bring-up area per image variant; the normal image adds nothing. `optics`: the optics probe, once per boot. `buttons`: the panel buttons, raw lines and the SDK button service's gestures (press, holds, chords), logged for as long as the build runs. `adc`: the power inputs, read-only, sampled once a second. `speaker`: the startup chime and the tune player. `pulse`: the first supervised motor pulse. `stall` and `fail`: trial images KM0 must roll back |
| [`feeder`](feeder) | The product firmware: the PLAF103's `Platform` for the product's loop pass (`wellfed_feeder::feeder`), which the simulator runs too, and the feeding owner. So far the panel, whose child lock follows stock (5 s Lock hold, 30 s idle relock), shows on PA4 and takes a signed `lock` request, BLE setup mode (a Lock and Mic hold, then a phone's key and Wi-Fi, confirmed by Dispense), the device API, and the feeding owner, which holds the motor and emitters stopped and supervised (no feed is submitted yet); its loop task and the button service are its two tasks. Claims the ADC for the owner, which publishes power health in the runtime monitor's place. Links the device's BLE stack (`bluetooth = true`) and starts it only in setup mode |
| [`blink`](blink) | One task that blinks the connection indicator; the smallest firmware and the template for new ones |

The probes these firmwares run live once, in `src/sdk/wellfed-probes`, so a bring-up variant and QA step the same code. Each firmware owns the tasks that step them.

The bench still names the bring-up areas as firmwares: `wellfed-bench ota --build --firmware optics` builds `--firmware bringup --variant optics`, `--firmware trial --variant stall` builds `--firmware bringup --variant stall`, and `--firmware trial` alone the normal image. A bring-up area is a variant, so it builds only with the `firstboot` profile (`make -C src/firmware/vendor firstboot FIRMWARE=bringup VARIANT=optics`); the `bringup` make target is the unrelated SDK profile of that name, not this firmware. Every variant's source identity covers the whole crate, so a change to one area is a new identity for every bring-up image.

## What the SDK runtime guarantees

A firmware adds tasks; it does not own the device. The runtime always runs first and keeps these whatever the firmware does:

- **Startup checks.** Motor STOP and infrared OFF are requested and read back, the synthetic core diagnostic runs, and the indicators are verified LOW before any firmware code starts. A failure stops startup.
- **Recovery.** Bank 0 holds the fallback: the `feeder` build, which runs its firmware there too (it feeds and serves the device API) and is the only bank that installs updates into bank 1. A failed update or trial lands on it. A unit whose bank 1 holds a firmware that serves no device API (the bring-up firmwares) takes no offer: it leaves that build through KM0's trial or a UART install.
- **Supervision and rollback.** The watchdog feeder stops feeding when a runtime task hangs, outputs drift from stopped, or a trial image fails or never confirms. KM0 counts the reset and rolls a trial back to the previous build.
- **Confirmation, settings and updates.** The runtime confirms a healthy build on bank 1, runs the settings writer (the product's, which every firmware links from `wellfed-feeder-runtime`, so a swap keeps the owner's settings, keys and history), and runs the update stage, which takes offers through a firmware's device API (the `feeder`'s), checks their signatures on its own stack and writes bank 1 from bank 0.
- **Task slots.** The SDK has eight task slots, and the runtime holds three on either bank (the watchdog feeder, the startup task, and one service task for power, confirmation, settings and the update stage), so a firmware gets **four tasks**, counting the SDK services it starts (the button service is one), and one slot stays spare. `spawn` refuses a fifth (`PLFW1 error firmware_tasks=full`), and the build's stack audit refuses an image that links more. When those runtime stages were separate tasks, a firmware that took one of their slots could no longer save settings or see an update offer (`fw44-simon-01`).
- **Panics.** Interrupts are masked and outputs forced stopped before halting.

Not yet guaranteed: the feeder does not watch firmware tasks' liveness, so a hung firmware task does not reset the device by itself.

## Writing a firmware

A firmware is a `no_std` crate in `src/firmware/wellfed/<name>`, with the package name `wellfed-fw-<name>`. It is a KM4 staticlib with a `km4` feature that enables `wellfed-sdk/km4` and `wellfed-feeder-runtime/km4`. It links `wellfed-feeder-runtime` (`use wellfed_feeder_runtime as _;`): the runtime's settings writer and Wi-Fi join call the stores it defines, and an image without it does not link. Copy `blink`'s `Cargo.toml` and change the name.

Its `[package.metadata.wellfed]` table is all the builder knows about it (`wellfed-build list` prints it):

```toml
[package.metadata.wellfed]
devices = ["plaf103"]                       # boards under src/firmware/devices/
bluetooth = true                            # link the device's BLE stack (default false)
policies = { optics = "fw13-optics-v2" }    # startup policies of the normal image, if any

[package.metadata.wellfed.variants.loud]   # an opt-in image variant: the Cargo feature `loud`
summary = "What the variant changes"
policies = {}                               # replace the normal image's policies
```

The startup audit checks each policy against the image's markers, so declare only the policies the image really carries. A variant whose policies name a `motion` policy drives the motor (`list` marks it `"motion": true`); motion builds stay a manual, attended step. Only a variant may name one: the builder refuses a motion policy on the normal image, because only first-boot builds record policies, and a motion image must always show its policy to the bench's attended-motion gate. Build a variant with `--variant <name>` (`VARIANT=<name>` for make); only the `firstboot` profile takes one.

Its target-only module defines the start hook and creates its tasks:

```rust
use wellfed_feeder_runtime as _;

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
- Drive hardware through `wellfed_sdk::hal`'s owners (the board, indicators, audio, buttons). Indicator requests are refused unless outputs read back stopped. The raw pin, watchdog and flash writers are `unsafe`; only a bring-up firmware takes them, from `wellfed_sdk::hal::bringup`, with a `// SAFETY:` comment saying why it owns what it writes.
- Log records as `PL<NAME>1 …` lines, through `wellfed_probes::log` or `hal::startup::Report` as `blink` does.
- To run a bench probe, depend on `wellfed-probes` and step its `Probe` from your own task, as `bringup`'s `optics` variant does. A new bring-up area is a new variant of `bringup` (a feature, a `variants` entry and a module under `src/app/`), not a new firmware; the crate builds one variant per image.

The build's stack audit finds every `spawn(c"name", WORDS, priority, entry)` call in the firmware's `src/`, so keep that call shape, with the stack size as a literal or a `const NAME: u16` and a non-generic entry function (it refuses a task it cannot resolve, such as `step::<S>`), and keep the call in the firmware crate rather than a shared library. Each task's static bound must leave a quarter of its stack free. A firmware gets four tasks in all (`wellfed_sdk::FIRMWARE_TASKS`), its SDK services included; the runtime holds three of the eight slots and one stays spare (see the runtime guarantees above).

Host tests run the firmware's logic against the SDK fake (`wellfed_sdk::hal::sdk::fake`); see `blink/tests/` and `qa/tests/`.
