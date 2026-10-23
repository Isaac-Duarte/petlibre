# Region-scoped UART development updates

This is a separate experimental path for the tested PLAF103 bank-1 layout. It accepts complete, frozen source/target images and updates their required application sectors in an already established supervised helper session. The pinned stock-recovery writer retains its original restrictions.

It is not replacement OTA, automatic rollback, a boot controller, or a partial-write resume mechanism. The factory guards still require the original research baseline; another unit's backup is not automatically compatible.

## Review and scope

`review-development-image.py` freezes either the exact stock OTA or a reviewed `plaf103-firstboot` SDK build. Build review checks the current source snapshot, SDK revision/overlays, artifacts/archives, both core packages, linked mains/code, RAM parking/hooks, stock pulls, and modeled factory mapping. Frozen source reviews remain useful after later source changes. Freezing an arbitrary new application does not establish its behavior or physical acceptance.

Offline build/review commands use new output directories:

```sh
.venv/bin/python src/tools/flash/review-development-image.py stock \
  --output private/verification/stock-review
make -C src/firmware/vendor plaf103-firstboot
.venv/bin/python src/tools/flash/review-development-image.py build \
  --build private/firmware-build/plaf103-firstboot \
  --output private/verification/development-review
cargo run --locked -p wellfed-flash -- update --help
```

The [Rust client](rust-flash.md) runs the same inspect, prepare and execute sequence as `wellfed-flash update`; it has no hardware acceptance yet. These commands do not open a serial port. Build snapshots copied from the research archive can be stale against Wellfed's changed source/input hashes. Rebuild and freeze new targets rather than editing manifests or substituting expected hashes. Retain a frozen source matching the actually installed application.

## Inspection, planning, and execution

The CLI exposes separate `inspect`, `prepare`, and `execute` modes. Supply explicit baseline, frozen source/target directories, port, pinned ROM report, fresh bounded-read report, and new private output locations. Preparation binds the complete inspected source region, report and tool hashes into a plan. Execution additionally requires that exact reviewed plan and its SHA-256. Reports and companion serial/read files must remain together.

Writable bank 1 starts at `0x186000` and cannot extend beyond `0x2d6000`. Each acquisition reads the union of the old/new application extents plus the 24 KiB factory range and the other bank's 4 KiB first sector. The current application must exactly match the frozen source, and the factory range must match the pinned baseline.

The other bank's first sector must be in a known state, and its hash is pinned in the plan, so it must stay byte-identical across every acquisition. For bank 1 that means bank 0 is either the pinned baseline (an invalidated marker) or a valid image. Until October 2026 this path required bank 0 to be invalid. The FW09 finding that the factory loader prefers bank 1 and falls back to bank 0 ([bank policy](../firmware/bank-policy.md)) made a valid bank 0 the safer case: an interrupted bank-1 update then boots bank 0 rather than waiting forever.

`--bank 0` updates bank 0 (`0x6000`–`0x156000`, the same reviewed footprint) under its own transaction kind. Bank 1 must be valid throughout, so the unit always has a bootable bank while bank 0's marker is withheld. Program packets are bounded to the selected bank in the updater itself, and the factory range `0x0`–`0x6000` is never a write target in either mode. Bank 0 may omit `--source`: bank 0 must then still be exactly the pinned baseline's bytes over the target's extent, with an invalid marker, and the target overwrites it with its marker last.

`wellfed-flash give-way` (which replaced `src/tools/flash/uart-give-way.py`) clears bank 1's first marker byte with one 1 KiB program frame, so the loader falls back to bank 0. It runs only when bank 0 holds exactly a frozen, reviewed image, read in full. The frame rewrites the other 1023 bytes unchanged, since NOR programming only clears bits, which is what stock's activation does. A development update with `--restore-marker` then restores bank 1. It accepts only the source with its first marker byte cleared, rewrites that one sector with the marker withheld, and commits the marker last.

For changed images, execution verifies preimage, staged, and activated application regions and guards. It stages the marker sector first with eight erased bytes, then only other changed sectors, with status checks, durable intent, and exact sector readback. Whole-region verification precedes one 1 KiB no-erase marker commit, followed by final verification. A shorter target retains and verifies the old tail; a larger target explicitly includes additional sectors. No-op plans perform one acquisition and no writes.

Readiness synchronization follows durable erase-intent fsync, immediately before transmission. This ports the fix for journal stalls crossing the helper's idle-ready interval. Missing ACKs, protocol errors, or drift stop terminally; there is no automatic resume, replay, fallback write, or boot command.

`--interrupted` finishes a stopped development update from a fresh inspection, never from the stopped journal. It requires the marker sector to be staged, and every other sector to be exactly its source, its staged target or erased. One sector may instead hold the staged target's first one to three 1024-byte program frames followed by erased flash, the only state a program stopped after its erase leaves; it is erased and rewritten. A partial erase, a second partial program or any other content is refused. It then stages the remaining sectors and commits the marker with the same checks as a normal update. The flag is bound into the inspection, plan and execution, so a normal inspection cannot prepare an interrupted plan, nor the reverse.

Settings/audio/log areas outside the declared application footprint are not reread during each development update. Their preservation relies on the bounded mutation sequence, giving narrower evidence than a full-flash preservation audit. Full-flash audits remain appropriate for layout changes, recovery investigations, or unexplained drift.

## Evidence and remaining work

Physical changed-build updates and their subsequent boots were demonstrated. One small-image iteration took approximately **2m22s** for inspection plus execution, excluding build, manual setup, and normal boot. Timing depends on image changes and hardware; a small source edit can move many linked sectors.

The conservative full-image stock restoration and subsequent stock 1.1.15 boot were completed separately. Stock return through this newer updater remains untested; do not infer it from emulator success or the conservative restoration. Power-cut and failed-boot acceptance remain open. Neither a feature-board card nor a copied historical plan authorizes a new physical operation.

See [startup diagnostics](../firmware/startup-diagnostics.md), [UART and loader](../hardware/uart.md), [recovery limits](../hardware/recovery.md), and [optional fixtures](fixtures.md).
