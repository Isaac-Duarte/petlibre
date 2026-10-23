# QA firmware

Status: host only (October 23, 2026). The sequence runs against the SDK fake with a scripted operator, and its records grade through `petlibre-audit qa`. It has not run on the feeder.

The `qa` firmware (`src/firmware/petlibre/qa`) is the default build. One boot walks an operator through every bring-up check that needs no motor, and ends with one line that says whether the unit passed:

```sh
make -C src/firmware/vendor firstboot                    # FIRMWARE=qa is the default
petlibre-bench ota NAME --build                          # or install it over the air
petlibre-audit qa --capture private/captures/NAME-01 --manifest … --image … --output private/…/qa.json
```

The single-purpose firmwares (`optics`, `buttons`, `adc`, `speaker`, `motor`, `trial`) each run one probe for as long as a bring-up session needs. QA steps the same probes (`src/sdk/petlibre-probes`), for a bounded time each.

## Sequence

| Check | Operator | Pass when |
| --- | --- | --- |
| **Optics** | none, about 100 s | The [optics probe](optical-inputs.md) (plan 2) opens in full, every window reads stopped outputs, and its owner stall cuts the emitters and closes the supervisor. It holds the KM0 flash handshake, so the build does not confirm before it ends. |
| **Power** | none, 5 s | Five [ADC probe](startup-diagnostics.md) samples convert with the clock verified and outputs stopped. Values are logged as `PLADC1` lines, not judged: the battery divider is measured on one unit only, and no limits are set. |
| **Buttons** | Press each button when prompted | Each prompted press arrives on that button's pin within 30 s: Reset (PB4), Mic (PB5), Dispense (PB6), Lock (PB7), active LOW (FW20). A press of another button fails that item as `wrong`; no press fails it as `timeout`. All four lines must read released when the check opens, or every guided check fails as `lines`. |
| **Indicators** | Answer for each indicator | Each indicator (PA0, PA2, PA4, PB22) is driven HIGH and its settled readback accepts the level, then the operator answers whether it lit: **Dispense = yes, Lock = no**. Other buttons are ignored. |
| **Speaker** | Answer for the tune | The test tune (1 s at 1 kHz) starts, and the operator answers yes. With `sound` off it fails as `muted`, without a prompt. |

The startup chime plays at boot, and each button prompt plays a short cue, so the operator needs no screen to follow along. Every prompt is also a UART line, and the bench page's live log shows it.

The sequence never drives the motor: the optics and power probes hold it at STOP, and pressing Dispense only answers a prompt. When the result is logged the QA task ends; the runtime keeps the build confirmed and updatable as usual.

## Records

Each probe logs its own records (`PLOPT1`, `PLADC1`, `PLBTN1`, `PLAUD1`), so their existing parsers keep working. QA adds `PLQA1` lines around them:

- `start stage=optics|power|buttons|indicators|speaker`
- `prompt press=reset|mic|dispense|lock`
- `prompt look=<pin> yes=dispense no=lock` and `prompt listen=<tune> yes=dispense no=lock`
- `item stage=… press=…|led=…|tune=… status=pass`, or `status=fail reason=timeout|wrong|no|refused|unverified|muted|busy|lines`
- `check stage=… status=pass|fail`
- `result status=pass|fail optics=… power=… buttons=… indicators=… speaker=…`, last

The image carries the optics and ADC probes, so it declares their startup policies (`fw13-optics-v2`, `fw14-adc-v2`), and the startup audit binds each to the image's markers.

## Audit

`petlibre-audit qa` loads the capture and its source-bound build as the startup audit does, requires `firmware: qa` in the manifest, and requires a passing startup. It then reads the `PLQA1` lines and trusts none of the summaries on their own:
- every check must start in order and report one verdict;
- the result line must repeat those verdicts;
- its status must follow from them.

The outcome is `pass`, `qa_fail` (the run completed and a check failed, with the failed items listed), `qa_sequence` (missing, repeated, out-of-order or inconsistent records, with the fault named), or `startup_<outcome>`.

Limits: buttons, indicators and the tune are graded by what the operator answered. A lit indicator and an audible tune are the operator's observations, not measurements. The power check shows that the ADC path works, not that the battery voltage is in range.

## Known before the first run

- Every reference-firmware boot since `itest-20261022-10` logged `PLOPT1 error kind=supervisor n=1 fault=7`. FW40 found the cause (physical): the first WPA3 join starves the QA task past its 300 ms owner lease (`stall=703` on an OTA trial boot, `stall=702` on a cold boot with the hotspot up; every window passes with the hotspot down). The radio now waits for `startup_done` (`PLWIF3 hold`), so the optics check runs before the join.
- A press must start after its prompt. A button still held from the previous answer, then released and pressed again within the 30 ms settle, can be missed. It then times out rather than passing wrongly.
