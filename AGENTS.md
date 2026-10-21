# Contributor and agent instructions

- Keep FIRMWARE_BOARD.md current when implementation, dependencies, or acceptance evidence changes. Card status never authorizes physical operations.
- Read README.md and docs/development/README.md before changing behavior. Use docs/hardware/ and docs/firmware/ for hardware assumptions and component contracts.
- Work within this repository. A neighboring research archive is not a build dependency or an authorized mutation target.
- Keep user changes and private evidence intact. Do not copy credentials, real device identifiers, captures, backups, vendor binaries, or decompiled artifacts into tracked files.
- Keep first-party source in the Rust workspace; preserve existing crate names, public protocols, and C ABI unless the task requires a compatibility change.
- Record whether a finding comes from physical observation, static analysis, or simulation. Host checks do not establish hardware reliability.
- Run cargo fmt after Rust edits. Run the applicable checks; make check PYTHON=.venv/bin/python is the contributor gate. Missing optional fixtures are not successful evidence checks.
- Never weaken pinned recovery hashes, marker sequencing, preservation checks, or no-retry behavior to make tests pass. Rebuilt diagnostics are not automatically compatible with the pinned writer.
- Build/test commands must not flash hardware, operate motors, alter routing, or start persistent services. Such operations require an explicit user task and a reviewed device-specific procedure.
- Do not infer authorization to commit, push, publish, or resume bench operations from historical research notes. Respect the contributor's Git identity.
