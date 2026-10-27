# Server spec

The owner's design decisions for building out the server API on [SERVER_BOARD.md](SERVER_BOARD.md), settled October 26, 2026 for an unattended overnight run. This page supplements the board, the [API design](docs/api/README.md) and the [product architecture](docs/firmware/product-architecture.md): it records only what they leave open, and says so where it changes them. SV01 folds the lasting parts into the API design. Everything here is a design decision; none of it is evidence.

Scope: the HTTP API, the CLI over it and their crates. No web UI.

## Run

| Topic | Decision |
| --- | --- |
| In scope | Every SV card except SV09 and SV65 (another agent holds the bench tonight for long tests), SV61 (no web UI), SV23 (FP03 is not built) and SV19 (avoid conflicts in `src/client/`). SV13 and SV52 run host-only against the simulator, since FW54, FW55, FW62, FW42 and FW43 are host-done |
| SV09 before SV07 | Dropped. SV07 proceeds without the bench run; SV09 runs later against the restructured server, toward SV65 |
| Order | SV01 → SV06, SV07, SV08; then SV20 → SV21 → SV22 → SV30, SV31, SV32 → SV24, SV25 → SV33 → SV34 → SV60, SV63, SV12 → SV10, SV11, SV64 → SV40, SV41 → SV70 → SV42 → SV13, SV14 → SV50, SV51, SV52 → SV62. If the night runs short, the feed path lands first |
| Run rules | The board's: an implementer and a reviewer per card, three review rounds at most, rebase on the newest `master`, `cargo fmt --all` and `make check`, commit to `master`, push. The agent may change a rule or the structure when it judges that better, and records each change under [run log](#run-log) for the owner's morning review |
| Blocked cards | A small design gap gets a reasonable call, recorded in the run log. A card parks on its branch only on a feeding-safety or contract-level gap: the question goes under [open questions](#open-questions) and the run moves to the next card whose prerequisites are Done |
| Hardware | None. No bench, no motion, no flashing |

## Crate layout

The board's [target architecture](SERVER_BOARD.md#target-architecture) stands, with these settled:

| Topic | Decision |
| --- | --- |
| `wellfed-service` | One crate with a module per domain (accounts, household, commands and the journal, owner data, derived views, alerts, audit, export). Each domain has its own error enum, wrapped in the crate's `Error`. Derived views are a pure module inside it |
| Auth | The service owns the rules: argon2, issuing and verifying sessions and tokens, expiry, the login-limit state, and a `Permission` enum with one `can(role, permission)` table that holds the [matrix](#permissions). `wellfed-server` owns only the axum extractors that call it |
| Jobs | The supervisor lives in `wellfed-server` and schedules on the cancellation tree with the injected `Clock`. Each job's body is a service method that takes `now` (`run_retention`, `run_backup`, …) |
| Hub | **Changes the board:** the hub lives in the service, not the mirror. The mirror exposes a per-feeder broadcast of typed mirror events (events, gaps, samples, link state). The service merges command results, alerts and install progress into it and assigns the stream ids. The mirror knows nothing of alerts or users |
| Ingest | The mirror writes to the store directly: it decodes each event once and calls the store to record the raw row, its projections and the cursor in one transaction. The service reads projections |
| Store writes | Keep today's single writer connection beside the read pool. The API design's "writer task" wording is updated to match |
| `wellfed-server-client` | In `src/client/`, async over `reqwest` with rustls, including following the SSE stream. The CLI and the server's tests use it |
| Export format | The archive's manifest and row types live in `wellfed-formats`, since export writes the archive and import reads it. The manifest's hashes go through `wellfed_formats::canonical` |
| Release verifier | Reuse the verifier the feeder's own checks are tested against, never a copy. If it drags in firmware-only dependencies, split out a small host-safe crate and record it in the run log |
| Dependency rules | `cargo xtask check-repository` enforces the allowed edges from `cargo metadata`, with an error naming the edge: no `sqlx` above the store, no `axum` below the server, and the server crates' arrows as drawn |
| `wellfed-api` on Android | Enforced by an allow-list of its dependencies in the same check. No Android cross-compile in `make check` |

## API conventions

- `/api/v1` may change freely until the first server release; the `openapi.json` diff shows every change. Migrations squash until then too.
- Every list takes `?after=<opaque cursor>&limit=` (default 100, max 1000) and answers `{ items, next }`, with `next` null at the end: events, commands, audit, alerts and raw samples alike.
- New refusals are `ErrorCode` variants (`key_reused`, `install_running`, `account_disabled`, …), never a string.

## Accounts and auth

### Permissions

| Action | Owner | Member | Viewer |
| --- | --- | --- | --- |
| Status, history, samples, plans, food views | ✓ | ✓ | ✓ |
| Feed and cancel | ✓ | ✓ | |
| Settings, lock, play, a time sync that does not step the clock | ✓ | ✓ | |
| An invite for their own phone (one key per token) | ✓ | ✓ | |
| Their own commands and audit entries | ✓ | ✓ | ✓ |
| Everyone's commands and audit | ✓ | | |
| Users, others' tokens, household, time settings | ✓ | | |
| Pets, foods, refills | ✓ | | |
| Wi-Fi, reboot, listing and revoking clients | ✓ | | |
| Adopting and removing feeders | ✓ | | |
| Releases, installs, backups, export, import | ✓ | | |

Every account sees every feeder; there are no per-feeder grants. A member who cannot list clients still gets their own phone's key through the server, which records which account it belongs to (SV24).

### Sessions, tokens and passwords

- A browser session lasts 30 days from its last use. A phone token lasts until revoked unless it was given an expiry. An account's expiry overrides both.
- An account past its expiry is disabled, not deleted: its sessions and tokens stop, its name stays in the audit, and an owner extends or deletes it. The jobs mark it; nothing is removed.
- A password has at least 10 characters, with no other rules. An owner sets a temporary password for another user. A sole owner who is locked out runs `wellfed-server reset-owner` on the host.
- Failed logins are limited to 5 a minute per name and 20 a minute per address, in token buckets held in a bounded map (10,000 entries, least recently used evicted), answered `429` with `Retry-After`. No account is ever locked, and other sessions are unaffected.

## Commands and feeds

- **The journal row** is written before the command is sent: the idempotency key, user, token, feeder, kind, a canonical hash of the request body, the device request id, the feeder's boot count, the state (`sent`, `answered`, `unknown`, `resolved`), the outcome and, for a feed, its number.
- **A key reused with a different body** is refused `422 key_reused` and never sent.
- **`POST /feeders/{id}/feed`** answers `202` with the journal entry once the feeder answers `Feeding { feed }`. The `Fed` outcome arrives on the stream and at `GET /commands/{key}`. There is no long poll.
- **`GET /commands/{key}`** reads one entry (the caller's own; an owner's any). `GET /commands?feeder=&state=` lists them, so `state=unknown` is the uncertain list.
- **No server-side feed limits.** The feeder's own refusals are the policy.
- **The journal is kept forever.** It is the audit's source and part of the export.
- **Removing a feeder** revokes the server's key on it (or reports that it could not) and archives the feeder. Its events, samples and journal stay readable and exported. Adopting the same identity again un-archives it and continues its history.
- **Installs** are owner-only and one at a time per server: a second is refused `409 install_running`.

## Household and time

`POST /setup` requires the household's IANA zone, prefilled from the host's. `PATCH /household` changing the zone pushes the POSIX rule to every feeder; a feeder that cannot be reached gets it when it reconnects.

## Owner data and derived views

- **Pets:** name, species, birth date, a weight log and a daily calorie target. Photos wait for the UI.
- **Pets and feeders** are many-to-many. Intake from a feeder that several pets share is split evenly between them, and the view labels it an estimate, since the feeder cannot tell who ate.
- **Food:** each feeder has one current food, with a history of when each started, so a past feed's calories use the food it served.
- **Refills:** an owner records one with `POST /feeders/{id}/refills` (grams, or a full bag). A food-beam change from low or empty to ok with no refill recorded shows "refilled?" in the food view and does not reset the count.
- **Portion size** *(provisional; the owner reviews it in the morning):* a portion is 1/12 cup, taken as FW18's calibration. Each food carries `grams_per_cup` (default 110), so grams = portions × 1/12 × `grams_per_cup`.

## Data plane

- **Samples:** raw samples are kept forever by default, and the owner may set a raw retention in days. Hourly rollups are always kept.
- **Debug log (SV14):** an owner can follow it live on the stream. The mirror tails it only while someone is following, and none of it is stored.
- **Alerts (SV70):** a feeder offline for more than 10 minutes (the only setting), any feed that ends `Uncertain`, `Failed` or `Expired`, an empty hopper, a failed install, a clock invalid for more than an hour, and a feeder `Blocked` for more than 30 minutes. An alert clears itself when its condition ends, an owner may acknowledge it, and each change goes on the stream.

## Releases, data and operations

- **Releases:** fetching is off by default (`releases.fetch`). When it is on, a job checks the project's GitHub releases daily, and an owner can ask for a check at any time. Uploads always work. Nothing installs without an owner's `POST /feeders/{id}/install`.
- **Export:** a streamed tar with `manifest.json` (format version, server version, row counts, hashes) and a `.jsonl` per table, or `.csv` files with `?format=csv`. It carries no secrets, so after an import the feeders are archived and must be adopted again. Import runs only on a fresh server (no accounts), in one transaction.
- **Backups:** `VACUUM INTO data_dir/backups/` daily at 03:30 household time, keeping the last 7, plus `POST /backups` on demand. The key file is not copied into the backups; its place is documented beside them.
- **Configuration:** `data_dir/server.toml`, overridden by flags and environment variables. The sealing key moves to `data_dir/keys/server.key` (mode 0600), migrated from today's `config.json` on the first start.
- **CLI (`wellfed server`):** human-readable text by default, and with `--json` the API's bodies verbatim. It exits 0 on success, 1 on an error and 3 when an outcome is unknown.

## Left to the implementer

Technical calls the agent makes and records in the run log: how SV22 serves the two lanes (pipelining over the client's pending list or interleaving), after checking what the feeder serves per client per pass; module and type names; the test layout within SV05's kit.

## Open questions

Questions a parked card raises, for the owner.

## Run log

Each deviation from the board's rules or this spec, with its reason.

- **SV01, arrows as a table.** The diagram's arrows are enforced as an explicit table of the workspace crates each server crate may take (in the API design), not their transitive closure, so the binary cannot reach past the service into the mirror. Edges the diagram leaves implicit are rows: the mirror takes `wellfed-api`, `wellfed-model`, `wellfed-client`, `wellfed-proto` and `wellfed-session`; the service takes `wellfed-store`, `wellfed-api`, `wellfed-model` and `wellfed-formats` (export); the test kit takes what it builds. The service also keeps `wellfed-client`, `wellfed-connect`, `wellfed-proto` and `wellfed-session` until SV07 moves the feeder tasks, which then removes them from the table.
- **SV01, rule scope.** Dev-dependencies are exempt from the arrows (tests build simulated feeders and whole servers) but not from the sqlx rule. "Below the server" for axum includes `wellfed-server-client`. The `wellfed-api` allow-list covers its direct dependencies; a new one needs an Android build check by hand, since `make check` builds no Android target.
- **SV01, the app's bridge crate.** The arrows name it `wellfed-app-bridge` (the crate map gave it no name), allowed into `wellfed-api` only; FP19 keeps that name or changes the table with it.
- **SV02, scope.** Only the server crates' pins moved. Host tools that pin the same versions (`wellfed-stock`'s `reqwest` and `tracing`, the tools' `sha2` 0.11) keep their own lines, to stay out of the firmware run's manifests; they can take the workspace entries later with no version change.
- **SV04, header order and the required key.** The header extractors run before the JSON body, so a request with a bad body and a missing or bad `If-Match` or `Idempotency-Key` now gets the header's problem first; a request with one fault gets the same answer as before. A route that requires `Idempotency-Key` refuses a request without one as `invalid` (400): `precondition_required` stays the code for a missing `If-Match`.
- Foundations SV01+SV02, SV03 and SV04 ran in parallel branches (different crates) rather than one at a time, to fit the night; each rebased on master before merging.
- **SV03, the epoch migration's test.** The squash removed the epoch migration, so its upgrade test (`the_epoch_migration_keeps_the_history_under_epoch_zero`) went with it; every other store test asserts what it did. A development database made before the squash no longer opens (its applied `0001` checksum differs and `0002` is unknown to the build): delete it and adopt again.
- **SV06, scope of the rule.** Clippy reads only the nearest `clippy.toml`, so the server's rule lives in `src/server/clippy.toml`, which repeats the root file's settings (`cargo xtask check-repository` refuses it when one is missing). Besides `Timestamp::now` and `SystemTime::now` it refuses `Zoned::now`, `SystemTime::elapsed`, `Uuid::now_v7` and `uuid::Timestamp::now` (each reads the clock), and std's `Instant::{now, elapsed}`. User and feeder ids stay UUIDv7, built from the `Clock`'s time and the injected `Random`. Log line timestamps (`tracing_subscriber`) are not time the server keeps and stay on the host clock. Monotonic deadlines stay, on `tokio::time::Instant`: they are not wall-clock time and a paused test runtime controls them, so they are not routed through `Clock`. `TimeZone::system()` reads no clock and stays in `main.rs` until SV30.
- **SV05, the server's tests over real HTTP.** The API tests drove the router in process (`oneshot`); they now run against the server on a localhost port through `wellfed-server-client`, so a request carries the address as its `Host` (an IP the server always answers to) rather than a configured name. Every test now runs on the kit's one config (sealing key, a one-hour session) with `FixedClock` and a seeded `Random`, where the API and end-to-end tests used the system's: nothing they assert depends on either.
- **SV05, requests the typed client cannot make.** A malformed request (no `If-Match`, a malformed key, a body missing a field) goes through `Client::request`, with the client's base, origin and credentials; one that must leave out the origin or name another host goes through `Client::http`. Their answers are read with the client's own `expect_status` and `decode`, so a refusal is still the typed `Problem`. Where a test checks the `Set-Cookie` header itself, it makes that call this way too.
- **SV05, the client's choices.** The session cookie's name moved into `wellfed-api` (`SESSION_COOKIE`), which the server and the client share. The client sends the server's own `Origin`, since the server's CSRF check refuses a cookie write without it. TLS is reqwest's `rustls` feature with its aws-lc provider: `rustls-no-provider` on ring was tried, but features unify across the workspace and every other reqwest client (wellfed-stock's) then panicked on build with no provider installed. Writes take a required `Idempotency-Key`, though the routes accept a write without one.
- **SV05, rustls with two providers.** The workspace now builds rustls with both ring (`mqtt5`) and aws-lc (reqwest's `rustls`), so rustls cannot pick a process default from its features: any code that builds a rustls config itself uses `ClientConfig::builder_with_provider` (or `ServerConfig`'s) with the provider named, never `builder()`. reqwest names aws-lc itself.
- **SV60, the old key file is deleted.** After `keys/server.key` (the key's 32 raw bytes) is written and synced, `config.json` is deleted rather than renamed, since it held only the key and a renamed copy would leave the secret in a second file; the server logs the move. A start interrupted between the two finds one key in both and finishes; two different keys are refused, and so is a `server.db` with no key file (a backup restored without its key), rather than making a new key that cannot open the stored feeder keys.
- **SV60, settings beyond the list.** `backups.keep` (default 7) joins the spec's settings, so SV64's count is the owner's; the 03:30 time stays fixed. `trusted_proxies` takes single addresses, not CIDR ranges; SV32 widens it if a proxy needs a range. A flag or `WELLFED_SERVER_*` variable replaces the file's value whole (`--host` given once replaces `hosts`), and `--no-discovery` stays as the short form of `--discovery off`, winning over `--discovery` and its variable (clap counts a set variable as given, so the two cannot be a clap conflict). A list override cannot clear the file's list. An existing loose `keys` directory is tightened to 0700 with a warning rather than refused, since the server's own user owns it. The file is parsed with `toml` and `serde_path_to_error` (both now workspace dependencies), so a refusal names the key's path and its line.
- **SV60, the key needs hard links.** The key file is linked into place (`hard_link`, which never replaces an existing file), so the data directory must be on a filesystem with hard links; SV62's packaging docs say so (a volume without them, such as some network or FUSE mounts, fails the first start with a write error). A staged `server.key.new` left by a crash between the link and its removal is removed on the next start.
- **SV07, the mirror's edges.** Adoption's network steps moved with the sessions: `Mirror::locate` (the address, or mDNS) and `Mirror::enroll` (redeem the invite) hold the only `wellfed-connect` calls, and the mirror re-exports `wellfed-client`'s `Invite` and `InviteText` for the service to parse an invite. The service seals and unseals the feeder key, and hands the mirror the unsealed secret. `Clock`, `Random` and `RandomFailed` live in the mirror (the lowest server crate that needs both), re-exported by the service. The stored event body's decoder (`stored_event`) is the mirror's, beside its encoder. The service's hex for session tokens comes from `wellfed-formats` (already on its row), with the same lowercase-only rule, so the service row drops `wellfed-session` too.
- **SV07, the forwarder.** The service forwards each feeder's broadcast into the hub with a task per feeder, and keeps each feeder's link from that forward rather than reading the mirror's: the list then never shows a link the stream has not sent (reading the mirror's would let a test see `online` before the stream did). A forwarder that falls behind the broadcast (1024 updates) drops its backlog (resubscribes), publishes `resync`, then the link read after resubscribing, where the old in-line publish could not lag; when the broadcast closes and the mirror no longer holds the feeder, the forwarder drops its link entry. `Mirror::start` on a running id and `Mirror::stop` wait for the old task to end, and a task ends by sending `offline`, so a stopped feeder's last update is never `online` and two sessions never overlap.
- **SV07, the long-operation turn.** `Mirror::operation` waits for the feeder's one long-operation turn and returns an `Operation` whose `step` goes through the same queue as `ask`, with the same answer deadline and the same unknown and unsent rules. Nothing uses it yet; SV13, SV22 and SV51 do.
- **SV07, error domains.** `Error` wraps `Store`, `Invalid` (field checks, shared by every domain), `Account`, `Feeder` (adoption and the stored history), `Command` (anything sent to a feeder) and `Random`. A feeder's refusal of an invite is `FeederError::Refused` and of a command `CommandError::Refused`; both answer `feeder_refused`. Three internal failures that reused the command's "unexpected answer" are now named (`AccountError::Hashing`, `AccountError::Expiry`, `FeederError::Lookup`); each still answers `internal` with no detail. The mirror's refusals (`AskError`, `ConnectError`) map onto these variant by variant, so no HTTP code changed.
- **SV08, what the guard counts.** Rust sources only: the generated `openapi.json` (about 2,200 lines) and the sqlx metadata follow what they describe and cannot be split, so they are out of scope by type, not exempt by name. "Under `tests/`" means any `tests` directory in the path (an integration test and its modules, or a `src/tests/` module tree); a `tests.rs` file beside the code it tests keeps the 400 limit. Nothing was over when the guard landed.
- **SV63, the readiness route.** `GET /api/v1/status`, under the API's prefix like every authenticated route (the session cookie's path covers it either way), with the existing `Owner` extractor until SV30 moves it onto the `Permission` table. A failing store is reported in the body (`store: failing`, the error logged under the request's id) rather than as a `500`, so the view answers what it can; the feeder list comes from the store, so it is empty then. `jobs` is a list of `JobRun` (`retention`, `rollups`, `expiry`, `backup`; started, finished, outcome) that stays empty until SV12's supervisor records runs; SV12 may reshape `JobRun` freely (no release yet).
- **SV63, the request id.** tower-http's `SetRequestIdLayer` with random UUIDs, outside the trace layer so the `request{id, method, path}` span carries it, and `PropagateRequestIdLayer` inside it, so a refusal from the host, origin or size checks carries `x-request-id` too. A request that brings one `x-request-id` of at most 64 bytes of `[A-Za-z0-9._-]` keeps it (a reverse proxy's, so both logs share it); any other id, or several, is dropped before the id layer and replaced with a fresh UUID, so a client cannot fill or garble the log with it. The span leaves the query string out. The service's per-feeder forward runs under the same `feeder{id}` span as the mirror's task. Both spans are roots (`parent: None`), so a feeder adopted over HTTP does not log under that request's id for as long as it runs.
