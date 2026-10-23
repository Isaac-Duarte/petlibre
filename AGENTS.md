# Contributor and agent instructions

## Start here

- Read [RESUME.md](RESUME.md) when continuing firmware work, then [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md), [FEATURE_BOARD.md](FEATURE_BOARD.md), [README.md](README.md) and the [development guide](docs/development/README.md). Use docs/hardware/ and docs/firmware/ for hardware assumptions and component contracts.
- Keep RESUME's checkpoint and its ignored local evidence index current when bench state changes. Reconfirm physical state rather than assuming the handoff is live.
- Keep FIRMWARE_BOARD.md (SDK and hardware capability) and FEATURE_BOARD.md (owner-visible behavior and stock parity) current when implementation, dependencies or acceptance evidence change. Card status never authorizes physical operations.
- Label every finding as physical observation, static analysis or simulation. Host checks do not establish hardware reliability.

## Hard rules

- Work within this repository. A neighboring research archive is not a build dependency or a mutation target.
- Keep user changes and private evidence intact. Never copy credentials, real device identifiers, captures, backups, vendor binaries or decompiled artifacts into tracked files.
- Never weaken pinned recovery hashes, marker sequencing, preservation checks or no-retry behavior to make tests pass. Rebuilt diagnostics are not automatically compatible with the pinned writer.
- Build and test commands must not flash hardware, operate motors, alter routing or start persistent services. Those need an explicit user task and a reviewed device-specific procedure (or the authorization below).
- Do not infer authorization to commit, push, publish or resume bench operations from historical notes. Respect the contributor's Git identity.

## Rust conventions

Everything first-party is under `src/`. The Rust workspace is `src/sdk/` (the SDK every firmware builds on), `src/tools/wellfed-*` and `src/tools/xtask` (host tools) and `src/firmware/wellfed/` (one crate per firmware); `src/firmware/vendor/` holds the AmebaD C ports, pins and image build, and `src/web/` the web frontends. The compiler enforces most of these rules. Every crate inherits `[workspace.lints]`, and `make check-rust` runs a second Clippy pass over libraries and binaries with `PRODUCTION_LINTS` (no `unwrap`, no narrowing `as`, no needless by-value parameters, no unreachable `pub`). A deliberate exception is an `#[expect(lint, reason = "…")]` at the narrowest scope. Keep existing crate names, public protocols and the C ABI unless the task requires a compatibility change. Every host tool is Rust; the Python tools were ported or retired by October 23, 2026, and `cargo xtask check-repository` refuses Python under `src/`.

- **Typed data.** Model JSON with `#[derive(Serialize, Deserialize)]` structs and enums. Use `serde_json::Value` only for JSON that is genuinely opaque: hashed or replayed verbatim, or passed through unread.
- **Shared formats.** A file one crate writes and another reads gets its type in `wellfed-formats`, not a copy in each reader. Anything hashed goes through `wellfed_formats::canonical`, which fixes the bytes regardless of field order.
- **Enums, not strings or flags.** Closed sets are enums (with `clap::ValueEnum` for CLI options). Prefer a two-variant enum over a `bool` parameter whose meaning isn't obvious at the call site.
- **Errors.** Library crates expose a `thiserror` enum; binaries and CLI glue use `anyhow` with context. No `Result<_, String>` and no catch-all string variants where a typed variant fits. Checks go through the crate's `require` helper.
- **No panics on input.** No `unwrap`/`expect` outside tests unless it guards an invariant the code just established, with the reason in the message. Use `let … else`, `try_from` and `?`.
- **Narrow visibility.** Default to `pub(crate)`. Use `pub` only for what another crate or an integration test uses.
- **One helper, one place.** Search before writing hex, hashing, HMAC, report-writing or serial helpers; the workspace already has them.
- **Firmware.** Firmware crates are `no_std` staticlibs. `unsafe` belongs only at the C ABI and MMIO edge, and each block gets a `// SAFETY:` comment. Changing `repr(C)` types, `#[no_mangle]` symbols, the park/hook RAM code or flash record layouts changes audited binaries: rerun the link, RAM-park and stack audits and keep their pins.
- **Readable code.** Separate the logical steps in a function body (setup, the work, the result) with a blank line, and break long match arms, tests and builders into paragraphs. Comments and `//!` docs explain why the code exists and how it fits the system (invariants, hardware or protocol constraints, ordering requirements), not what the next line does.

## Compatibility

Old on-disk evidence and stale file fields may break when that makes the code simpler; drop the legacy reader rather than carrying it. Some things are live interop rather than legacy, and must stay exact:
- formats a current consumer reads (the frozen reviews, plans and journals `wellfed-flash` accepts, including those the former Python tools wrote);
- the stock and replacement firmware wire protocols;
- canonical JSON that is hashed;
- every pinned check in the hard rules.

## Checks

| Change | Run |
| --- | --- |
| Any Rust edit | `cargo fmt --all`, then `make check` (the contributor gate) |
| Builder, audits, formats or flash review | Also `make check-fixtures` against fresh builds (`make -C src/firmware/vendor device firstboot`) |
| Firmware crates | Also a fresh image build, so its link, RAM-park and stack audits run |
| `src/web/`, or the views `wellfed-bench --ui` serves | Also `make check-web` (needs Node and pnpm): web types, unit tests, the build, and Playwright against the real binary |

A skipped or missing fixture is not a passing check.

## Owner authorization (October 22, 2026)

For an agent working an owner-assigned firmware task:
- It may flash any flash region through the reviewed bench tools, as many times as needed.
- For live tests it may bring the feeder hotspot up and run `wellfed-local serve`, including OTA offers, as a bounded foreground test process.
- Afterwards it must restore the prior network and service state and record what ran.

This does not cover motor motion, host firewall or system-service changes, publishing outside this repository's remote, or weakening any pinned check above.
