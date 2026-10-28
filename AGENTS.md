# Contributor and agent instructions

## Start here

- The working files live in the ignored `.agents/` folder, not in Git: `.agents/RESUME.md` (the bench handoff), `.agents/FIRMWARE_BOARD.md`, `.agents/FEATURE_BOARD.md`, `.agents/SERVER_BOARD.md` (server crates and capabilities) and `.agents/SERVER_SPEC.md` (the owner's server design decisions) and `.agents/api-design.md` (the former API design, with its decisions and open questions). Tracked docs never link to them; the public summary is [the roadmap](docs/roadmap.md), and card ids stay out of tracked docs.
- Read `.agents/RESUME.md` when continuing firmware work, then the boards, [README.md](README.md) and the [development guide](docs/development/README.md). Write docs by the [writing guide](docs/development/writing.md). Use docs/chips/ (shared SoC facts), docs/devices/ (one folder per feeder model, its board and stock firmware) and docs/firmware/ (the replacement firmware) for hardware assumptions and component contracts, and [product architecture](docs/firmware/product-architecture.md) for the product the SDK, server and app are built toward, with [server docs](docs/server/README.md) for its contracts and crates.
- Keep RESUME's checkpoint and its ignored local evidence index current when bench state changes. Reconfirm physical state rather than assuming the handoff is live.
- Keep the three boards (SDK and hardware capability; owner-visible behavior and stock parity; the server) current when implementation, dependencies or acceptance evidence change, and update [the roadmap](docs/roadmap.md) when a capability's public status changes. Card status never authorizes physical operations.
- Label every finding as physical observation, static analysis or simulation. Host checks do not establish hardware reliability.

## Hard rules

- Work within this repository. A neighboring research archive is not a build dependency or a mutation target.
- Keep user changes and private evidence intact. Never copy credentials, real device identifiers, captures, backups, vendor binaries or decompiled artifacts into tracked files.
- Never weaken pinned recovery hashes, marker sequencing, preservation checks or no-retry behavior to make tests pass. Rebuilt diagnostics are not automatically compatible with the pinned writer.
- Build and test commands must not flash hardware, operate motors, alter routing or start persistent services. Those need an explicit user task and a reviewed device-specific procedure (or the authorization below).
- Do not infer authorization to commit, push, publish or resume bench operations from historical notes. Respect the contributor's Git identity.

## Rust conventions

Everything first-party is under `src/`. The Rust workspace is `src/sdk/` (the SDK every firmware builds on), `src/product/` (product policy shared by the firmware and the server, starting with `wellfed-feeder`), `src/client/` (the owned device API values of `wellfed-model`, the sans-IO `wellfed-client` and the tokio `wellfed-connect` every host client shares), `src/server/` (the owner's server, layered as the [server design](docs/server/design.md#crates) lays out), `src/tools/wellfed-*` and `src/tools/xtask` (host tools) and `src/firmware/wellfed/` (one crate per firmware); `src/firmware/vendor/` holds the AmebaD C ports, pins and image build, `src/web/` the web frontends and `src/app/` the Flutter Android app. The [Rust standard](docs/development/rust.md) is the full rule set, with what enforces each rule; in short, the compiler enforces most of them. Every crate inherits `[workspace.lints]`, and `make check-rust` runs a second Clippy pass over libraries and binaries with `PRODUCTION_LINTS` (no `unwrap`, no narrowing `as`, no needless by-value parameters, no unreachable `pub`). `make check-deps` runs cargo-deny over licenses, advisories and sources (`deny.toml`), and tests run under cargo-nextest. A deliberate exception is an `#[expect(lint, reason = "…")]` at the narrowest scope. Keep existing crate names, public protocols and the C interface the AmebaD ports call (the `wellfed_*` exports `src/firmware/vendor` uses, and the KM0 park and hook symbols) unless the task requires a compatibility change; there is no general-purpose C ABI over the SDK. Every host tool is Rust; the Python tools were ported or retired by October 23, 2026, and `cargo xtask check-repository` refuses Python under `src/`.

- **Typed data.** Model JSON with `#[derive(Serialize, Deserialize)]` structs and enums. Use `serde_json::Value` only for JSON that is genuinely opaque: hashed or replayed verbatim, or passed through unread.
- **Shared formats.** A file one crate writes and another reads gets its type in `wellfed-formats`, not a copy in each reader. Anything hashed goes through `wellfed_formats::canonical`, which fixes the bytes regardless of field order.
- **Enums, not strings or flags.** Closed sets are enums (with `clap::ValueEnum` for CLI options). Prefer a two-variant enum over a `bool` parameter whose meaning isn't obvious at the call site. Firmware states, faults and reasons are Rust enums with fixed discriminants: a record or report carries `code()`, and a byte read from flash becomes a variant only through `from_byte` at the decode boundary.
- **Errors.** Library crates expose a `thiserror` enum; binaries and CLI glue use `anyhow` with context. No `Result<_, String>` and no `Refused(String)`-style catch-all: each refusal is a variant carrying the values its message names, so a caller or test matches it rather than its text. Checks go through `wellfed_formats::require(condition, || Error::…)`; an error from a crate below is wrapped whole (`#[from]`), not copied into a message.
- **No panics on input.** No `unwrap`/`expect` outside tests unless it guards an invariant the code just established, with the reason in the message. Use `let … else`, `try_from` and `?`.
- **Narrow visibility.** Default to `pub(crate)`. Use `pub` only for what another crate or an integration test uses.
- **One helper, one place.** Search before writing hex, hashing, HMAC, report-writing or serial helpers; the workspace already has them.
- **Firmware.** Firmware crates are `no_std` staticlibs. `unsafe` belongs only at the C interface and MMIO edge, and each block gets a `// SAFETY:` comment. Changing `repr(C)` types, `#[no_mangle]` symbols, the park/hook RAM code or flash record layouts changes audited binaries: rerun the link, RAM-park and stack audits and keep their pins.
- **Readable code.** Separate the logical steps in a function body (setup, the work, the result) with a blank line, and break long match arms, tests and builders into paragraphs. Comments and `//!` docs explain why the code exists and how it fits the system (invariants, hardware or protocol constraints, ordering requirements), not what the next line does. They state the constraint itself, not the board card it came from: card history lives on the boards and in RESUME, and `cargo xtask check-repository` refuses card ids under `src/`.

## App conventions

The Android app in `src/app/` is the only Dart in the repository. Its protocol logic lives in Rust, so the app, the server and the CLI cannot disagree about what goes over the wire.

- **Dart is the UI and platform IO.** Screens, state, Android permissions, BLE, NSD discovery, sockets and joining a Wi-Fi network belong in Dart, through maintained plugins.
- **Rust is everything the feeder checks.** Noise handshakes, pairing, invites, request building, event-cursor sync, validation and the installer's steps live in the sans-IO `wellfed-client` crate, which takes bytes in and returns bytes out. Dart moves those bytes and never reimplements them: no Dart crypto, signing or wire codec.
- **Generated types.** Dart types for requests, events and settings are generated from Rust through `flutter_rust_bridge`, as the web's TypeScript types are. Never hand-write a Dart copy of a Rust type, and commit regenerated bindings with the Rust change that caused them.
- **Feeding safety.** Every feed and state change goes through the device API like any other client. The app holds no schedule or feeding logic of its own, and a retry never resends a request the core marked uncertain.
- **Lints.** `analysis_options.yaml` enables strict analysis; fix warnings rather than suppressing them, and give a deliberate `// ignore:` its reason on the same line.

## Web conventions

The web frontends in `src/web/` follow the [TypeScript standard](docs/development/typescript.md) (the full rule set and its enforcement) and the [web UI design](docs/web/README.md). In short:

- **Generated contracts.** The server UI's types and client come from `openapi.json`, the bench's from ts-rs, and colors and spacing from `assets/brand/tokens.json`. Never hand-write a TypeScript copy of a Rust type or a hex value.
- **Boundaries.** Only `packages/ui` imports bits-ui, and only the API layer touches `fetch`, `EventSource` or `BroadcastChannel`. ESLint enforces both.
- **Dependencies.** Exact versions, a frozen lockfile, a 7-day minimum release age, no install scripts unless listed, and an allowed license (`pnpm run licenses`).
- **Feeding safety.** Mutations never retry. A feed keeps one idempotency key from confirm to a terminal result, and an uncertain feed is never retried.
- **Strict and quiet.** Strict TypeScript with no `any`, non-null assertions or casts outside generated code; zero warnings from svelte-check, ESLint and Prettier; a deliberate lint disable carries its reason on the same line.

## Compatibility

Wellfed is in active development, and the research unit is the only device running it. **Never add backward compatibility.** Change a format in place and drop the old reader: on-flash records, the device API's messages, server JSON and database rows, file shapes and old evidence. No migrations, no `#[serde(default)]` shims, no capability bits or new variants added only to keep an old peer working. When a stored format changes, the change records the bench step that clears it (erasing or resetting the store) instead.

This covers what this repository builds. Interop with things it does not build, and the safety pins, stay exact:
- the stock firmware's wire protocol and on-flash formats;
- the frozen reviews, plans and journals `wellfed-flash` accepts, including those the former Python tools wrote;
- canonical JSON that is hashed;
- every pinned check in the hard rules.

## Checks

| Change | Run |
| --- | --- |
| Any Rust edit | `cargo fmt --all`, then `make check` (the contributor gate) |
| Builder, audits, formats or flash review | Also `make check-fixtures` against fresh builds (`make -C src/firmware/vendor device firstboot`) |
| Firmware crates | Also a fresh image build, so its link, RAM-park and stack audits run |
| `src/web/`, or the views `wellfed-bench --ui` serves | Also `make check-web` (needs Node and pnpm): ESLint, Prettier, licenses, web types, unit tests, the build, and Playwright against the real binary |
| `Cargo.lock` or `deny.toml` | `make check` includes `check-deps` (cargo-deny); its first run downloads the RustSec database |
| `src/app/`, or a `wellfed-client` type the app uses | Also `make check-app` (needs the Flutter SDK): bindings regenerated with no diff, `dart format`, `flutter analyze`, `flutter test`, and a debug APK build |

A skipped or missing fixture is not a passing check. `make check-app` arrives with the first app code (FP19); until then no change touches `src/app/`.

## Owner authorization (October 22, 2026)

For an agent working an owner-assigned firmware task:
- It may flash any flash region through the reviewed bench tools, as many times as needed.
- For live tests it may bring the feeder hotspot up and run `wellfed-stock serve` (until October 26, `wellfed-local serve`), including OTA offers, as a bounded foreground test process.
- Afterwards it must restore the prior network and service state and record what ran.

This does not cover motor motion, host firewall or system-service changes, publishing outside this repository's remote, or weakening any pinned check above.
