# Server design

This page explains how the server is built, for contributors who want to change it. It covers the crates and their boundaries, storage, the per-feeder runtime, accounts, the command journal, and the server's larger features. For what the server does, read [the server](README.md). For running it, read [running the server](running.md).

The one idea: **the server is a mirror of each feeder, never a second source of truth.** The feeder owns feeding, its schedule and its journal. The server holds a session to each feeder, copies its history into SQLite, and forwards people's requests. Anything it isn't sure the feeder did, it settles from the feeder's history rather than doing again.

## Why it's built this way

- **REST JSON with Server-Sent Events, OpenAPI generated from code.** Server-to-client is all the live view needs, so SSE beats a WebSocket's custom reconnect and auth. GraphQL would need per-field authorization, and gRPC needs a proxy in browsers.
- **SQLite through sqlx, with compile-time-checked queries.** Self-hosters shouldn't have to run Postgres, and supporting both would mean writing every query twice.
- **Household accounts, plus a revocable token per phone.** One admin password gives no per-person audit. A proxy or OIDC alone is too hard for most owners, and can come later on top.
- **One household per server.** Multiple households would scope every query and every authorization check.
- **Layered crates, so the compiler enforces the boundaries.** One server crate would keep its layers only by review, and the HTTP types couldn't be shared with the app.
- **A concrete `Store`, tested against a fresh SQLite per test.** Repository traits with mocks are boilerplate, and mocks drift from real SQL.
- **`jiff` for times.** Zoned times, daylight-saving changes and schedules are easy to get wrong with chrono and chrono-tz.

## Crates

```text
src/sdk/wellfed-session      no_std, no alloc. Noise, HMAC, the setup code, length-prefixed frames, hex and base64
src/product/wellfed-proto    no_std, no alloc. Device-protocol messages, framing, versions and capabilities
src/product/wellfed-device-api no_std, no alloc. The feeder's side: the sans-IO session server, pairing, the setup exchange
src/client/wellfed-model     owned device API values (status, settings, events, requests, answers) and their conversion from the wire
src/client/wellfed-client    sans-IO. Noise sessions, pairing, invites, the Connection: keepalive, answers, event cursor, recovery
src/client/wellfed-connect   tokio loop over wellfed-client's Connection: locate, open, ask, request and receive (several outstanding), follow, join, recover,
                              and the update driver (Installer) the CLI and the server share
src/client/wellfed-server-client  a typed async client of the server's HTTP API over wellfed-api (reqwest with rustls,
                              the SSE stream included): the CLI and the server's own tests
src/tools/wellfed-feeder-sim the host-simulated feeder: wellfed-feeder's loop pass and stores over real TCP
src/server/wellfed-api       HTTP bodies, error codes, OpenAPI schemas; a module per resource. No axum. Builds for Android
src/server/wellfed-store     sqlx, migrations, typed queries; a module per table group, one Store. Raw rows plus
                              projections. The only crate that contains SQL
src/server/wellfed-mirror    the per-feeder runtime: connect, sync, ingest and projections, the command lanes (an
                              install, a stock restore or a read of bank 1 on the long one), and a typed broadcast of
                              each feeder's mirror events
src/server/wellfed-releases  fetch, upload and verify releases, the release cache, and each feeder's verified stock backup
src/server/wellfed-service   the use cases: accounts and auth, household, commands and their journal, pets and food,
                              derived views, alerts, audit, export, and the hub. No axum, no SQL
src/server/wellfed-server    the binary: config, axum routers per resource, the auth extractors, SSE, the jobs
                              supervisor, the embedded web UI
src/server/wellfed-server-testkit  a temp store, a fixed clock and random, the simulator harness, signed test releases, the server in process
src/app/                      Flutter app, plus a thin flutter_rust_bridge crate (wellfed-app-bridge) over wellfed-client,
                              wellfed-connect and wellfed-model, and wellfed-api once it has server screens
```

