# Server board

This board tracks the architecture and build-out of the owner's server (`src/server/`): the crates, their boundaries, and each capability the product needs from them. [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) tracks the SDK and hardware, and [FEATURE_BOARD.md](FEATURE_BOARD.md) tracks owner-visible behavior. FP18 (server mirror), FP21 (server web UI) and FP22 (device health) stay on the feature board as the acceptance cards; the SV cards here are the engineering that gets them there. The contracts are in the [API design](docs/api/README.md), the product decisions in [product architecture](docs/firmware/product-architecture.md) and the UI in the [web UI design](docs/web/README.md).

Drafted October 26, 2026, from a read of the four server crates and the docs above (rechecked at `bb244c8`), with the owner's decisions from the same day. Every finding here is **static analysis**. No server has met a real feeder yet.

## Using the board

Cards use the FIRMWARE_BOARD workflow (**Planned → Ready → In progress → Done**) and its evidence labels. A card is Ready when its prerequisites are Done. The [foundations](#foundations) come first: every feature card after them adds a module, a table group or a route file, so the code should be shaped for that before the features arrive.

A card never relaxes the feeding-safety rules: every feed goes through the device API, a feed keeps one idempotency key from confirmation to a terminal result, and an uncertain feed is never retried under a new key or over another path. Every path that can request a feed has a test that retries it and proves one feed.

## Run rules (owner, October 26)

- Each card runs the review loop: one implementer agent in its own worktree branched from current `master`, one reviewer agent resumed until it reports nothing blocking (three rounds at most, then the card parks on its branch for the owner). Then the card is rebased on `master`, `cargo fmt --all` and `make check` pass on the result, and it is committed to `master` and pushed.
- One card at a time through the foundations: they edit the same crates, so parallel branches would conflict.
- **Sequencing is API first, UI after:** the server API is built across the board, and FP21's screens follow once the cards they need are Done.

## Decisions (owner, October 26)

| Topic | Decision |
| --- | --- |
| Scale | A household: 1–5 feeders per server. A task per feeder, one SQLite writer and the in-memory hub stay |
| Exposure | LAN by default, and a documented public setup behind a reverse proxy |
| TLS | The reverse proxy only. The server trusts a configured proxy for the scheme and the client's address |
| App path | Server first when one is configured, direct to the feeder as a fallback (see [two paths](#two-paths-one-feed)) |
| Migrations | Squash freely until the first server release; append-only from then on |
| Roles | Owner, member and viewer (read-only). A login or token may carry an expiry, for a pet sitter |
| Releases | Fetched from GitHub releases, or uploaded by the owner; both verified the same way |
| Notifications | Through the Android app's push, later. The server decides what is an alert; the app delivers it |
| Backups | Separate from export: SQLite copies with the key file for restore, export for moving servers and analysis |
| Derived views | Computed from the server's own history, in the service; no crate shared with the app |
| Time zone | One per household; the server syncs every feeder's clock to it |
| CLI | Speaks both: the device API as today, and the server API under `wellfed server` (SV34). One path per command, never a fallback between them |
| File size | 400 lines per file, 600 under `tests/`, in `src/server/` and `src/client/` (SV08) |
| Service types | The service keeps returning `wellfed-api` bodies until a second API version exists |
| Packaging | A static binary with a systemd unit, and a container image |

### Two paths, one feed

Most owners will run a server, so the app talks to it whenever it is reachable: remote access works, and the audit log names the person. Each phone still keeps its own key on every feeder, and uses it on the LAN when the server is down, for reads and new intents only. The danger is a retry that crosses paths: the server and the phone send different device request ids, so a feed sent directly that timed out, then retried through the server, could feed twice. So an uncertain write never switches path. It is retried on its own path with its own key, or resolved from the feeder's history (a feed by its `Fed` event), never resent on the other.

This costs the server an operation for everything the app does (SV24), and the feeder a client slot per phone (8 slots: the server, the CLI and up to six phones).

## What the server has to carry

| Area | Needs | Source |
| --- | --- | --- |
| Mirror | One task per feeder: sync from the cursor, clock sync, raw events, gaps and samples, reconnect and discovery | FP18 (built) |
| Commands | Settings, lock, feed and cancel, plans, clients and invites, Wi-Fi, reboot; idempotent, uncertain outcomes kept uncertain | FP03, FP05, FP18, FW17 |
| App parity | Every operation the app makes, since the app goes through the server first | Decision above |
| History and charts | Event pages across feeders, samples raw and hourly, retention, per-feed traces, stats, the debug log | FP06, FP22, FW54–FW56 |
| Derived views | Food remaining and refill estimate, intake against pet targets, feeding anomalies, hardware health | [Data](docs/firmware/product-architecture.md#data) |
| Owner data | Household (name, units, time format, zone), pets (weight log, target), foods (kcal per gram, bag size) | API design |
| Accounts | Owner, member and viewer; phone tokens; expiring access; login limits; the proxy setup; audit | API design, decisions |
| The CLI | The server API beside the device API, for scripting and owners at a terminal | Decisions |
| Alerts | Missed or uncertain feeds, an empty hopper, a feeder offline, delivered by the app later | Decisions |
| Releases | Fetched or uploaded releases, verified; owner-approved install with progress; the stock backup and rollback | FW42, FW43, FW48 |
| Data | Export and import, backups | API design |
| Delivery | The embedded web UI, a config file, the binary and the container, health and logs | FP21, web UI design |

## Findings

What the first slice gets right, and where it will not scale as features land.

**Keep:**

- **The layering holds.** `wellfed-server` has no SQL, `wellfed-service` no axum, `wellfed-store` no protocol logic; a handler parses, authorizes, calls one method and maps the result.
- **The feeding-safety core is right.** A command that times out is `outcome_unknown`, never failed (`wellfed-service/src/feeder.rs`, `run`). A command whose caller left is dropped unsent. The device request id binds the user, the feeder, the kind of write and the key (`feeders.rs`, `write_id`).
- **The mirror is idempotent.** Events are keyed by feeder, epoch and sequence, and the cursor moves in the insert's transaction (`wellfed-store/src/lib.rs`, `record_event`).
- **Determinism seams exist:** `Clock` and `Random` are injected, and tests run against the real simulator over TCP.

**Will not scale:**

1. **One `Service` for every domain.** Users, feeders, the per-feeder tasks, the hub and key sealing hang off one struct and one error enum (`wellfed-service/src/lib.rs`). With feeds, plans, pets, foods, intake, releases, installs, audit, alerts, export, retention and rollups to come, it becomes a god object, and every handler's error mapping matches one growing enum.
2. **The store is one 767-line `lib.rs`.** Every table group to come lands in it.
3. **Routes are one file** (`wellfed-server/src/routes.rs`: 8 resources of the design's 25 or so). `If-Match` and `Idempotency-Key` are parsed by helpers, not typed extractors, so each new write repeats them.
4. **Event bodies and sample values are stored only as postcard bytes.** That keeps the mirror raw, but SQL cannot filter, sum or roll them up. Food remaining, intake, charts, anomalies and alerts all need decoded values.
5. **One command at a time per feeder.** `wellfed_connect::Connection::ask` takes `&mut self` and waits for its answer, and the feeder task runs one command before taking the next. `wellfed-client`'s `Connection` already tracks several pending requests. An install, a trace fetch or stats paging would hold up status reads and feeds.
6. **The server keeps no record of its own writes.** Idempotency rests on the feeder's ring of 32 write ids. The audit log, the UI's uncertain state, the app's [path rule](#two-paths-one-feed) and a retry after the ring has moved all need a server-side record.
7. **No background jobs.** Retention, rollups, session expiry and backups have nowhere to run; session expiry piggybacks on login (`users.rs`, `open_session`).
8. **Clock leaks.** The hub's epoch and the store's pre-migration backup name read the wall clock directly (`hub.rs`, `Hub::new`; `wellfed-store/src/lib.rs`, `backup_path`), past the injected `Clock`. The feeders' zone is the host's (`main.rs`, `TimeZone::system()`), not a household setting.
9. **Dependency pins outside the workspace.** 16 server dependencies are pinned in the crates' own manifests (`axum-extra`, `tower-http`, `tracing`, `sha2`, `argon2`, `tokio-util`, `reqwest` and others), against the API design's rule that shared versions live in `[workspace.dependencies]`.
10. **Tests grow into single files.** `wellfed-service/tests/service.rs` is 721 lines, and each test file builds its own store, clock and simulator.

## Target architecture

A crate per runtime or dependency boundary, a module per domain inside it. A new domain (pets, alerts) is a module and some tables, not a crate, unless it brings its own runtime (tasks), a heavy dependency (`reqwest`) or a consumer outside the server.

```text
src/server/wellfed-api       HTTP bodies, error codes, OpenAPI schemas; a module per resource. Builds for Android
src/server/wellfed-store     SQL only; a module per table group, one Store. Raw rows plus projections
src/server/wellfed-mirror    the per-feeder runtime: connect, sync, ingest and projections, the command lanes, the hub
src/server/wellfed-releases  fetch, upload and verify releases, the release cache, the install driver
src/server/wellfed-service   the use cases: accounts, household, commands and their journal, pets and food,
                              derived views, alerts, audit, export
src/server/wellfed-server    the binary: config, axum routers per resource, auth, SSE, jobs, the embedded web UI
src/server/wellfed-server-testkit  a temp store, fixed clock and random, the simulator harness, an HTTP client
src/client/wellfed-server-client  a typed Rust client of the HTTP API over wellfed-api: the CLI and the server's own tests
```

Allowed dependencies (each arrow means "may depend on"):

```text
wellfed-server ─► wellfed-service ─┬─► wellfed-mirror ─► wellfed-store ─► wellfed-api ─► wellfed-model
       │                             │          └─► wellfed-connect ─► wellfed-client
       │                             └─► wellfed-releases ─► wellfed-client (Installer), a release verifier
       └─► wellfed-api, wellfed-store (to open it)

wellfed (the CLI) ─► wellfed-server-client ─► wellfed-api
          └─► wellfed-connect (the device API, as today)
```

- **`wellfed-mirror`** owns every task that holds a feeder session. Its surface is small: start and stop a feeder, its link state, `ask` (one answer or unknown), a long-operation lane, and the hub. The service never sees a socket; the mirror never sees a user.
- **`wellfed-store`** stays one crate, so the sqlx offline metadata and the migrations stay in one place, split into `users`, `feeders`, `history`, `samples`, `owner` (pets and foods), `commands`, `releases` and `jobs` modules.
- **Derived views** are a service module over the store's projections. They are pure functions of typed rows, with "now" and the household zone passed in, so they are tested without a database.
- **The service's errors split by domain** (`AccountError`, `CommandError`, …), wrapped in its `Error`, as the [Rust standard](docs/development/rust.md) asks of a module with several refusals.

## Current status

- **Done:** the first slice (FP18, Oct 24, host simulation): setup and login, adoption by invite, the mirror, settings, lock, history pages and the stream.
- **Ready:** SV01–SV05.
- **Next:** the foundations, in this order: SV02, SV03, SV04, SV05, SV01, SV06, SV07, SV08. They change no behavior.
- **Then:** the command plane (SV20, SV21) and the public setup's accounts (SV30–SV32), the CLI on the server API (SV34), then data, owner data and releases.
- **Critical path to a feed from the web UI:** SV07 → SV20 → SV21. FW17 (feeds that survive a reboot) is done.
- **Waiting on firmware:** SV13 (FW54, FW55), SV14 (FW56 on hardware), SV23 (FP03), SV52 (FW42, FW43).

## Foundations

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV01 Server architecture record** | Ready | — | — | The [target architecture](#target-architecture) and the [decisions](#decisions-owner-october-26) move into the [API design](docs/api/README.md#crates): the crate map, the allowed dependencies, the "crate per runtime or dependency boundary" rule, the service returning `wellfed-api` bodies, migrations squashed until the first release, and the [two-path rule](#two-paths-one-feed). The dependency rules are enforced through cargo-deny `wrappers` or `cargo xtask check-repository`, so nothing above the store links sqlx and nothing below the server links axum. |
| **SV02 Workspace dependencies** | Ready | — | — | Every server dependency comes from `[workspace.dependencies]` (finding 9); `make check-deps` passes with no new duplicate. |
| **SV03 Store modules** | Ready | — | — | `wellfed-store` splits into a module per table group, with the conversion helpers (`millis`, `to_i64`, …) in one module. The pre-migration backup takes its time from the caller (finding 8). Migrations `0001` and `0002` squash into one (no release yet). Store tests pass unchanged in what they assert. |
| **SV04 Routes and extractors** | Ready | — | — | `wellfed-server` has a route module per resource, matching `wellfed-api`'s modules, and typed extractors for `If-Match` (a `Generation`) and `Idempotency-Key` (required or optional per route) that document themselves in the OpenAPI. `openapi.json` changes only where the extractors make it more exact. |
| **SV05 Server test kit** | Ready | — | — | `wellfed-server-testkit` holds what every server test builds: a temp store, a fixed `Clock` and a seeded `Random`, the simulator over TCP and an in-process HTTP client with cookies. Service and API tests split into a file per domain. |
| **SV06 One clock** | Planned | — | SV03 | No server crate reads the wall clock outside `SystemClock`; the hub's epoch comes through `Clock` (finding 8). Clippy `disallowed_methods` on `Timestamp::now` and `SystemTime::now` in the server crates keeps it that way. |
| **SV07 `wellfed-mirror`** | Planned | — | SV01, SV05 | The per-feeder tasks, ingest, the command channel and the hub move out of `wellfed-service` into `wellfed-mirror`, behind the surface the [target architecture](#target-architecture) names. The service keeps users and the use cases, and its error enum splits by domain. Every service and end-to-end test passes unchanged in what it asserts. |
| **SV08 File size guard** | Planned | — | SV03, SV05, SV07 | `cargo xtask check-repository` refuses a file in `src/server/` or `src/client/` over 400 lines, or over 600 under `tests/`. At `bb244c8` ten files are over. SV03, SV05 and SV07 split the server's four (`wellfed-store/src/lib.rs` 767, `wellfed-service/tests/service.rs` 721, `feeder.rs` 480, `feeders.rs` 416); this card splits the client's six: `wellfed-client/tests/api.rs` 2666 and `tests/update.rs` 1181, `wellfed-model/src/convert.rs` 1300, `wellfed-client/src/api.rs` 594 and `src/connection.rs` 466, `wellfed-connect/src/lib.rs` 446. None is exempt when the card closes. |

## Command plane

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV20 Command journal** | Planned | — | SV07 | Every write through the server is recorded before it is sent: key, user, feeder, kind, device request id, and its state (sent, answered, unknown) with the outcome (finding 6). A retry with the same key reads the journal first, so it never needs the feeder's ring to have kept the id. The journal feeds the audit log (SV33), the UI's uncertain state and the app's path rule. Tests: a retry after the feeder's ring has moved returns the first answer; a server restart between send and answer leaves the write unknown, never failed. |
| **SV21 Feed and cancel** | Planned | — | SV20 | `POST /feeders/{id}/feed` requires `Idempotency-Key`. Its answer names the feed number; the `Fed` event is correlated to it, and the result reaches the stream. `POST /feeders/{id}/feeds/{feed}/cancel`. Tests against the simulator: a double submit, a retry after a dropped answer, a server restart mid-feed, a cut session and a feeder reboot mid-feed each end with exactly one feed. |
| **SV22 Command lanes** | Planned | — | SV07 | The feeder task keeps short commands (status, settings, lock, feed) apart from long operations (install chunks, trace and stats paging; finding 5), so a long operation yields between its steps and a feed or a status read waits at most one step. Either `wellfed-connect` pipelines requests over the client's pending list, or the task interleaves; the card first checks what the feeder serves per client per pass. |
| **SV23 Plans** | Planned | — | SV20, FP03 | `GET`/`PUT /feeders/{id}/plans` with `If-Match`, read from the feeder as settings are; a stale write gets `412` with the current plans. The next meal on `GET /feeders/{id}`, computed with `jiff` in the household zone across DST. |
| **SV24 App parity** | Planned | — | SV20 | Every operation the app makes has a server route through the journal: `GET /feeders/{id}/clients` and revoke, an invite for another client (a phone joining a feeder through the server), Wi-Fi, reboot, and removing a feeder from the server (revoking its own key first, or saying it could not). A test checks the app's operation list against the routes, so a new device op without a route fails. |
| **SV25 The two-path rule** | Planned | — | SV20, SV21 | The [two-path rule](#two-paths-one-feed) is in the API design and the device API doc, and the server's half is enforced: a write's answer says whether its outcome is known, an uncertain feed's resolution is readable by key (from its `Fed` event), and the stream reports it, so the app can resolve a feed it lost on either path without resending it. The app's half is FP19's. |

## Accounts and security

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV30 Users, roles and household** | Planned | — | SV04, SV07 | `/users`: owners create, change roles and delete, never the last owner. `Role` gains `Viewer` (status and history, no writes), checked in the auth extractors. A user may carry an expiry, after which their sessions and tokens stop. `/household`: name, units, time format and the IANA zone the feeders' clocks sync to (finding 8). Deleting a user ends their sessions and tokens. |
| **SV31 Phone tokens** | Planned | — | SV30 | `/tokens`: a bearer token shown once and stored hashed, named for its user and device, with its last use and an optional expiry. The auth extractor takes a cookie or a token; token-authenticated writes skip the `Origin` check only cookies need. |
| **SV32 Login limits and the proxy** | Planned | — | SV30, SV60 | Failed logins are limited per name and per address. The scheme (`Secure` cookies) and the client's address come from a configured trusted proxy only. A flood of logins neither locks out the owner's other sessions nor exhausts memory. The public setup is documented: a reverse proxy terminating HTTPS (Caddy as the example), the trusted-proxy setting, and what a member's phone needs. |
| **SV33 Audit log** | Planned | — | SV20, SV30 | `GET /audit?after=&limit=` from the command journal and account changes: who did what, when, through which token, with the outcome. |
| **SV34 The CLI on the server API** | Planned | — | SV04, SV31 | `wellfed server …` speaks the server API: `login` (a token from SV31, kept beside the CLI's feeder keys, mode 0600), `logout`, and the device commands' verbs over the server (`feeders`, `status`, `settings`, `lock`, `feed`, `cancel-feed`, `events`, `clients`, `revoke`, `reboot`), plus what only the server has (`users`, `tokens`, `releases`, `install`, `export`) as each card lands. It goes through `wellfed-server-client`, a typed client over `wellfed-api` that the server's end-to-end tests use too, so the CLI and the tests cannot disagree with the routes. A command takes one path: an uncertain write prints its key and the command that resolves it, and is never resent over the device API. The CLI's `main.rs` splits into a module per command group on the way. |

## Data plane

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV10 Projections beside the raw log** | Planned | — | SV07 | Ingest decodes each event once and writes its projection rows in the raw insert's transaction: a `feeds` table (feed number, source, requested and delivered portions, outcome, reason, times), lock and food-level changes, boots and resets (finding 4). The raw log stays the truth: a projection version, and a rebuild from raw when it changes. Tests: projections equal a rebuild after random ingest, replays included. |
| **SV11 Samples, rollups and retention** | Planned | — | SV10, SV12 | Samples carry a numeric value beside the raw bytes, written in small batches. Hourly rollups (count, min, max, mean) are kept incrementally. `GET /feeders/{id}/samples?metric=&from=&to=&resolution=raw\|hour`, and the owner's raw retention keeps rollups past it. A month of five feeders' samples queries quickly in a test, with the figures recorded. |
| **SV12 Background jobs** | Planned | — | SV07 | One supervisor in the binary runs typed jobs on the cancellation tree: retention, rollup catch-up, expired sessions and users, and backups (SV64). Each job records its last run and result, runs at most once at a time, and runs on a virtual clock in tests (finding 7). |
| **SV13 Feed traces and statistics** | Planned | — | SV10, SV22, FW54, FW55 | After each `Fed` event the mirror fetches that feed's trace and stores it with the feed; the stats pages are read and kept as samples. Both run on the long-operation lane, never ahead of a feed or a status read. Serves FP22's device page. |
| **SV14 Debug log** | Planned | — | SV07, FW56 | The owner can follow a feeder's debug log through the stream. Whether any of it is stored is decided on the card. |

## Owner data and derived views

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV40 Pets and foods** | Planned | — | SV30 | `/pets` (name, weight log, daily target) and `/foods` (brand, kcal per gram, bag size), with which feeder serves which pet and food. Photos are files in the data directory with size and type limits, never in the database. |
| **SV41 Derived views** | Planned | — | SV10, SV40 | A service module: food remaining and a refill estimate (grams per portion from FW18's calibration, portions since the last refill, the bag size), grams and calories against each pet's target, and feeding anomalies (missed, uncertain or short feeds). Pure functions of typed rows with "now" and the zone passed in, property-tested. `GET /feeders/{id}/food` and `GET /pets/{id}/intake`. |
| **SV42 Export and import** | Planned | — | SV10, SV40 | `GET /export` streams everything as JSON or a CSV archive without loading the database into memory; `POST /import` restores it onto a fresh server. The archive format is typed and versioned. A round trip (export, import into an empty server, export again) is byte-identical. |

## Alerts

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV70 Alert rules** | Planned | — | SV10, SV21 | A service module decides what is an alert (a missed or uncertain feed, an empty hopper, a feeder offline past a limit, a failed install) from the projections and link states, records each with its state (open, cleared) and publishes it as a typed stream event. `GET /alerts`. Delivery to phones is the app's push (with FP19): a self-hosted server cannot send through Google's push service alone, so the card that delivers chooses UnifiedPush, ntfy or a small project relay. |

## Releases and installs

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV50 `wellfed-releases`** | Planned | — | SV07 | Releases come from the project's GitHub releases over rustls, when the owner allows the server online, or as a file the owner uploads (offline homes, self-built images). Both are verified the same way, against the project key and the owner's enrolled key, with the verifier the feeder's own checks are tested against, and kept in a cache in the data directory. `GET /releases`, `POST /releases` (upload). Nothing installs without SV51. |
| **SV51 Install** | Planned | — | SV22, SV50 | `POST /feeders/{id}/install`, owner-only and per feeder, drives `wellfed-client`'s `Installer` through a tokio driver shared with the CLI (moved into `wellfed-connect`). Progress and the result reach the stream; feeds and status keep working during the transfer. Tested against the simulator's update path, with a cut session and a server restart. |
| **SV52 Stock backup and rollback** | Planned | — | SV51, FW42, FW43 | The server keeps the verified stock backup the installer took, shows its version and state, exports it as a file, and offers "Restore stock" through FW43's path. |

## Delivery and operations

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **SV60 Configuration** | Planned | — | SV04 | A typed config file in the data directory for everything that is not a secret or a household setting: listen address, host names, label, discovery, retention, trusted proxies, whether releases are fetched. The sealing key stays in its own file. Flags override the file, and a bad file is a typed error naming the field. |
| **SV61 Embedded web UI** | Planned | — | SV04, FP21 | `wellfed-server` serves the SvelteKit build embedded with `rust-embed`, with cache headers by content hash and a Content Security Policy, from the same origin as `/api/v1`. The Playwright suite runs against the real binary. |
| **SV62 Packaging** | Planned | — | SV60 | One static binary per target in the GitHub releases with an example systemd unit, and a container image. The image's docs ask for host networking for mDNS discovery; on a bridged network it adopts and reconnects by address with `--no-discovery`. A release check starts each fresh and adopts the simulator. |
| **SV63 Health and logs** | Planned | — | SV07 | `/health` stays unauthenticated liveness; an owner-only readiness view adds the store, each feeder's link and the jobs' last runs. Every log line from a feeder task carries the feeder, and every request a request id. Whether to expose Prometheus metrics is decided on the card. |
| **SV64 Backups** | Planned | — | SV12 | A scheduled `VACUUM INTO` copy in the data directory, kept by count, with the key file's place documented beside it, answering the API design's open question. A restore test opens a backup and its feeder keys. |
| **SV65 First real feeder** | Planned | — | SV07, SV20 | **Bench:** the server adopts the research unit, mirrors its history, changes settings and the lock, and keeps the history whole across a server restart, a feeder power cut and a Wi-Fi drop. Run under the owner's bench authorization, with the prior network and service state restored and recorded. |

## Status at a glance

✅ Done · 🚧 In progress · 🧪 Bench check · 🟢 Ready · ⬜ Planned. The cards above are authoritative.

| Card | Status |
| --- | --- |
| SV01 Server architecture record | 🟢 Ready |
| SV02 Workspace dependencies | 🟢 Ready |
| SV03 Store modules | 🟢 Ready |
| SV04 Routes and extractors | 🟢 Ready |
| SV05 Server test kit | 🟢 Ready |
| SV06 One clock | ⬜ Planned |
| SV07 wellfed-mirror | ⬜ Planned |
| SV08 File size guard | ⬜ Planned |
| SV10 Projections beside the raw log | ⬜ Planned |
| SV11 Samples, rollups and retention | ⬜ Planned |
| SV12 Background jobs | ⬜ Planned |
| SV13 Feed traces and statistics | ⬜ Planned |
| SV14 Debug log | ⬜ Planned |
| SV20 Command journal | ⬜ Planned |
| SV21 Feed and cancel | ⬜ Planned |
| SV22 Command lanes | ⬜ Planned |
| SV23 Plans | ⬜ Planned |
| SV24 App parity | ⬜ Planned |
| SV25 The two-path rule | ⬜ Planned |
| SV30 Users, roles and household | ⬜ Planned |
| SV31 Phone tokens | ⬜ Planned |
| SV32 Login limits and the proxy | ⬜ Planned |
| SV33 Audit log | ⬜ Planned |
| SV34 The CLI on the server API | ⬜ Planned |
| SV40 Pets and foods | ⬜ Planned |
| SV41 Derived views | ⬜ Planned |
| SV42 Export and import | ⬜ Planned |
| SV50 wellfed-releases | ⬜ Planned |
| SV51 Install | ⬜ Planned |
| SV52 Stock backup and rollback | ⬜ Planned |
| SV60 Configuration | ⬜ Planned |
| SV61 Embedded web UI | ⬜ Planned |
| SV62 Packaging | ⬜ Planned |
| SV63 Health and logs | ⬜ Planned |
| SV64 Backups | ⬜ Planned |
| SV65 First real feeder | ⬜ Planned |
| SV70 Alert rules | ⬜ Planned |

## Parked

- **Home Assistant over MQTT discovery** through the server (FP16, paused October 23). The hub is the seam; nothing here should assume it.
- **Remote access** beyond the documented reverse proxy: the owner's VPN.

## Open questions for the owner

- Whether the server exposes Prometheus metrics (SV63).
