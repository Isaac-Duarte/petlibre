# Checking a feeder with the QA firmware

The `qa` firmware (`src/firmware/wellfed/qa`) grades a feeder's hardware in one boot. It walks you through every bring-up check that needs no motor: the optical sensors, the power inputs, the four buttons, the four lights and the speaker. It ends with one UART line that says whether the unit passed. This guide is for contributors bringing up a unit on the bench.

The motor never moves. The optics and power checks hold it at STOP, and pressing Dispense only answers a prompt.

## Before you start

You need the bench set up for UART capture and over-the-air installs, and someone at the feeder to press buttons, look and listen. No screen is needed at the feeder: the startup chime plays at boot, and each button prompt plays a short cue. Every prompt is also a UART line, and the bench page's live log shows it.

## Steps

1. Run `make -C src/firmware/vendor firstboot`. `FIRMWARE=qa` is the default, so this builds the QA image.
2. Run `wellfed-bench ota NAME --build` to install it over the air. You should see the build confirm, as any build does.
3. Start a receive-only UART capture, then cold boot the feeder. You should hear the startup chime.
4. Wait about 105 s while the **optics** and **power** checks run on their own.
5. When prompted, press each **button** in turn: Reset, Mic, Dispense, Lock. Each prompt plays a cue.
6. For each **indicator**, look at the light and answer: press **Dispense for yes** (it lit) or **Lock for no**.
7. For the **speaker**, listen to the test tune (1 s at 1 kHz) and answer the same way.
8. Read the last line, `PLQA1 result status=pass|fail …`. You should see `status=pass` and every check named `pass`.
9. Grade the capture: `wellfed-audit qa --capture private/captures/NAME-01 --manifest … --image … --output private/…/qa.json`. You should see the outcome `pass`.

When the result is logged, the QA task ends. The runtime keeps the build confirmed and updatable as usual.

## What each check does

| Check | You do | Passes when |
| --- | --- | --- |
| **Optics** | Nothing, about 100 s | The [optics probe](optical-inputs.md#the-optics-probe) (plan 2) opens in full, every window reads stopped outputs, and its owner stall cuts the emitters and closes the supervisor |
| **Power** | Nothing, 5 s | Five [ADC probe](bring-up-probes.md#power-input-probe) samples convert, with the clock verified and outputs stopped |
| **Buttons** | Press each button when prompted | Each prompted press arrives on that button's pin within 30 s |
| **Indicators** | Answer for each light | Each light is driven HIGH, its settled readback accepts the level, and you answer yes |
| **Speaker** | Answer for the tune | The test tune starts and you answer yes |

Notes:

- **Optics** holds the KM0 flash handshake, so the build does not confirm before it ends.
- **Power** values are logged as `PLADC1` lines, not judged. The battery divider is measured on one unit only, and no limits are set. The check shows the ADC path works, not that the battery voltage is in range.
- **Buttons** are Reset (PB4), Mic (PB5), Dispense (PB6) and Lock (PB7), active LOW ([buttons](buttons.md)). A press of another button fails that item as `wrong`, and no press fails it as `timeout`. All four lines must read released when the check opens, or every guided check fails as `lines`.
- **Indicators** are PA0, PA2, PA4 and PB22 ([indicator lights](indicators.md)). Buttons other than Dispense and Lock are ignored while it waits for your answer.
- **Speaker** with `sound` off fails as `muted`, without a prompt.

## Records

Each probe logs its own records (`PLOPT1`, `PLADC1`, `PLBTN1`, `PLAUD1`), so their existing parsers keep working. QA adds `PLQA1` lines around them:

- `start stage=optics|power|buttons|indicators|speaker`
- `prompt press=reset|mic|dispense|lock`
- `prompt look=<pin> yes=dispense no=lock` and `prompt listen=<tune> yes=dispense no=lock`
- `item stage=… press=…|led=…|tune=… status=pass`, or `status=fail reason=timeout|wrong|no|refused|unverified|muted|busy|lines`
- `check stage=… status=pass|fail`
- `result status=pass|fail optics=… power=… buttons=… indicators=… speaker=…`, last

The image carries the optics and ADC probes, so it declares their startup policies (`fw13-optics-v2`, `fw14-adc-v2`), and the startup audit binds each to the image's markers.

## How the audit grades

`wellfed-audit qa` loads the capture and its source-bound build as the startup audit does. It requires `firmware: qa` in the manifest and a passing startup. It then reads the `PLQA1` lines and trusts none of the summaries on their own:

- every check must start in order and report one verdict;
- the result line must repeat those verdicts;
- its status must follow from them.

The outcome is one of:

- `pass`;
- `qa_fail`: the run completed and a check failed, with the failed items listed;
- `qa_sequence`: missing, repeated, out-of-order or inconsistent records, with the fault named;
- `startup_<outcome>`.

Buttons, indicators and the tune are graded by what you answered. A lit light and an audible tune are your observations, not measurements.

## Common mistakes

- **Pressing too early.** A press must start after its prompt. A button still held from the previous answer, then released and pressed again within the 30 ms debounce, can be missed. The item then times out rather than passing wrongly.
- **Answering too fast.** An answer within a fraction of a second of the prompt is often a mis-press. Wait for the light or the tune before you answer.
- **Missing a prompt.** Each button item waits 30 s. Watch the live log if you can't hear the cue.

## Known issues

- **Optics on a trial boot.** Reference-firmware boots once logged `PLOPT1 error kind=supervisor n=1 fault=7`, which fails the optics check. The first WPA3 join starved the QA task past its 300 ms owner lease (`stall=703` on an OTA trial boot, `stall=702` on a cold boot with the hotspot up; every window passed with the hotspot down). The radio now waits for `startup_done` (`PLWIF3 hold`), so the optics check runs before the join.

## Next steps

- One probe at a time, for as long as a session needs: the `bringup` firmware's variants (`optics`, `buttons`, `adc`, `speaker`, `pulse`, and the `stall` and `fail` trial images). QA steps the same probes from `src/sdk/wellfed-probes`, for a bounded time each.
- What the startup records mean: [startup and diagnostics](startup-diagnostics.md).

## Evidence

- **Every check passed in one run.** Observed (October 23, 2026, owner answering): `qa-20261023-02-guided`, on `qa` `645c1cd4…` (v92), graded `pass`. This was the first complete passing run.
- **Two earlier guided runs** of `qa` `ef76b85e…` (v84), each graded by `wellfed-audit qa`, passed every check and item between them:
  - `fw39-qa-03-guided`: optics, power, all four buttons, three indicators and the speaker passed. PA0 failed as `no`: the owner mis-pressed Lock about 0.9 s after the prompt.
  - `fw39-qa-04-guided`, coached by the bench page's guided-QA panel: optics, power, all four indicators, the speaker and three buttons passed. Reset timed out with no press on the line; the owner missed the prompt.
- **Optics fault.** Observed: the starvation figures above (`stall=703`, `stall=702`).
- **Host.** Modeled: the sequence runs against the SDK fake with a scripted operator.
