# Import provenance

Petlibre begins with a curated copy of first-party code from a private PLAF103 research archive. The archive remains separate and is not a runtime/build dependency. No old Git history, raw captures, private evidence, vendor binaries, device backups, or decompiled outputs were imported.

[import-manifest.json](import-manifest.json) records the source revision and SHA-256 of each selected source file **before** portability/documentation changes. These hashes describe the import, not the current files or a deployable firmware image. The private revision is an audit reference, not a public dependency or reproducible public link.

The selected code comprises the Rust server and firmware workspace, their tests, SDK pins/hooks/build audits, simulator scenarios, stock marker/probe preparation, UART read/guarded recovery tools, and their local dependencies. Unrelated stock-behavior emulators, Ghidra tooling, host-specific hotspot setup, experiment diaries, and historical commit mappings stay in the research archive.

Documentation consolidates label/board inspection, UART and backup observations, instruction-level analysis of stock 1.1.15, and host/model testing. Raw evidence is not public, so a summarized observation is explicitly not a publicly reproducible hardware acceptance result. Guides cite external reference documents and retain relevant firmware addresses/versions without copying vendor code or private device identifiers.

Portability changes make network/capture inputs explicit, default new configurations to UTC, replace dated fixture paths, separate contributor tests from optional binary evidence, and rewrite current instructions. Existing Rust crate names, wire formats, C ABI, and guarded recovery restrictions are retained. The initial repository is prepared for review; no publication, deployment, or hardware operation is part of this migration.

## Later code update

[import-updates.json](import-updates.json) records the follow-up source snapshot and incoming hashes. This update carries the region-scoped development updater, startup evidence verifier, stopped outputs, LED diagnostic, shared-pad supply fix, helper-readiness synchronization, and associated tests. The feature board and guides consolidate new findings without importing the bench diary or personal agent permissions.

Private material was copied separately into ignored storage at the user's request. Copied SDK worktree administration points to the local copied SDK so later local builds do not modify the research archive; raw evidence content is retained. Generic fixture aliases preserve complete report bundles. Previous Petlibre code/docs are retained in an ignored migration backup. Neither copied receipts nor old source identities authorize a new physical operation.

## Self-contained research preservation

The original research repository can be retired without making it a dependency of this checkout. Ignored `private/research-archive/files/` retains its complete tracked source/documentation snapshot, including omitted research utilities and dated records. `research-history.bundle` is a verified self-contained Git bundle of the original branches and history; it can be cloned locally without the original directory. `snapshot.json` identifies the archived revision and bundle hash. An imported firstboot SDK-build archive retains the original artifacts before a Petlibre rebuild.

Copied SDK worktree administration and toolchain links reference Petlibre's local copies. Generic fixture aliases point to local evidence bundles. Historical logs, receipts, frozen images, and archived source can still mention the old path as recorded metadata; they are preserved rather than rewritten into new evidence. Active public workflows use only this repository plus explicitly acquired dependencies. No external running service or physical bench session is moved by this migration.
