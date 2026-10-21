# Optional binary and SDK fixtures

Ordinary contributor checks need no private binary inputs. The optional suite is selected explicitly:

```sh
make check-fixtures PYTHON=.venv/bin/python
```

It fails for missing fixtures, incompatible hashes, errors, or skips. It does not download binaries or produce missing evidence. A passing synthetic suite does not imply these checks passed. Python evidence cases are marked with `support.evidence`; direct unittest discovery can still show their unavailable-input skips, so use the Make targets when reporting contributor results.

## Generic local locations

| Input/location | Purpose and availability |
| --- | --- |
| `private/firmware/PLAF103_OTA_010115.bin` | Contributor-obtained stock 1.1.15 OTA; exact hash is recorded in the layout file |
| `private/upstream/ameba-rtos-d/` | SDK checkout obtained by explicit bootstrap; supplies candidate ROM/helper inputs |
| `private/toolchains/` and `private/firmware-build/` | Explicit SDK build artifacts and audited tools; a build whose manifest inputs differ from the current sources is stale and its suites skip (so `check-fixtures` fails) until rebuilt |
| `private/fixtures/baseline.bin` | Exact historical 8 MiB research baseline required by pinned recovery checks |
| `private/fixtures/current-read/flash-read.bin` | Recorded current full snapshot required by firstboot/reference tests |
| `private/fixtures/diagnostic-ota.bin` | Exact reviewed UART-only diagnostic, not an arbitrary new build |
| `private/fixtures/bounded-read/report.json` | Finalized bounded-read report; linked serial/read evidence must remain beside it |
| `private/fixtures/rom-inspection/report.json` | Finalized ROM report; linked serial evidence must remain beside it |
| `private/firmware/plaf103-sdk-rom-inputs/` | Optional extracted candidate ROM text used by the memory audit |
| `private/firmware/experiments/` | Explicitly prepared stock marker/probe original and candidate pairs, manifests, and compiled probe |

The generic locations replace dated workstation paths. They do **not** make the exact research-unit recovery profile portable. No other contributor is expected to possess its baseline or historical artifact. Keep each capture/report bundle intact and supply explicit report arguments when required; two independently captured bundles should remain in separate subdirectories with their companion files. Generic fixture paths are test conventions, not proof of valid session provenance.

Obtain stock updates from your own authorized device/app workflow, read original backups from your own hardware, and get the SDK/toolchain through the pinned upstream downloads. Those inputs may support independent analysis, but a new device's backup will intentionally fail a writer pinned to the original research profile. The historical baseline, captures, and exact accepted diagnostic are not publicly distributed, and the complete reference suite is not reproducible from public files alone.

For useful fixture-independent validation, run `make check`. To extend recovery support, design and review explicit per-device compatibility and preservation criteria as separate work; do not remove provenance checks or substitute hashes in order to get a green optional suite.

## Development-update fixtures

The new helper-model and CLI tests additionally use `private/fixtures/development-source/` and `development-target/` for intact frozen review bundles, and `private/fixtures/development-bounded-read/report.json` for the associated bounded-read evidence. Generic directory aliases must preserve companion serial/read files; individual report-file aliases are insufficient.

If importing your own private archive, local directory/file symlinks can connect these conventions to unchanged original evidence bundles. They remain ignored. The public checkout has no such aliases, and default tests stay independent of fixture presence. Existing SDK artifacts may not validate against changed Wellfed inputs; rebuild and freeze a new target instead of rewriting historical evidence.