The map is the target. Today the server is `wellfed-api`, `wellfed-store`, `wellfed-mirror`, `wellfed-releases`, `wellfed-service` and `wellfed-server`, with `wellfed-server-client` and `wellfed-server-testkit` beside them. The embedded web UI isn't built yet, and the app talks to feeders only, not yet to a server.

The server's crates follow one rule: **a crate per runtime or dependency boundary, a module per domain inside it.** A new domain, such as pets or alerts, is a module and some tables, not a crate. It gets a crate only if it brings its own runtime (tasks), a heavy dependency (`reqwest`) or a consumer outside the server. That's why the mirror (the feeder tasks), the releases (`reqwest` and the verifier) and the HTTP client (the CLI) are crates, and accounts and alerts are not.

| Crate | What it owns |
| --- | --- |
| `wellfed-api` | Every HTTP body, a module per resource. It builds for every target, including the app's Android library, so it carries no server dependencies. `wellfed-service` returns these bodies directly until a second API version exists. |
| `wellfed-store` | All SQL, a module per table group (`users`, `feeders`, `history`, `projections`, `samples`, `owner`, `commands`, `releases`, `jobs`). One crate, so the sqlx offline metadata and the migrations stay in one place. |
| `wellfed-mirror` | Every task that holds a feeder session: connect, sync, clock sync, ingest, reconnect and the command lanes. It also holds the injected `Clock` and `Random` every server crate uses. It never sees a user, an alert or a stream id. |
| `wellfed-releases` | Fetching, uploading and verifying releases, the release cache, and each feeder's stock backup. |
| `wellfed-service` | The use cases, a module per domain, each with its own error enum wrapped in the crate's `Error`. It owns the auth rules, the hub and each job's body. It never sees a socket or an HTTP type. |
| `wellfed-server` | The binary. A handler parses, authorizes through an axum extractor, calls one service method and maps the result. It never sees SQL. The jobs supervisor lives here. |
| `wellfed-server-client` | The typed client the CLI's `wellfed server` commands and the server's own tests go through, so neither can disagree with the routes. Nothing in it retries. |
| `wellfed-server-testkit` | The service on a temporary store with a fixed `Clock` and a seeded `Random`, the simulated feeder, and the HTTP server in process. |

A few crates outside `src/server/` matter here:

- **`wellfed-model`** holds the device API's values as a host sees them: owned, with no lifetimes or wire buffers. They cross into Dart and into the server's store and HTTP API unchanged. `wellfed_model::convert` is the one place they meet `wellfed-proto`'s borrowed wire types. `wellfed-api` serves the model's status, settings and event bodies as they are, adding only what the server knows (when it received them, its own ids).
- **`wellfed-client`** holds no sockets, timers or radios. Its `Connection` owns every session rule: a `Status` keepalive after 45 s quiet, under the feeder's 90 s idle limit; the end of a session heard from not at all for 90 s; one answer or `Unanswered` per request; and the event cursor with its epoch.
- **`wellfed-connect`** is a thin tokio loop over that `Connection`. It takes its randomness from the caller, so the server can inject its generator and tests can fix it.
- **`wellfed-feeder-sim`** runs `wellfed-feeder`'s own API server, panel and pairing policy, on a flash image laid out like the feeder's storage. So service, end-to-end and web tests meet the feeder's real behavior, not a mock. A test can stall a feed in flight, and the harness's relay can hold back the feeder's answers or cut every session.
- **The export archive's** manifest and row types live in `wellfed-formats` (`wellfed_formats::export`), since export writes the archive and import reads it.

### Allowed dependencies

Each arrow means "may depend on":

