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

Start from a copy of `blink` and change the name. A firmware is a `no_std` crate in `src/firmware/wellfed/<name>`, with the package name `wellfed-fw-<name>`. It is a KM4 staticlib with a `km4` feature that enables `wellfed-sdk/km4` and `wellfed-feeder-runtime/km4`, and it depends on `wellfed-feeder-runtime`: the runtime's settings writer and Wi-Fi join call the record stores that crate defines.

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

Its target-only module (`app.rs`, behind `#[cfg(target_os = "none")]`) defines the start hook and the tasks, with no `unsafe`:

```rust
use wellfed_sdk::{LogRecord as _, Slot, Startup};

#[wellfed_sdk::start(runtime = wellfed_feeder_runtime)]
fn start() -> Startup {
    wellfed_sdk::spawn(blink_task);
    // Nothing here holds the KM0 flash handshake: the runtime may confirm.
    Startup::Done
}

const BLINK_STACK_WORDS: u16 = 1024;

/// Touched only by the blink task.
static BLINKER: Slot<Blinker> = Slot::new();

#[wellfed_sdk::task(name = "wellfed-blink", stack = BLINK_STACK_WORDS, priority = 2)]
fn blink_task() -> u32 {
    let Some(mut blinker) = BLINKER.enter_or_init(Blinker::default) else {
        return 0;
    };
    let step = blinker.step();
    if let Some(record) = step.record {
        record.log();
    }
    step.delay_ms
}
```

- `#[start]` runs once in the normal role, after the startup checks passed. It must create tasks and return, never block. `runtime` names the crate whose record stores the runtime calls, which links it.
- It returns `Startup::Done`, or `Startup::Pending` while startup work holds the KM0 flash handshake (an optics or pulse probe). The task holding it then reports through `wellfed_sdk::startup_done`, as `bringup`'s optics task does with `startup_done(step.delay_ms == 0)` on each step. The runtime does not confirm the build, sample power or join Wi-Fi before that.
- `#[task]` makes a step function into a `wellfed_sdk::Task` of the same name for `spawn`. The step returns the milliseconds to sleep before the next step, or zero to end the task. Keep each task's state in a `wellfed_sdk::Slot` static owned by that task.
- Drive hardware through `wellfed_sdk::hal`'s owners (the board, indicators, audio, buttons). Indicator requests are refused unless outputs read back stopped. The raw pin, watchdog and flash writers are `unsafe`; only a bring-up firmware takes them, from `wellfed_sdk::hal::bringup`, with a `// SAFETY:` comment saying why it owns what it writes.
- Log records as `PL<NAME>1 …` lines. `#[derive(wellfed_sdk::Record)]` writes a record's line from its fields (see `blink::Record`): `#[record(tag = "PLBLK1")]` on the type, `#[record("level")]` on each variant for the text before the fields, and on a field `rename`, `with = path`, `fmt = "02x"` or `skip`. A `bool` is written `0` or `1`. `record.log()` (the `LogRecord` trait) then writes the whole line. A line the attributes cannot express gets a hand-written `Display` and `wellfed_sdk::log(format_args!("PLXXX1 {record}\r\n"))`. Either way a line longer than the 192-byte buffer is dropped, never truncated.
- To run a bench probe, depend on `wellfed-probes` and step its `Probe` from your own task, as `bringup`'s `optics` variant does. A new bring-up area is a new variant of `bringup` (a feature, a `variants` entry and a module under `src/app/`), not a new firmware; the crate builds one variant per image.

The build's stack audit reads every `#[wellfed_sdk::task(name = "…", stack = WORDS, priority = N)]` in the firmware's `src/` and walks the linked entry of that name. The attribute keeps that shape: its arguments in that order, the stack size as a literal or a `const NAME: u16` in the firmware's sources, and a plain, non-generic function, so declare the task in the firmware crate rather than a shared library. The audit refuses a task it cannot read, a `spawn` other than `wellfed_sdk::spawn(entry)` of a task declared in the firmware's own sources (one from a library, or behind a renamed attribute, would link unread), and a firmware that creates a task with `task_create` itself. Each task's static bound must leave a quarter of its stack free. A firmware gets four tasks in all (`wellfed_sdk::FIRMWARE_TASKS`), its SDK services included; the runtime holds three of the eight slots and one stays spare (see the runtime guarantees above).

Host tests run the firmware's logic against the SDK fake (`wellfed_sdk::hal::sdk::fake`); see `blink/tests/` and `qa/tests/`.
