# FW16 first supervised motor pulse

Status: prepared offline: probe, host tests, build variant and guards. Not installed, never run. It waits on FW30's transient check and FW14's current channel, and needs the owner present.

## What the image does

`wellfed-build build --profile firstboot --firmware motor --variant fw16-pulse` builds the `motor` firmware's `fw16-pulse` variant. It records `startup_motion_policy: fw16-pulse-v1` (declared in `src/firmware/wellfed/motor/Cargo.toml`) and `variant: fw16-pulse` in the manifest and gets its own build ID. The motor firmware's normal image adds no task, and the builder refuses a motion policy outside a variant, so nothing else can drive the motor. The bench never builds it.

After the LED probe, the pulse probe (`src/sdk/wellfed-probes/src/pulse.rs`):
1. Skips everything unless this is a clean cold boot: reset reason 0 and its backup-register marker clear.
2. Opens the board (motor STOP, emitters OFF, sensors), binds a fresh flash authority to the KM0 peer and opens the supervisor (TIM2 at 10 ms, owner and motor lease 200 ms). It enables feedback capture.
3. Logs `armed`, then waits 60 s, renewing its owner lease every 2 ms, so the operator can be ready.
4. Sets the marker, then drives once: `DRIVE_0` (PA13 high, PA14 low) for 100 ms, then STOP. It counts PA18 feedback edges and samples motor current on ADC channel 4 every 10 ms.
5. Watches for 1 s more, then verifies STOP. It checks the gate state, the motor pins, the supervisor closing and stopped-output readback.

Bounds that hold even if the task stalls: the motor gate's 200 ms command lease and the owner lease, both enforced by the timer interrupt. A host test stalls the task mid-pulse and requires STOP within the lease. A drive is only applied through `Events::motor`, which needs a running timer, a live owner and enabled capture.

**Every clean cold boot pulses once.** The marker survives a watchdog reset but not power loss. The image must not stay installed after the session.

Records (`PLMOT1`):
- `start created=`
- `open board= ready= attached= events= capture= ms=`
- `armed delay_ms= pulse_ms= lease_ms=`
- `pulse dir= applied_ms= on_ms= edges= first_edge_ms= current=min,max,mean samples=`
- `after ms= edges= current=… stopped=`
- `done`
- `skip`, `timeout` or `error stage= fault= stopped=` end the probe stopped.

## Guards

- **`wellfed-bench`:** `flash-prepare` and `flash-execute` refuse a target whose manifest names a motion policy unless `--attended-motion` is given.
- **Startup audit:** binds `fw16-pulse-v1` to the `PLMOT1 start created=` format both ways, and refuses motion and optics policies together.
- **Prerequisites:** the board makes FW30 (transients) and FW14 (current channel) prerequisites. Do not install this before both pass.

## Procedure (owner present)

1. FW30 transients pass (`wellfed-bench transients`), and FW14 confirms the current channel reads apart from the battery channel.
2. Hopper empty, the outlet clear, hands away from the mechanism. The owner can cut USB power at any moment.
3. Install the frozen review with `--attended-motion` and `--boot-seconds` short (10). Power off right after the install.
4. Load a dashboard script anchored on `PLMOT1 armed`, with a countdown to the pulse at 60 s.
5. Cold boot with a 120 s capture. Observe the motion and record its direction and size.
6. Reinstall a non-motion image before anything else.

Pass: one `pulse` record with `on_ms` about 100 and `stopped=1` after it, the motor visibly moving only once, and no `error`. The `edges` and current values are the first real measurements of feedback and drive current. Direction 0's physical rotation and its INA/INB mapping are recorded from observation.

## Not covered

Lease expiry with a stalled owner was host-tested and FW05-measured, but not under real motion; this session gives the first real-motion data. Cancellation, failed stop, the journal-reserved feed, portions and reverse recovery belong to the full FW16 owner path and FW19.
