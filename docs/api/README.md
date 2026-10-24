# API design

The contracts between the feeder, the owner's server, the Android app and the web UI, and the crates that implement them. This is a design (Oct 23, 2026); nothing here is built yet. The product decisions it serves are in [product architecture](../firmware/product-architecture.md).

There are three contracts, and each changes at a different cost:

| Contract | Between | Cost of a change |
| --- | --- | --- |
| [Device protocol](#device-protocol) | Feeder ↔ app, server, CLI | Highest: feeders in homes update last |
| [Server HTTP API](#server-http-api) | Server ↔ web UI, app, third parties | Medium: the server and its clients ship together, but integrators exist |
| [Storage](#storage) | Server ↔ its database | Lowest: migrations are local, but owner data must never be lost |

## Decisions

| Area | Decision | Rejected, and why |
| --- | --- | --- |
| Device encoding | postcard with serde, inside Noise | CBOR: larger, with its own derives. Protobuf: `.proto` codegen and weaker `no_std` support. JSON: largest, and costly to parse on the MCU |
| Device versioning | Protocol major version and capability set in the handshake; the feeder accepts N and N−1 for one release | Lockstep: every firmware update would force app and server updates the same day |
| Server API style | REST JSON under `/api/v1`, Server-Sent Events for live updates, OpenAPI generated from code | WebSocket: needs custom reconnect and auth, and server-to-client is all we need. GraphQL: per-field authorization and caching. gRPC/Connect: browsers need a proxy, and no curl |
| API stability | Unstable until the first release, then additive-only within `/api/v1` | Stable now would freeze early mistakes |
| Server auth | Household users (argon2 passwords, cookie sessions) plus a revocable token per phone | One admin password: no per-person audit. Proxy or OIDC only: too hard for non-technical owners; can come later on top |
| Tenancy | One household per server | Multiple households would scope every query and authorization check |
| Database | SQLite through sqlx, with compile-time-checked queries and embedded migrations | rusqlite: queries unchecked, and sync in async code. Postgres: a database for self-hosters to run. Both: every query written twice |
| Samples | Raw forever by default; the owner can set a raw retention and keep hourly rollups beyond it | — |
| Crates | Layered, so the compiler enforces the boundaries | One server crate: layering by review only, and the HTTP types couldn't be shared with the app |
| Store testing | A concrete `Store`, tested against a fresh SQLite per test | Repository traits with mocks: boilerplate, and mocks drift from real SQL |
| Time | `jiff` for zoned times, DST and schedules | chrono and chrono-tz make DST-ambiguous and skipped times easy to get wrong |
| App → server | `petlibre-api` types reach Dart through flutter_rust_bridge; Dart makes the HTTP calls | An OpenAPI-generated Dart client is a second source of types. HTTP inside the Rust core breaks sans-IO |

## Crates

```text
src/sdk/petlibre-proto        no_std, no alloc. Device-protocol messages, framing, versions and capabilities
src/client/petlibre-client    sans-IO. Noise sessions, pairing, invites, request tracking, event-cursor sync
src/server/petlibre-api       HTTP request and response types, error codes, OpenAPI schemas. No axum
src/server/petlibre-store     sqlx, migrations, typed queries. The only crate that contains SQL
src/server/petlibre-service   domain logic: feeder connections, sync, users, releases. No axum, no SQL
src/server/petlibre-server    the binary: config, axum routers, auth middleware, SSE, the embedded web UI
src/app/                      Flutter app, plus a thin flutter_rust_bridge crate over petlibre-client and petlibre-api
```

Allowed dependencies (each arrow means "may depend on"):

```text
petlibre-server ─► petlibre-service ─► petlibre-store ─► petlibre-api*
       │                   │
       │                   └─► petlibre-client ─► petlibre-proto
       └─► petlibre-api
* the store uses only the api crate's ID and enum types
```

- **`petlibre-proto`** is used by the firmware (`petlibre-sdk`, `petlibre-feeder`) and by every host. It holds only data types, framing and version rules: no IO, no crypto.
- **`petlibre-client`** holds no sockets, timers or radios. Its callers feed it bytes and the current time and send what it returns. The server drives it with tokio, the app with Dart IO, the CLI and bench with tokio, and the host simulator directly in tests.
- **`petlibre-api`** builds for every target, including the app's Android library, so it carries no server dependencies.
- **`petlibre-service`** never sees an HTTP type, and **`petlibre-server`** never sees SQL. A handler parses, authorizes, calls one service method and maps the result.
- **`petlibre-local`** is retired in stages ([tools roadmap](../development/tools-roadmap.md)): its stock install path moves to a `petlibre-stock` library and `petlibre-client`'s installer steps, its replacement path to `petlibre-client` and the owner's `petlibre` CLI, and its MQTT replacement transport is removed once FW47 lands.
- **Workspace members** gain `src/client/*` and `src/server/*`. Shared versions stay in `[workspace.dependencies]`.

## Device protocol

### Session

- **Transport.** TCP to the port advertised over mDNS (`_petlibre._tcp`), with Noise inside. Each Noise transport message carries one postcard-encoded message, behind a 2-byte length prefix. Frames are capped at a size FW44 sets.
- **Handshake.** After pairing, both sides know each other's static key, so sessions use **Noise KK**: one round trip, mutual authentication, and no identity sent in clear. Pairing itself runs over the BLE setup channel, or over the LAN during an invite or button-hold window, using a pattern that exchanges static keys (XX with a pairing secret as PSK). FW45 confirms the patterns and the `no_std` Noise implementation; one implementation on both sides is preferred, so interop never depends on two.
- **Hello.** The first messages carry `protocol_major`, a capability set, the build identity, and the client's last event cursor. A major the feeder doesn't accept ends the session with a typed reason, so the app can say "update the app" or "update the feeder".
- **Feeder side.** The firmware has two tasks and the button service holds one ([task slots](../../src/firmware/petlibre/README.md#what-the-sdk-runtime-guarantees)), so every session runs in one loop task over non-blocking sockets, with the same sans-IO shape as `petlibre-client`: bytes in, bytes out, no allocation. Per-client buffers and the client limit are fixed at build time (FW44). A handshake is one loop step, and the feeding owner is stepped in the same loop ([device API](../firmware/product-architecture.md#device-api)).
- **Keys.** A client's identity is its static X25519 key. The HMAC request keys of FW08 become the session's transport security, and the per-request signature goes away.

### Messages

All messages are serde enums in `petlibre-proto`.

```text
Client → feeder                       Feeder → client
Request { id, op }                    Response { id, result }
Ack { cursor }                        Event { seq, at, body }
Subscribe { samples }                 Sample { at, metric, value }
                                      Gap { from, to }   // events lost to the ring
```

- **`id`** is a client-generated 128-bit request ID. The feeder keeps a fixed ring of recent IDs per client (FW44 sizes it), so a retried request returns the first answer rather than running again. An ID that has left the ring is no longer recognized, which is safe for everything but feeds. A feed's ID also goes into its journal reservation (FW17), so even a retry after a reboot cannot feed twice.
- **Ops (first set):**
  - `Feed { portions }`
  - `Status`
  - `GetPlans` and `PutPlans { base_generation, plans }`
  - `GetSettings` and `PutSettings { base_generation, changes }`
  - `ReadEvents { after, max }`
  - `TimeSync`
  - `CreateInvite`, `ListClients` and `RevokeClient`
  - `OtaOffer`
  - `Reboot`

  A `Put` against a stale generation is refused with the current generation.
- **Results** are typed enums of outcomes and refusals. There is no free-text error.
- **Events** are the durable records described in [data](../firmware/product-architecture.md#data). `seq` is monotonic across reboots.
- **Samples** go only to subscribers and are never stored on the feeder.

### Evolution

- **Within a major version,** append new enum variants and new ops at the end, and gate them behind a capability. Never reorder, remove or retype a field or variant, because postcard is positional and not self-describing.
- **A new field** on an existing message is a new message version, behind a capability.
- **Fixtures.** `petlibre-proto` keeps byte fixtures of every message for each released major version. A test decodes them with the current code, so a breaking change fails CI instead of failing in a home.
- **A major bump** is rare. The feeder accepts N and N−1 for one release, and the server and app accept both until every paired feeder reports N.

## Server HTTP API

### Shape

- **Resources** live under `/api/v1`. They use JSON bodies, `snake_case` fields (serde's default for Rust field names, so no rename attributes; the generated TS and Dart types use the same names), UUIDv7 IDs and RFC 3339 timestamps with offsets.
- **Errors** use RFC 9457 `application/problem+json`, with a typed `code` from a closed enum in `petlibre-api` and no catch-all string.
- **Writes** carry an `Idempotency-Key` header, which the server forwards as the device request ID. Edits carry `If-Match` with the generation, and a stale one gets `412` with the current resource.
- **Live updates.** `GET /api/v1/stream` is Server-Sent Events for every feeder the user can see: events, samples (opt in per metric), connection state and command results. It resumes with `Last-Event-ID`, mapped to the server's own event sequence; when that ID is too old to resume, the server sends a `resync` event and the client refetches. Every event payload is a typed schema in `openapi.json` (a `StreamEvent` enum).
- **OpenAPI** is generated from the handlers (utoipa) into a committed `openapi.json`. A test fails when it is stale, as with `bench.gen.ts`. The web UI's types and client come from that file through `openapi-typescript` and `openapi-fetch` ([web UI](../web/README.md)), so `petlibre-api` carries no ts-rs derives. Dart types come through flutter_rust_bridge, and a test checks the app's hand-written endpoint list against the spec.

### First resources

```text
GET    /api/v1/setup                        whether a fresh server awaits its first owner
POST   /api/v1/setup                        create the first owner (refused once any account exists)
POST   /api/v1/session                      log in (cookie)   DELETE: log out
GET    /api/v1/household                    name, units (g or oz), time format   PATCH (owner)
GET    /api/v1/users      POST, PATCH, DELETE /users/{id}
GET    /api/v1/tokens     POST (an app token, shown once), DELETE /tokens/{id}
GET    /api/v1/feeders                       paired feeders with connection state
POST   /api/v1/feeders                       adopt a feeder by redeeming an invite
GET    /api/v1/feeders/{id}                  status, firmware, clock, power
POST   /api/v1/feeders/{id}/feed             Idempotency-Key required
GET    /api/v1/feeders/{id}/plans            PUT with If-Match
GET    /api/v1/feeders/{id}/settings         PATCH with If-Match
GET    /api/v1/feeders/{id}/events           ?after=&limit=&kind=
GET    /api/v1/feeders/{id}/samples          ?metric=&from=&to=&resolution=raw|hour
GET    /api/v1/feeders/{id}/clients          clients the feeder knows; DELETE revokes
GET    /api/v1/feeders/{id}/food             food remaining and refill estimate (derived)
GET    /api/v1/pets/{id}/intake              grams and calories against target  ?from=&to=
GET    /api/v1/audit                         who did what through the server  ?after=&limit=
GET    /api/v1/releases                      fetched and verified releases
POST   /api/v1/feeders/{id}/install          owner-approved install of a release
GET    /api/v1/pets    /foods                CRUD, owner data the feeder doesn't hold
GET    /api/v1/export                        everything, as JSON or a CSV archive
POST   /api/v1/import
GET    /api/v1/stream                        SSE
GET    /health                               no auth; liveness and version
```

### Auth

- **Users** log in with argon2-hashed passwords. Browsers get a session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` when a trusted proxy reports HTTPS). Every cookie-authenticated write must carry a JSON body and an `Origin` matching the server, and `Sec-Fetch-Site: same-origin` when the browser sends it; browsers send Fetch Metadata only over HTTPS, and most owners use plain HTTP on the LAN.
- **The first owner** is created by the first visitor to a fresh server through `/setup`, which closes once any account exists.
- **Phones** get a long-lived bearer token, stored hashed and revocable. Each token names its user, so the audit log names the person.
- **Roles** are owner and member: members feed and edit schedules, and owners also manage users, tokens, feeders and installs. This is a closed enum, checked in one middleware layer plus per-handler extractors.
- **The server has its own feeder key** per feeder, enrolled by invite like any client, so the feeder's audit shows "server (on behalf of user X)".

## Storage

- **SQLite in WAL mode.** One writer connection and a small read pool. The writer is a single task that owns all writes, so writes never contend.
- **Migrations** are sqlx's, embedded in the binary and applied at startup after a backup copy of the database file. They are append-only: a released migration is never edited.
- **Queries** use `sqlx::query_as!`, with the offline metadata (`.sqlx/`) committed, so builds need no database. SQL lives only in `petlibre-store`.
- **Time** is stored as integer Unix milliseconds (UTC), with zones as IANA names. `jiff` converts at the store boundary.
- **Tables (first cut):**
  - `household` (one row: name, units, time format), `users`, `sessions`, `tokens`
  - `feeders` (identity, key material, cursor) and `feeder_state` (cached plans, settings and status, with generation)
  - `events` (raw, keyed by feeder and `seq`, with a unique constraint so the cursor sync is idempotent)
  - `samples` (feeder, metric, time and value) and `sample_rollups` (hourly)
  - `pets` and `foods`
  - `releases`
  - `audit` (who did what through the server)
- **Volume.** About 100k sample rows a day per feeder is 1–2 GB a year, which SQLite handles. Retention runs as a background job under the owner's setting. Export streams rows, so it never loads the database into memory.
- **Secrets.** The server's feeder keys are stored encrypted with a key from the config file, so a copied database alone cannot impersonate the server.

## Server runtime

- **One task per feeder.** It owns the TCP connection and the `petlibre-client` session, reconnects with jittered backoff, and syncs events from the stored cursor before anything else. It applies events and samples through the store writer and publishes them to an in-process broadcast that SSE subscribers read. Commands reach it through a bounded channel and return through a oneshot channel with a timeout. A timeout reports "unknown, retry with the same key", never "failed", because the feeder may have run the command.
- **Discovery** browses mDNS (`mdns-sd`) for paired feeders whose address changed.
- **Shutdown and cancellation** go through one `CancellationToken` tree. Every task ends on it, and the store flushes before exit.
- **Determinism.** Time, randomness and the network sit behind small injected interfaces, so service tests run on a virtual clock against the host-simulated feeder.

## Common crates

Pinned in `[workspace.dependencies]`; a new dependency needs a reason in its pull request.

| Purpose | Crate |
| --- | --- |
| HTTP | `axum` (already), `tower-http` (trace, request ID, timeout, body limit, compression), `axum-extra` (cookies, typed headers) |
| Async | `tokio` (already), `tokio-util` (`CancellationToken`), `futures-util` (already) |
| Data | `serde` and `serde_json` (already), `postcard` (device), `uuid` v7 (already) |
| Database | `sqlx` with SQLite, migrations and macros |
| Time | `jiff` |
| Crypto | a `no_std` Noise implementation (chosen in FW45), `argon2`, `rand_core` |
| OpenAPI and types | `utoipa`, `utoipa-axum`, `flutter_rust_bridge`; `ts-rs` (already) stays for the bench's views only |
| Discovery | `mdns-sd` |
| Releases | `reqwest` with rustls (already in `petlibre-local`), `ed25519-dalek` (already) |
| Observability | `tracing` and `tracing-subscriber` (already) |
| Tests | `insta` (snapshots), `proptest` (codec round trips), `tower::ServiceExt` (in-process requests), `sqlx::test` |
| Hygiene | `cargo-deny` (licenses, advisories, duplicates) and `cargo-nextest`, both in place ([Rust standard](../development/rust.md)) |

## Quality standard

The repository's [Rust conventions](../../AGENTS.md#rust-conventions) apply in full. On top of them:

- **Boundaries are crate boundaries.** If a change wants SQL in a handler or axum in the service, the design is wrong. Move the code; don't add the dependency.
- **One error enum per crate** (`thiserror`). The server maps service errors to `problem+json` in one `IntoResponse`. No `anyhow` below the binary.
- **Typed everything:** IDs are newtypes, roles and metrics are enums, and generations are their own type. Never a raw `String` or `i64` for one of these across a crate boundary.
- **Committed contracts.** `openapi.json`, the TS types, the Dart bindings, the `.sqlx/` metadata and the protocol fixtures are generated, committed and checked for staleness in `make check`. A reviewer sees every contract change as a diff.
- **Tests by layer:**
  - proto: round trips and fixtures
  - client: scripted byte exchanges
  - store: real SQLite
  - service: a simulated feeder and a virtual clock
  - server: in-process HTTP
  - end to end: the server against the host simulator speaking real Noise over TCP
- **Feeding safety in tests.** Every path that can request a feed has a test that retries it, and the test proves one feed.

## Open questions

- Which `no_std` Noise implementation, and its RAM cost (FW44, FW45).
- The frame size cap, the client limit and the request-ID ring size (FW44), all bounded by one loop task's fixed buffers.
- The KK and XX handshake time on the KM4, and the per-client work cap per loop pass, which together set the loop's worst-case latency (FW44).
- Whether the web UI's live view needs per-metric sample throttling.
- Backups: a scheduled SQLite backup inside the server, or left to the host.