```text
wellfed-server ─► wellfed-service ─┬─► wellfed-mirror ─┬─► wellfed-store ─► wellfed-api ─► wellfed-model
       │                             │                    │          └─► wellfed-formats (the export archive's rows)
       │                             │                    └─► wellfed-connect ─► wellfed-client
       │                             ├─► wellfed-releases ─► wellfed-store, wellfed-client (the release verifier)
       │                             └─► wellfed-store (projections), wellfed-api (the bodies it returns)
       └─► wellfed-api, wellfed-store (to open it)

wellfed (the CLI) ─► wellfed-server-client ─► wellfed-api
          └─► wellfed-connect (the device API, as today)
```

| Crate | May depend on (workspace crates) |
| --- | --- |
| `wellfed-api` | `wellfed-model` |
| `wellfed-store` | `wellfed-api` (its ids and enums), `wellfed-formats` (the export archive's rows, which it reads and inserts) |
| `wellfed-mirror` | `wellfed-store`, `wellfed-api`, `wellfed-connect`, `wellfed-client`, `wellfed-model`, `wellfed-proto`, `wellfed-session` |
| `wellfed-releases` | `wellfed-store`, `wellfed-api`, `wellfed-client`, `wellfed-model`; the verifier is `wellfed_client::release`, over the firmware's own offer check |
| `wellfed-service` | `wellfed-mirror`, `wellfed-releases`, `wellfed-store`, `wellfed-api`, `wellfed-model`, `wellfed-formats` |
| `wellfed-server` | `wellfed-service`, `wellfed-api`, `wellfed-store` |
| `wellfed-server-testkit` | `wellfed-server`, `wellfed-service`, `wellfed-store`, `wellfed-api`, `wellfed-server-client`, `wellfed-feeder-sim`, `wellfed-client`, `wellfed-connect`, `wellfed-model`, `wellfed-session`, `wellfed-core` (to sign test releases) |
| `wellfed-server-client` | `wellfed-api` |

`cargo xtask check-repository` enforces these from `cargo metadata` (all features, every platform). Each error names the edge or path that breaks them.

- **SQL stays in the store.** No crate but `wellfed-store` depends on sqlx, in its tests included.
- **HTTP stays in the binary.** Nothing below `wellfed-server` links axum, or any of axum's own crates (`axum-core`, `axum-extra`), by any path.
- **The arrows as drawn.** Each server crate's normal and build dependencies on workspace crates come from its row above. A crate outside the server takes a server crate only where drawn: the CLI takes `wellfed-server-client`, and the app's `wellfed-app-bridge` takes `wellfed-api`. Dev-dependencies are exempt, since a test may build a simulated feeder or a whole server. Rows may name crates that don't exist yet, so each is checked from its first manifest. Changing a row changes this table and the check together.
- **`wellfed-api` on Android.** Its own dependencies come from an allow-list (`jiff`, `wellfed-model`, `serde`, `utoipa`, `uuid`). `make check` builds no Android target.
- **Files stay small.** A Rust source under `src/server/` has at most 400 lines, or 600 under a `tests/` directory. A `tests.rs` beside the code it tests keeps 400. Generated files (`openapi.json`, the sqlx metadata) aren't counted.

Beyond the [Rust standard](../development/rust.md), the server keeps a few rules. If a change wants SQL in a handler or axum in the service, move the code rather than adding the dependency. The server maps service errors to `problem+json` in one `IntoResponse`, and there is no `anyhow` below the binary. IDs are newtypes, roles and metrics are enums, and generations are their own type. Every server dependency's version is pinned in `[workspace.dependencies]`, and a crate's manifest only picks features.

## Storage

- **SQLite in WAL mode.** One writer connection beside a small read pool. Every write goes through that connection, so writes never contend. A household's one to five feeders need no writer task in front of it.
- **Migrations** are sqlx's, embedded in the binary and applied at startup. Before pending migrations run, `VACUUM INTO` copies the database beside itself as `server.db.before-<version>-<unix ms>`. Until the first server release, migrations are squashed freely, since no owner's database has run them. From then on they are append-only, and a released migration is never edited.
- **Queries** use `sqlx::query!` and `query_as!`, with the offline metadata (`src/server/wellfed-store/.sqlx/`) committed, so builds need no database. `make sqlx-prepare` regenerates it from a scratch database that the `sqlite3` command line builds from the migrations.
- **Time** is stored as integer Unix milliseconds (UTC), with zones as IANA names. `jiff` converts at the store boundary. The store never reads the wall clock: `Store::open` is given the time.
- **Secrets.** The sealing key lives in `keys/server.key`, outside the database. Each feeder key is sealed with ChaCha20-Poly1305, bound to the feeder's row ID and identity. So a copied database alone can't impersonate the server.
- **Volume.** About 100k sample rows a day per feeder is 1–2 GB a year, which SQLite handles. Export streams rows, so it never loads the database into memory.

The tables:

| Group | Tables |
| --- | --- |
| Accounts | `household` (one row: name, units, time format, the IANA zone), `users` (with an optional expiry), `sessions` (with their last use), `tokens` |
| Feeders | `feeders` (identity, sealed key, cursor), `feeder_state` (cached plans, settings and status, with generation), `phone_keys` (the phones' keys the server invited), `kept_clients` |
| History | `events`, raw, keyed by feeder, epoch and `seq`, so the cursor sync is idempotent and a re-created ring's history stays apart; `gaps` |
| Projections | `feeds` (one row per feed, its latest event), `lock_changes`, `boots`, `clock_syncs`, `blocked_changes`: written in the raw insert's transaction, and rebuilt from `events` when `projection_version` changes |
| Samples | `samples` (feeder, metric, time, the raw value and its number), `sample_rollups` (count, min, max and sum per feeder, metric and UTC hour), `sample_marks` (the newest sample rolled up, and the number code's version) |
| Diagnostics | `feed_traces`, `feeder_stats`, `food_readings`: beside the history, never rebuilt from it |
| Commands | `commands` (the journal), `installs`, `job_runs` |
| Owner data | `pets`, `foods`, `pet_weights`, `pet_feeders`, `feeder_foods`, `refills` |
| Audit | `audit`: account changes as typed JSON, and a row per journal entry that the journal's own insert trigger adds |

Releases and stock backups are files in the data directory, not tables.

## Runtime

**One task per feeder.** The mirror's task owns the TCP connection and the `wellfed-client` session. It reconnects with jittered backoff and syncs events from the stored cursor before anything else (from the oldest event when the feeder's epoch is new). Then it sets the feeder's clock, again every 6 h. It records every event, gap and sample raw, each raw row with its projections and the cursor in one transaction. Samples go in small batches: 32, or `Timing::sample_wait` after the first, and at the session's end.

**Two command lanes.** Commands reach the task through a bounded channel per lane: short commands, and the steps of a long operation such as an install. A short command waits at most one step of a long one. Each lane has one request in flight, matched to its answer by id. A command times out as "unknown, retry with the same key", never "failed", because the feeder may have run it. A feeder heard from not at all for 90 s counts as gone, so a feeder that loses power shows offline within that time.

**The hub** is the service's. It merges each feeder's broadcast with command results, alerts and install progress, and assigns the stream ids `GET /stream` resumes from. A forwarder that falls behind drops its backlog and publishes `resync`. The SSE `id` is `<epoch>-<seq>`, with the epoch fixed at each start, so an id from before a restart resyncs.

**Discovery** browses mDNS (`mdns-sd`) for a paired feeder whose stored address fails, unless `discovery = "off"`.

**Shutdown** on Ctrl-C or SIGTERM goes through one `CancellationToken` tree. It ends the streams, stops the jobs and every feeder task, and closes the database.

**Determinism.** Time, randomness and the network sit behind small injected interfaces, so service tests run against the host-simulated feeder with a fixed clock. No server crate reads the wall clock outside `SystemClock`. `src/server/clippy.toml` refuses jiff's `Timestamp::now` and `Zoned::now`, std's `SystemTime::now`, `SystemTime::elapsed`, `Instant::now` and `Instant::elapsed`, and uuid's `Uuid::now_v7` and `Timestamp::now`. IDs are UUIDv7 built from the `Clock` and `Random`. Timeouts use tokio's `Instant`, which is monotonic and follows a paused test runtime. `SystemClock`'s `#[expect]` fails the build if that rule stops applying.

**Jobs.** One supervisor task in the binary (`wellfed_server::jobs`) runs the background jobs one at a time, waiting on the service's `Clock`. A job missed while the server was down runs once at the next start.

| Job | When | What |
| --- | --- | --- |
| `expiry` | Hourly | Deletes idle sessions and every session of an expired account |
| `rollups` | Hourly | Folds new samples into their hours |
| `retention` | 03:00 | Drops raw samples past `retention.raw_sample_days` |
| `backup` | 03:30 | Copies the database into `backups/` |
| `release_check` | 04:00 | Checks GitHub releases, while `releases.fetch` is on |

Daily times are in the household's zone (UTC before there is one).

**Health and logs.** `GET /health` is liveness: it reads nothing and answers the version. `GET /api/v1/status` is readiness for an owner: whether the store answers on its writer and a reader, each feeder's link, and each job's last run. Every log line from a feeder's task sits under a `feeder{id=…}` span, and every HTTP request under a `request{id=… method=… path=…}` span. The id comes back in `x-request-id`. The query string isn't logged. There is no metrics endpoint, since a household has no scraper to feed.

## Accounts and roles

- **Passwords** are argon2id-hashed, at least 10 characters, with no other rule. The server hashes at most two at once (about 19 MiB each), and an unknown name costs the same hash as a wrong password.
- **Browsers** get a session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` when a trusted proxy reports HTTPS). A session lasts 30 days from its last use. Every cookie-authenticated write needs an `Origin` matching the server, and `Sec-Fetch-Site: same-origin` when the browser sends it.
- **Phones and the CLI** get a bearer token, made from a login only: `plt_` and 64 hex digits, shown once and stored as its SHA-256. A token carries its user, not a role, so a role change applies at once. A request with `Authorization` is judged by it alone and skips the `Origin` check. From a token, making another token, setting a password or adding an account is `403 session_required`, so a leaked token can't mint others.
- **Expiry.** An account may carry an expiry, for a pet sitter. Past it, the account is disabled, not deleted: its login, sessions and tokens are refused `403 account_disabled` until an owner extends it. Owners never expire, and the last owner can be neither demoted nor deleted (`409 last_owner`).
- **Failed logins** are limited to 5 a minute per name and 20 a minute per address, in token buckets on the injected clock. A limited login is `429 login_limited` with `Retry-After`. No account is ever locked. The buckets live in one map of at most 10,000 entries.
- **The server has its own feeder key** per feeder, enrolled by invite like any client. So the feeder's own history shows the server as one client, and the server's audit log says which person it acted for.

Roles are owner, member and viewer. The service's `Permission` enum and its one `can(role, permission)` table hold the whole matrix. Each route names the permission it needs as a type (`Allowed<need::Control>`), and a refusal is `403 forbidden`. Every account sees every feeder.

| Permission | Covers | Owner | Member | Viewer |
| --- | --- | --- | --- | --- |
| `Read` | Status, history, samples and the food views | Yes | Yes | Yes |
| `Feed` | Feed, and cancel a feed | Yes | Yes | No |
| `Control` | Settings, the lock, a tune, a clock sync that doesn't step the clock | Yes | Yes | No |
| `InviteOwnPhone` | An invite for one's own phone | Yes | Yes | No |
| `OwnHistory`, `OwnAccount` | One's own commands, audit entries, password and tokens | Yes | Yes | Yes |
| `AllHistory`, `Users` | Everyone's commands and audit entries, and other accounts | Yes | No | No |
| `Household`, `OwnerData` | The household and feeders' time settings; pets, foods and refills | Yes | No | No |
| `FeederAdmin`, `Adopt` | Wi-Fi, reboot, a feeder's clients and its debug log; adopting and removing feeders | Yes | No | No |
| `ServerData` | Releases, installs, the stock backup, backups, export and import | Yes | No | No |

**The audit log** (`GET /audit`) is the command journal interleaved with account changes: setup, logins and failed logins with the client's address, users created, changed and deleted, passwords set, tokens made and revoked, and household changes. A journal row joins the log through a trigger in the statement that records it, so no write reaches a feeder unlogged. Owner data and adoptions aren't in it yet. It's kept forever.

## Commands and feeds

**The command journal** records every write before it is sent: its key, user, token, feeder, kind, the SHA-256 of the request's canonical JSON, the device request ID, and its state (`sent`, `answered`, `unknown`, `resolved`) with the outcome. The device request ID is a hash of the user, the feeder, the kind of write and the `Idempotency-Key`, since every user shares the server's key on the feeder. A key reused for a different request is refused `422 key_reused`. The journal is kept forever.

A retry under the same key reads the journal first:

- An **answered** write returns its recorded outcome without reaching the feeder.
- A **sent** write is still in flight, so a retry is answered `outcome_unknown` and sends nothing. A server that stops first leaves it `sent`, and the next start marks it `unknown`.
- An **unknown** write is sent again under the same device request ID when its kind allows: settings, lock and a cancel. A feed never is, because the feeder forgets request ids at a reboot.
- A **refused** write is final for its key, `409 feeder_refused` with the feeder's reason. Trying later is a new request with a new key.

**Feeds.** `POST /feeders/{id}/feed` (`{ "portions": n }`) requires `Idempotency-Key` and answers `202` once the feeder has reserved the feed in its journal. The answer carries the feeder's number for the feed, which keeps counting across restarts. There are no server-side limits: the feeder's refusals (`busy`, `invalid` past its cap) are the policy. The result is the feeder's `Fed` event, which settles the entry.

A feed whose answer was lost has no number and is never sent again. It is settled by the one `Fed` event after the history the server held when it was sent, from the server's key, when that match is unambiguous. Another client's feed or a panel feed never settles it. Once the feeder has restarted since, and the server holds its whole history with no gap, a feed the history still doesn't settle is `resolved` as `unrecorded`. Whether food was dispensed is then unknown, and it is never resent. A `Fed` event found later replaces `unrecorded`.

`POST /feeders/{id}/feeds/{feed}/cancel` goes through the journal too: `204` while the feed runs, `409 feeder_refused` `not_found` once it has ended.

**Settings** are always read from the feeder, never from the cache, because a change must name the current generation. An offline feeder's settings are `503 feeder_offline`.

**The zone.** The household stores its IANA zone by its canonical name. Every feeder keeps it twice: each clock sync carries its offset and next change, and the feeder's time settings hold its POSIX rule. So the feeder's clock and plans follow daylight saving with the server down. The rule comes from the tzdb bundled with `wellfed-client`, and is written whenever the feeder holds a different one.

## The CLI on the server

`wellfed server …` speaks this API through `wellfed-server-client` alone, beside the device commands. Each command takes one path ([two paths, one feed](README.md#two-paths-one-feed)).

- **Login.** `wellfed server login --server URL --name NAME` reads the password from standard input, opens a session, makes a token, and keeps it in `private/wellfed-server.json` (`--token-file`) with mode 0600, beside the device API's key file. Then it ends the session. `logout` revokes the token and deletes the file.
- **Verbs.** `feeders`, `status`, `settings`, `lock`, `feed` and `cancel-feed` each take `--feeder` with an id or a name when the server holds more than one. Then there are `events` (`--follow` over the stream), `command <key>` and `commands`, `users`, `tokens`, `releases`, `install`, `stock`, `clients`, `revoke` (a client by at least 8 hex digits of its key), `reboot`, and `export <file>` (`--csv` for spreadsheets).
- **Output.** Text by default. With `--json`, the API's bodies, one per line, a refusal's problem included. The exit status is 0 on success, 1 on an error and 3 when an outcome is unknown.
- **Uncertain writes.** A write goes out once under a fresh `Idempotency-Key`. When its outcome is unknown, the command prints the key and `wellfed server command <key>`, exits 3, and sends nothing more: not under a new key, and never over the device API. A feed waits on the stream for its result, opened before the feed is sent. An `uncertain` or `unrecorded` result exits 3 too.

## Samples

- **A number per sample.** Each stored sample keeps the feeder's bytes and one number to chart it by (`wellfed_api::sample_number`): `power` the battery's millivolts, `rssi` dBm, `heap` free bytes, `loop` the 99th-percentile pass in µs, `gateway` the round trip in ms, `wifi` the access-point changes, `supply` the battery's millivolts. When what a number means changes, `SAMPLE_NUMBER_VERSION` moves, and the next start renumbers every stored sample.
- **Hourly rollups** (count, min, max, mean per UTC hour) are folded in by sample id by the hourly `rollups` job, so hours a down server missed are filled later. A read at hour resolution adds samples past the last run from the raw rows. Rollups are kept forever.
- **Retention.** Raw samples are kept forever unless `retention.raw_sample_days` is set. Then the daily `retention` job deletes older raw samples in whole UTC hours, leaving their rollups.
- **Reads.** `GET /feeders/{id}/samples?metric=&from=&to=&resolution=raw|hour&after=&limit=` answers pages of 1–1000 items (default 100), oldest first.

## Releases

`wellfed-releases` keeps the signed updates an owner may install. Nothing installs from here.

- **A release file** is the version 2 manifest (160 bytes) followed by the image it names. On GitHub it is a release asset whose name ends in `.plota`. Drafts and pre-releases are skipped.
- **Verification** is `wellfed_client::release::verify`. It runs the feeder's own `admit_offer`, with the project's keys as built-in keys and the owner's as enrolled ones. Then it checks the image's length and SHA-256 against the manifest, and that it starts with the bank's validity marker. Uploads and fetched assets go through the same call, and an install verifies the release again.
- **Keys** are settings: `releases.project_keys` and `releases.owner_keys`. Nothing is built into the server. A server with neither keeps no release.
- **The cache** is `releases/` in the data directory: `<id>.plota` and its record `<id>.json`, the id being the manifest's SHA-256 in hex. Each file is written to a `.part`, synced and renamed. The directory is the index.
- **Fetching** is off unless `releases.fetch`. A check reads the newest 30 releases from the GitHub API and refuses an asset larger than a manifest and a full bank without downloading it.

## Installs

An owner installs a cached release on one feeder with `POST /feeders/{id}/install`. Nothing installs without that request.

The driver is `wellfed_connect::update::push`, which the CLI also runs. On the server, every step goes out on the feeder's long lane, so feeds and status reads keep working. When the session drops, for example as the feeder restarts into its new bank, the driver waits for the link and reads the status from the new session. Chunks are idempotent by offset, so a dropped session resumes where the feeder says. An offer is never sent again behind the owner's back.

One install runs at a time per server. The stream carries `install` updates: the start, progress every 32 KiB, and the end. An install a restart cut short reads as `interrupted`.

## Stock backup and restore

The server keeps each feeder's own stock firmware, the copy promotion would otherwise destroy, and offers "Restore stock" from it. See [rollback](../firmware/consumer-installation.md#rollback).

- **The copy** comes from the feeder (`POST /feeders/{id}/stock/read` reads bank 1 over the device API while stock is still there) or from an owner's upload of the copy the installer kept.
- **It's checked** against stock 1.1.15's pinned payload and container (`422 stock_refused` otherwise), then kept under `stock/<identity>/`. A kept copy is never replaced, and every read checks the file again.
- **Promotion waits for it.** An install on a feeder idle on bank 0 is `409 stock_backup_needed` until a verified copy is kept.
- **Restore** is signed by the owner on their computer, since the server holds no signing key: `wellfed server stock restore --version <n>`. The server then runs it as an install.

## Backups, export and import

**Backups** are `VACUUM INTO` copies in `backups/`, taken on a read connection so ingest goes on. They are named by the second they were taken (`server-20261027T033000Z.db`) and leave out the sealing key. [Running the server](running.md#back-it-up) covers restoring one.

### Export and import

- **The archive** (`wellfed_formats::export`, format version 2) is a POSIX tar: `manifest.json` first, in canonical JSON with each table's row count, size and SHA-256, then a file per table. `GET /export` writes `<table>.jsonl`. `?format=csv` writes `<table>.csv` for a spreadsheet. A quiet server exports the same bytes each time.
- **No secrets.** Accounts go without their password hashes and feeders without the server's sealed key. Sessions and tokens aren't exported.
- **Streaming.** The export reads one snapshot twice: once to count and hash each table, then to write it. Memory stays flat however large the tables. One export runs at a time (`409 export_running`).
- **`POST /import`** takes a JSON archive onto a fresh server, in one transaction. Like `/setup`, it needs no login and works only while no account exists (`409 import_closed`). Anything wrong rolls it all back (`422 archive_invalid`).
- **After an import,** every account has no password, so `wellfed-server reset-owner` gives an owner a temporary one. Every feeder is archived until it is adopted again, which continues its history under its id.

## Traces, statistics and the debug log

- **Traces and statistics.** A collector beside each feeder task reads on the long lane. After each `Fed` event and at each connect it fetches the trace of every recent feed that has none stored, among the 16 highest-numbered. It reads every statistics section the session allows at each connect and then hourly, and the food section after each `Fed` event. Reads change nothing on the feeder, so asking again is always safe.
- **What is kept.** A trace's summary and its body in the feeder's own encoding. The latest read of each statistics section. Every read of the food section, as a `food_readings` row, which is the only way the food beam reaches the server.
- **Routes.** `GET /feeders/{id}/feeds/{feed}/trace` and `GET /feeders/{id}/stats`, for every role.
- **The debug log.** `GET /feeders/{id}/log` is Server-Sent Events of the feeder's log lines, for owners only. The first follower opens a second session to the feeder, and it closes within a second of the last follower leaving. Nothing of it is stored.

## Tests

Tests follow the layers:

| Layer | Tested against |
| --- | --- |
| Store | Real SQLite, fresh per test |
| Service | `wellfed-feeder-sim` over TCP, with a fixed clock |
| HTTP API | The server in process, through `wellfed-server-client` |
| End to end | The server against the host simulator, speaking real Noise over TCP |
| Release | The shipped binary or image as a separate process ([release check](running.md#the-release-check)) |

Every path that can request a feed has a test that retries it and proves one feed. `openapi.json` and the `.sqlx/` metadata are generated, committed and checked for staleness in `make check`, so a reviewer sees every contract change as a diff.

The service tests fix the wall clock but run timers in real time with short intervals. tokio's paused clock would jump while sqlx's SQLite worker threads are busy, and fire timeouts that never happen.

## Evidence

- **Simulation**, October 24–27, 2026: every behavior on this page is covered by the server's tests against `wellfed-feeder-sim`. The stock backup and restore ran against the simulator's banks and a synthetic stock.
- **Static analysis**, October 27, 2026: the crate rules match `src/tools/xtask/src/layers.rs`, and the permission table matches `Permission::can` in `src/server/wellfed-service/src/accounts/permissions.rs`.
