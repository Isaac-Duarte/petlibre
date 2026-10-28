# Import provenance

Wellfed began as a curated copy of first-party code from a private PLAF103 research archive. This page records what came across, what stayed behind, and how the archive is kept. The archive is separate, and nothing builds or runs from it.

## What came across

- The Rust server and firmware workspace, and their tests.
- SDK pins, hooks and build audits.
- Simulator scenarios.
- Stock marker and probe preparation.
- UART read and guarded recovery tools, and their local dependencies.

No old Git history, raw captures, private evidence, vendor binaries, device backups or decompiled outputs were imported. Unrelated stock-behavior emulators, Ghidra tooling, host-specific hotspot setup, experiment diaries and historical commit mappings stayed in the archive.

[import-manifest.json](import-manifest.json) records the source revision and SHA-256 of each selected file **before** the portability and documentation changes. Those hashes describe the import, not the current files or any deployable image. The private revision is an audit reference, not a public dependency or a reproducible link.

## What changed on the way in

- Network and capture inputs became explicit.
- New configurations default to UTC.
- Dated fixture paths became generic ones.
- Contributor tests were separated from optional binary evidence.
- Current instructions were rewritten.

Crate names, wire formats, the C ABI and the guarded recovery restrictions stayed as they were. The import included no publication, deployment or hardware operation.

The docs consolidate label and board inspection, UART and backup observations, instruction-level analysis of stock 1.1.15, and host and model testing. Raw evidence is not public, so a summarized observation is not a publicly reproducible hardware result. The guides cite external references and keep the relevant firmware addresses and versions, without copying vendor code or private device identifiers.

## A later code update

[import-updates.json](import-updates.json) records a follow-up source snapshot and its incoming hashes. It brought the region-scoped development updater, the startup evidence verifier, stopped outputs, the LED diagnostic, the shared-pad supply fix, helper-readiness synchronization, and their tests.

Private material was copied separately into ignored storage. The copied SDK worktree's administration points to the local copy, so local builds never modify the archive. Raw evidence content is kept as it was. Neither copied receipts nor old source identities authorize a new physical operation.

## The preserved research archive

The original research repository can be retired without this checkout depending on it. The ignored `private/research-archive/` holds:

| Path | Contents |
| --- | --- |
| `files/` | The complete tracked source and documentation snapshot, including omitted research utilities and dated records |
| `research-history.bundle` | A verified, self-contained Git bundle of the original branches and history. You can clone it locally without the original directory |
| `snapshot.json` | The archived revision and the bundle's hash |

An imported firstboot SDK build archive keeps the original artifacts from before a Wellfed rebuild.

Copied SDK worktree administration and toolchain links point to Wellfed's local copies, and generic fixture aliases point to local evidence bundles. Historical logs, receipts, frozen images and archived source may still name the old path as recorded metadata. They are kept as they are, not rewritten into new evidence. Active workflows use only this repository plus explicitly acquired dependencies. No external running service or physical bench session moved with the import.
