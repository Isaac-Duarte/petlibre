# Firmwares

Each folder here is one firmware for the PLAF103, built on the Wellfed SDK (`src/sdk/wellfed-sdk`). This page lists them, says what the SDK runtime guarantees whichever one runs, and shows how to write your own. Building is covered in [firmware builds](../../../docs/development/firmware-builds.md).

## The firmwares

| Firmware | What it adds |
| --- | --- |
| [`qa`](qa) | The default: a guided self-test of optics, power, buttons, indicators and speaker, ending in one `PLQA1 result` line ([QA guide](../../../docs/firmware/qa.md)) |
| [`feeder`](feeder) | The product firmware every owner runs. See [below](#the-feeder-firmware) |
| [`bringup`](bringup) | One bring-up area per image variant; the normal image adds nothing. See [below](#bring-up-variants) |
| [`blink`](blink) | One task that blinks the connection indicator. The smallest firmware, and the template for new ones |

Pick one at build time:

```sh
make -C src/firmware/vendor firstboot FIRMWARE=bringup VARIANT=buttons   # default: qa
```

The probes these firmwares run live once, in `src/sdk/wellfed-probes`, so a bring-up variant and QA step the same code. Each firmware owns the tasks that step them.

### The feeder firmware

`feeder` is the PLAF103's `Platform` for the product's loop pass (`wellfed_feeder::feeder`), which the simulator runs too, plus the feeding owner. It provides:

- **The panel.** Its child lock follows stock: a 5 s Lock hold, and a 30 s idle relock. The panel shows on PA4 and takes a signed `lock` request.
- **BLE setup mode.** A Lock and Mic hold starts it. A phone then sends its key and Wi-Fi, confirmed by Dispense. The firmware links the device's BLE stack (`bluetooth = true`) and starts it only in setup mode.
- **The device API**, with its sockets and mDNS.
- **The feeding owner**, which holds the motor and emitters stopped and supervised between feeds, and runs the feeds the device API and the Dispense button ask for.
- **Power health.** It claims the ADC for the owner, which publishes power health in the runtime monitor's place.

Its two tasks are its loop task and the button service. Its test variants (`entropy`, `owner-stall`, `held-stall`, `gap-probe`, `feed-stall`, `stop-fail`, `owner-fail`, `feed-jam`, `sensor-fault` and `twin`) are each described in its `Cargo.toml`. They exist for the bench to show the supervision, rollback and fault handling at work.

### Bring-up variants

| Variant | What it runs |
| --- | --- |
| `optics` | The optics probe, once per boot |
| `buttons` | The panel buttons: raw lines and the SDK button service's gestures (press, holds, chords), logged for as long as the build runs |
| `adc` | The power inputs, read-only, sampled once a second |
| `speaker` | The startup chime and the tune player |
| `pulse` | The first supervised motor pulse |
| `stall`, `fail` | Trial images that KM0 must roll back |

A bring-up area is a variant, so it builds only with the `firstboot` profile:

```sh
make -C src/firmware/vendor firstboot FIRMWARE=bringup VARIANT=optics
```

> [!NOTE]
> The `bringup` make target is the unrelated SDK profile of that name, not this firmware.

The bench still names the bring-up areas as firmwares. `wellfed-bench ota --build --firmware optics` builds `--firmware bringup --variant optics`. `--firmware trial --variant stall` builds `--firmware bringup --variant stall`, and `--firmware trial` alone builds the normal image.

Every variant's source identity covers the whole crate. So a change to one area is a new identity for every bring-up image.

## What the SDK runtime guarantees

A firmware adds tasks; it does not own the device. The runtime always runs first and keeps these guarantees whatever the firmware does.

- **Startup checks.** Motor STOP and infrared OFF are requested and read back. The synthetic core diagnostic runs, and the indicators are verified LOW, before any firmware code starts. A failure stops startup.
- **Recovery.** Bank 0 holds the fallback: the `feeder` build. It runs its firmware there too, so it feeds and serves the device API, and it is the only bank that installs updates into bank 1. A failed update or trial lands on it. If bank 1 holds a firmware that serves no device API, such as the bring-up firmwares, the unit takes no offer. It leaves that build through KM0's trial or a UART install.
- **Supervision and rollback.** The watchdog feeder stops feeding when a runtime task hangs, when outputs drift from stopped, or when a trial image fails or never confirms. KM0 counts the reset and rolls a trial back to the previous build. A confirmed build that keeps resetting soon after each boot (a reset storm) gives way the same way. Bank 0 instead holds its outputs stopped for a bounded time, then feeds again ([reset storms](../../../docs/firmware/ota-design.md#reset-storms)).
- **Confirmation.** The runtime confirms a healthy build on bank 1. A firmware that runs a feeding owner (`claim_feeding`) is confirmed only once that owner has opened on its journal with its checks passing (`report_feeding`, [what confirmation proves](../../../docs/firmware/ota-design.md#what-confirmation-proves)).
- **Settings.** The runtime runs the settings writer. It is the product's, which every firmware links from `wellfed-feeder-runtime`, so a swap keeps the owner's settings, keys and history.
- **Updates.** The runtime runs the update stage. It takes offers through a firmware's device API (the `feeder`'s), checks their signatures on its own stack, and writes bank 1 from bank 0.
- **Task slots.** The SDK has eight task slots. The runtime holds three on either bank: the watchdog feeder, the startup task, and one service task for power, confirmation, settings and the update stage. One slot stays spare. So a firmware gets **four tasks**, counting the SDK services it starts (the button service is one). `spawn` refuses a fifth (`PLFW1 error firmware_tasks=full`), and the build's stack audit refuses an image that links more.
- **Panics.** Interrupts are masked and outputs forced stopped before halting.

Not yet guaranteed: the runtime does not watch a firmware task's liveness in general, so a hung firmware task does not by itself reset the device. A firmware that claims the feeding owner is the exception: a lapse in the owner's beat ends in a watchdog reset, as the `feeder`'s `owner-stall` variant shows.

## Writing a firmware

1. Copy `blink` to `src/firmware/wellfed/<name>`, and name the package `wellfed-fw-<name>`.

   A firmware is a `no_std` KM4 staticlib. It has a `km4` feature that enables `wellfed-sdk/km4` and `wellfed-feeder-runtime/km4`. It depends on `wellfed-feeder-runtime`, because the runtime's settings writer and Wi-Fi join call the record stores that crate defines.

2. Fill in its `[package.metadata.wellfed]` table. It is all the builder knows about the firmware, and `wellfed-build list` prints it:

   ```toml
   [package.metadata.wellfed]
   devices = ["plaf103"]                       # boards under src/firmware/devices/
   bluetooth = true                            # link the device's BLE stack (default false)
   policies = { optics = "fw13-optics-v2" }    # startup policies of the normal image, if any

   [package.metadata.wellfed.variants.loud]   # an opt-in image variant: the Cargo feature `loud`
   summary = "What the variant changes"
   policies = {}                               # replace the normal image's policies
   ```

3. Write the start hook and the tasks in its target-only module (`app.rs`, behind `#[cfg(target_os = "none")]`), with no `unsafe`:

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

4. Build it with `make -C src/firmware/vendor firstboot FIRMWARE=<name>`, and add a variant with `VARIANT=<name>` (`--variant <name>` for `wellfed-build`). Only the `firstboot` profile takes a variant.

   You should see the build pass its stack and startup audits.

### Startup policies

The startup audit checks each policy against the image's markers, so declare only the policies the image really carries.

A variant whose policies name a `motion` policy drives the motor, and `list` marks it `"motion": true`. Motion builds stay a manual, attended step. Only a variant may name one: the builder refuses a motion policy on the normal image. That is because only first-boot builds record policies, and a motion image must always show its policy to the bench's attended-motion gate.

### The start hook and tasks

- `#[start]` runs once in the normal role, after the startup checks passed. It must create tasks and return, never block. `runtime` names the crate whose record stores the runtime calls, which links it.
- It returns `Startup::Done`, or `Startup::Pending` while startup work holds the KM0 flash handshake (an optics or pulse probe). The task holding it then reports through `wellfed_sdk::startup_done`, as `bringup`'s optics task does with `startup_done(step.delay_ms == 0)` on each step. Until then the runtime does not confirm the build, sample power or join Wi-Fi.
- `#[task]` turns a step function into a `wellfed_sdk::Task` of the same name, for `spawn`. The step returns the milliseconds to sleep before the next step, or zero to end the task. Keep each task's state in a `wellfed_sdk::Slot` static owned by that task.

### Hardware

Drive hardware through `wellfed_sdk::hal`'s owners: the board, indicators, audio and buttons.

- The bring-up probe's indicator requests (`leds::request`) are refused unless outputs read back stopped. The product's [indicator service](../../../docs/firmware/indicators.md) drives them through `leds::show`, which a running motor does not refuse.
- The raw pin, watchdog and flash writers are `unsafe`. Only a bring-up firmware takes them, from `wellfed_sdk::hal::bringup`, with a `// SAFETY:` comment saying why it owns what it writes.
- To run a bench probe, depend on `wellfed-probes` and step its `Probe` from your own task, as `bringup`'s `optics` variant does.
- A new bring-up area is a new variant of `bringup`, not a new firmware: a feature, a `variants` entry and a module under `src/app/`. The crate builds one variant per image.

### Log records

Log records as `PL<NAME>1 …` lines.

- `#[derive(wellfed_sdk::Record)]` writes a record's line from its fields (see `blink::Record`). Put `#[record(tag = "PLBLK1")]` on the type, and `#[record("level")]` on each variant for the text before the fields. On a field, use `rename`, `with = path`, `fmt = "02x"` or `skip`. A `bool` is written `0` or `1`.
- `record.log()` (the `LogRecord` trait) then writes the whole line.
- A line the attributes cannot express gets a hand-written `Display` and `wellfed_sdk::log(format_args!("PLXXX1 {record}\r\n"))`.
- Either way, a line longer than the 192-byte buffer is dropped, never truncated.
- A firmware built with the SDK's `log-tail` feature (the `feeder`) also keeps the newest lines in a 4 KiB RAM tail, which the device API streams to an owner ([debug log](../../../docs/firmware/device-api.md#debug-log)).

### What the stack audit needs

The build's stack audit reads every `#[wellfed_sdk::task(name = "…", stack = WORDS, priority = N)]` in the firmware's `src/`, and walks the linked entry of that name. So the attribute keeps that shape:

- its arguments in that order;
- the stack size as a literal, or as a `const NAME: u16` in the firmware's sources;
- a plain, non-generic function. Declare the task in the firmware crate, not in a shared library.

The audit refuses:

- a task it cannot read;
- a `spawn` other than `wellfed_sdk::spawn(entry)` of a task declared in the firmware's own sources. One from a library, or behind a renamed attribute, would link unread;
- a firmware that creates a task with `task_create` itself.

Each task's static bound must leave a quarter of its stack free. A firmware gets four tasks in all (`wellfed_sdk::FIRMWARE_TASKS`), its SDK services included; see the task slots above.

### Tests

Host tests run the firmware's logic against the SDK fake (`wellfed_sdk::hal::sdk::fake`). See `blink/tests/` and `qa/tests/`.
