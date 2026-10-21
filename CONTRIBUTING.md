# Contributing

Start with [development setup](docs/development/README.md), then choose the local server, portable firmware core, AmebaD port, or documentation. No hardware is needed for ordinary checks.

Keep changes focused and include regression coverage for behavior changes. Run `cargo fmt --all` after Rust edits and `make check PYTHON=.venv/bin/python` before submitting. Report which optional evidence or hardware checks were actually performed; do not equate unavailable fixtures with passing validation.

For hardware findings, record model/revision, stock firmware version, observation method, uncertainty, and a redacted result. Distinguish software pin assignments from measured electrical wiring. Describe reproduction without requiring another contributor's private archive.

Use synthetic identifiers and inputs in examples and tests. Store real credentials, device dumps, logs, downloaded SDKs, and third-party binaries in ignored `private/`. Inspect proposed changes before sharing; ignore rules do not protect files deliberately added to Git.

Preserve command identifiers, durable request/journal behavior, authentication, and recovery integrity gates. Changes to feeding, flash, or boot behavior need explicit acceptance criteria and a recovery strategy appropriate to the tested board.

First-party contributions are submitted under the repository's [MIT license](LICENSE). Preserve third-party notices and document provenance for new dependencies or adapted material. Do not redistribute vendor firmware, ROM dumps, or raw device backups here.
