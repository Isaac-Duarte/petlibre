# API design

The contracts between the feeder, the owner's server, the Android app and the web UI, and the crates that implement them. This is a design (Oct 23, 2026), and the device protocol is now built: FW45 (Oct 24) added `wellfed-proto`'s setup messages, `wellfed-client`'s setup session and the owner's `wellfed setup` over BLE ([Bluetooth](../firmware/bluetooth.md#setup-exchange)). FW47 (Oct 24) added the device API itself, which the feeder serves ([device API](../firmware/device-api.md)), with the `wellfed` CLI's commands on it. The server's first slice (FP18, Oct 24) is built too; [what it covers](#server-first-slice) lists where it stops. The product decisions it serves are in [product architecture](../firmware/product-architecture.md).

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
| Device versioning | Protocol major version and capability set in the handshake; clients adapt to whatever capabilities a feeder claims, indefinitely ([rules](../firmware/device-api.md#evolution)) | Lockstep: every firmware update would force app and server updates the same day. "N and N−1": owners can hold an update forever |
| Server API style | REST JSON under `/api/v1`, Server-Sent Events for live updates, OpenAPI generated from code | WebSocket: needs custom reconnect and auth, and server-to-client is all we need. GraphQL: per-field authorization and caching. gRPC/Connect: browsers need a proxy, and no curl |
| API stability | Unstable until the first release, then additive-only within `/api/v1` | Stable now would freeze early mistakes |
| Server auth | Household users (argon2 passwords, cookie sessions) plus a revocable token per phone | One admin password: no per-person audit. Proxy or OIDC only: too hard for non-technical owners; can come later on top |
| Tenancy | One household per server | Multiple households would scope every query and authorization check |
| Database | SQLite through sqlx, with compile-time-checked queries and embedded migrations | rusqlite: queries unchecked, and sync in async code. Postgres: a database for self-hosters to run. Both: every query written twice |
| Samples | Raw forever by default; the owner can set a raw retention and keep hourly rollups beyond it | — |
| Crates | Layered, so the compiler enforces the boundaries | One server crate: layering by review only, and the HTTP types couldn't be shared with the app |
| Store testing | A concrete `Store`, tested against a fresh SQLite per test | Repository traits with mocks: boilerplate, and mocks drift from real SQL |
| Time | `jiff` for zoned times, DST and schedules | chrono and chrono-tz make DST-ambiguous and skipped times easy to get wrong |
| App → server | `wellfed-api` types reach Dart through flutter_rust_bridge; Dart makes the HTTP calls | An OpenAPI-generated Dart client is a second source of types. HTTP inside the Rust core breaks sans-IO |

## Crates

```text
src/sdk/wellfed-session      no_std, no alloc. Noise, HMAC, the setup code, length-prefixed frames, hex and base64
src/product/wellfed-proto    no_std, no alloc. Device-protocol messages, framing, versions and capabilities
src/product/wellfed-device-api no_std, no alloc. The feeder's side: the sans-IO session server, pairing, the setup exchange
src/client/wellfed-model     owned device API values (status, settings, events, requests, answers) and their conversion from the wire
src/client/wellfed-client    sans-IO. Noise sessions, pairing, invites, the Connection: keepalive, answers, event cursor, recovery
src/client/wellfed-connect   tokio loop over wellfed-client's Connection: locate, open, ask, follow, join, recover
src/tools/wellfed-feeder-sim the host-simulated feeder: wellfed-feeder's loop pass and stores over real TCP
src/server/wellfed-api       HTTP request and response types, error codes, OpenAPI schemas. No axum
src/server/wellfed-store     sqlx, migrations, typed queries. The only crate that contains SQL
src/server/wellfed-service   domain logic: feeder connections, sync, users, releases. No axum, no SQL
src/server/wellfed-server    the binary: config, axum routers, auth middleware, SSE, the embedded web UI
src/app/                      Flutter app, plus a thin flutter_rust_bridge crate over wellfed-client, wellfed-model and wellfed-api
```

Allowed dependencies (each arrow means "may depend on"):

```text
wellfed-server ─► wellfed-service ─► wellfed-store ─► wellfed-api* ─► wellfed-model ─► wellfed-proto, wellfed-session (hex)
       │                   │
       │                   └─► wellfed-client ─► wellfed-model, wellfed-proto, wellfed-session
       └─► wellfed-api
* the store uses only the api crate's ID and enum types
```

- **`wellfed-session`** is the session layer the feeder and every client run: the Noise handshakes and transport, the setup code's proof and the frames that carry them, with the strict hex and base64 text forms. It knows nothing of the feeder's hardware or storage, so the server's crates and the app take it without the firmware's SDK (`wellfed-core`).
- **`wellfed-proto`** is used by the firmware (`wellfed-device-api`, `wellfed-feeder` and the feeder firmware; no SDK crate) and by every host. It holds only data types, framing and version rules: no IO, no crypto.
- **`wellfed-model`** is what a host means by the device API's values: owned, with no lifetimes or wire buffers, so they cross into Dart through flutter_rust_bridge and into the server's store and HTTP API unchanged. `wellfed-proto`'s borrowed, positional types stay the wire; `wellfed_model::convert` is the one place the two meet (and the way back, for an op sent or a body the server stores in the feeder's own encoding). `wellfed-api` serves the model's status, settings and event bodies as they are, adding only what the server knows (when it received them, its own ids), so their JSON and OpenAPI schemas are the model's derives.
- **`wellfed-client`** holds no sockets, timers or radios. Its callers feed it bytes and the current time and send what it returns. Its `Connection` (`handle_input`, `poll_transmit`, `poll_timeout`/`handle_timeout`, `request`, `subscribe`, `poll_event`) takes a model `Op` and yields model `Answer`s and pushed `Event`s, `Gap`s and `Sample`s (as does the lower-level `Session` under it: nothing borrowed from a frame crosses the crate's surface), and owns every session rule: a `Status` keepalive after 45 s quiet, under the feeder's 90 s idle limit; the end of a session heard from not at all for 90 s; one answer or `Unanswered` per request; and the event cursor with its epoch. `Recovery` runs the recovery window. The server and the CLI drive it with tokio through `wellfed-connect`, the app with Dart IO, and the bench with blocking sockets over the lower-level `Session`.
- **`wellfed-connect`** is a thin tokio loop over that `Connection`: it moves bytes, keeps the clock and queues the events and samples that arrive while a request waits for its answer. It takes its randomness from the caller (request ids and ephemeral keys are arguments), so the server can inject its generator and tests can fix it.
- **`wellfed-feeder-sim`** runs `wellfed-feeder`'s own API server, panel and pairing policy, and its stores on a flash image laid out like the feeder's storage region, so service, end-to-end and web tests meet the feeder's behavior rather than a mock of it. Its standard input stands in for the panel's buttons; a feed is refused until the feeding owner lands.
- **`wellfed-api`** builds for every target, including the app's Android library, so it carries no server dependencies.
- **`wellfed-service`** never sees an HTTP type, and **`wellfed-server`** never sees SQL. A handler parses, authorizes, calls one service method and maps the result.
- **`wellfed-stock`** holds stock 1.1.15's protocol, and its binary is the bench's server for stock ([tools roadmap](../development/tools-roadmap.md)). The catch-all bench server it came out of was deleted on October 26, 2026: the replacement's requests and offers go over the device API, through `wellfed-client` and the owner's `wellfed` CLI, and the broker stays only for stock. The installer's steps (FW42) and the rollback to stock (FW43) go to `wellfed-client` when those cards land.
- **Workspace members** gain `src/client/*` and `src/server/*`. Shared versions stay in `[workspace.dependencies]`.

## Device protocol

### Session

- **Transport.** TCP port 4747, advertised over mDNS as `_wellfed._tcp` with TXT `v=1` and `id=` the identity key in hex, so a client matches the feeder it paired with. Discovery carries no trust; the handshake does. Every frame is a 2-byte little-endian length and its bytes, at most 1 KiB (FW44). After the handshake a frame is one Noise transport message sealing one postcard-encoded message.
- **Opening.** A connection's first frame is a clear `Open`: `Session`, `Invite { id }` or `Recovery`. It only chooses the handshake; tampering with it makes the handshake fail.
- **Handshake.** An enrolled client's session is **Noise IK**. The client knows the feeder's key, and sends its own encrypted in the first message, so the feeder learns which client is connecting without trying each enrolled key (KK would need up to 8 DH operations per message 1, over FW44's 80 ms loop budget). It is one round trip, mutually authenticated, with no identity in clear. An invite is redeemed with **XXpsk3** and the invite's secret. The recovery window runs FW45's setup exchange (XX and the code proof) over the stream. Both sides run `wellfed-session::noise`, checked byte for byte against `snow`.
- **Hello.** IK's first payload is a `Hello` (`protocol_major`, capabilities); the second is a `Welcome` (major, capabilities, build identity, the client's role, the event history's epoch) or a typed refusal: unknown key, unsupported version, or no free slot. So the app can say "update the app" or "update the feeder". The event cursor travels in `Subscribe`, not the hello; the client sends it only when it counts in the welcome's epoch.
- **Feeder side.** The feeder runs two of its four tasks, the button service and the loop ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)), and every session runs in that one loop task over non-blocking sockets. `wellfed_device_api::Server` is sans-IO, like `wellfed-client`: bytes in, bytes out, no allocation. It has four slots with 1 KiB buffers each way. It does at most one handshake operation per pass (during a feed only IK session handshakes, so an enrolled client can open a session and cancel it; a stranger is refused unanswered and holds the rest until the feed ends), and reads at most one frame per slot per pass ([device API](../firmware/device-api.md)).
- **Keys.** A client's identity is its static X25519 key. The HMAC request keys of FW08 are gone; the session is the security.

### Messages

All messages are serde enums in `wellfed-proto`.

```text
Client → feeder                       Feeder → client
Request { id, op }                    Response { id, result }
Subscribe { after, samples }          Event { seq, at, body }
                                      Gap { from, to }   // events lost to the ring
                                      Sample { at_ms, metric }
                                      Log { seq, dropped, text }   // after TailLog
```

- **`id`** is a client-generated 128-bit request ID. The feeder remembers the last 32 write IDs per client key, across reconnects, with their compact outcomes, so a retried request returns the first answer rather than running again. Reads are not remembered; they simply run again. An ID that has left the ring is no longer recognized, which is safe for everything but feeds. A feed's ID also goes into its journal reservation (FW17), so even a retry after a reboot cannot feed twice.
- **Ops (major version 1, FW47):**
  - `Status`
  - `GetSettings` and `PutSettings { base, changes }`
  - `PutWifi { base, wifi }` (`None` clears)
  - `SetLock`, `Play`, `TimeSync`
  - `CreateInvite { role }`, `ListClients` and `RevokeClient { key }`
  - `Feed { portions }`, answered `Feeding { feed }` once the feeding owner has reserved it in its journal; the result is a `Fed` event (FW16)
  - `Reboot`
  - `CancelFeed { feed }`: `Done` while that feed runs, `NotFound` once it has ended

  The feed ops, the `Feeding` answer, the `Fed` event and the `Fault` refusal are behind `Capabilities::FEED`. A feeder that serves feeds says so in its welcome; it answers a client whose hello lacks the bit `Unavailable`, and never pushes it a `Fed` event. `Fed` carries the feed's number, the portions asked for and confirmed, an outcome (`Done`, `Cancelled`, `Expired`, `Failed`, or `Uncertain` when the owner stopped without a durable result, or a reset interrupted the feed and the next boot resolved it) and a reason. The request id makes a retry return the same `Feeding` answer instead of feeding twice, from the moment the feed is queued, on any session of the same client, within one boot ([device API](../firmware/device-api.md)). So the server's feed route never re-asks an id across a feeder restart: a retry under the same idempotency key first compares the feeder's boot count with the one the feed was sent under, and a feed sent before a restart is uncertain, resolved from the history, never sent again.

  Behind `Capabilities::READINESS`, `Status` is answered `FeederStatus { status, readiness }`: `Ready`, or `Blocked` with why (`Starting`, `Journal`, `Fault`, `Resolving`, `Feeding`, `Joining`, `Updating`, `Setup`, `Unserved`). A `Feed` obeys the same decision, and a change of a lasting reason is a `FeedingBlocked { reason }` event.

  A `Put` against a stale generation is refused `Stale { current }`. The time settings and the clock's state are behind `Capabilities::TIME`: `GetTime`, answered `Time(TimeView)` (the zone's rule, the NTP server, the clock with its error bound or `None` while the feeder needs the time, the learned rate, the last source and the last NTP result), and `PutTime { base, changes }`, an owner's only ([clock](../firmware/clock.md)). A `TimeSync` from an owner may step the clock; anyone else's may only narrow it, and is refused `Time` when it disagrees with a valid clock beyond both error bounds. Plans (FP03) join behind a capability bit, as updates did (`Capabilities::UPDATE`: `OfferUpdate`, `UpdateChunk`, `UpdateState`, `CancelUpdate`, `EnrollUpdateKey`, `UpdateKeyStatement`; [OTA design](../firmware/ota-design.md#updates-over-the-device-api)). Statistics are behind `Capabilities::STATS`: `Stats { section }`, answered `Stats`, and four more metrics ([device API](../firmware/device-api.md#statistics)).
- **Debug log** (`Capabilities::LOG`, owners only): `TailLog { after }` asks for the record lines the feeder logged after line `after` of this boot (`None`: the oldest its RAM tail keeps), then each new one, as `Log` batches of whole lines that fit one frame. `dropped` counts lines the tail overwrote before they were sent. The text is diagnostics, not a contract ([device API](../firmware/device-api.md#debug-log)).
- **Subscribe** asks for every event after a cursor (`None`: the oldest kept), then each new one, and the live samples if asked.
- **Results** are typed enums of outcomes and refusals. There is no free-text error.
- **Events** are the durable records described in [data](../firmware/product-architecture.md#data). `seq` is monotonic across reboots within the ring's epoch; a ring re-created by a storage wipe draws a new epoch and counts from 1 again.
- **Samples** go only to subscribers and are never stored on the feeder. `Power` (every supply sample, 5 s) and `Rssi` (30 s) are major version 1's; `Heap`, `Loop`, `Gateway` and `Wifi` (30 s) go only to a subscriber whose hello claims `STATS`. The feeder holds one of each kind per session, so one not sent before the next of its kind is replaced by it.

### Evolution

The rules, and the tests that enforce them, are in the [device API](../firmware/device-api.md#evolution). In short: the prologue never changes and a new major is negotiated in the hello; every addition is a new variant behind a capability bit both sides claim; no existing message gains a field, which the pinned byte fixtures check; the history carries an epoch; and clients adapt to a feeder's capabilities for as long as it runs, since an owner may never update it.

## Server HTTP API

### Shape

- **Resources** live under `/api/v1`. They use JSON bodies, `snake_case` fields (serde's default for Rust field names, so no rename attributes; the generated TS and Dart types use the same names), UUIDv7 IDs and RFC 3339 timestamps with offsets.
- **Errors** use RFC 9457 `application/problem+json`, with a typed `code` from a closed enum in `wellfed-api` and no catch-all string.
- **Writes** carry an `Idempotency-Key` header, which the server forwards as the device request ID. Edits carry `If-Match` with the generation, and a stale one gets `412` with the current resource.
- **Live updates.** `GET /api/v1/stream` is Server-Sent Events for every feeder the user can see: events, samples (opt in per metric), connection state and command results. It resumes with `Last-Event-ID`, mapped to the server's own event sequence; when that ID is too old to resume, the server sends a `resync` event and the client refetches. Every event payload is a typed schema in `openapi.json` (a `StreamEvent` enum).
- **OpenAPI** is generated from the handlers (utoipa) into a committed `openapi.json`. A test fails when it is stale, as with `bench.gen.ts`. The web UI's types and client come from that file through `openapi-typescript` and `openapi-fetch` ([web UI](../web/README.md)), so `wellfed-api` carries no ts-rs derives. Dart types come through flutter_rust_bridge, and a test checks the app's hand-written endpoint list against the spec.

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
- **Queries** use `sqlx::query!` and `query_as!`, with the offline metadata (`src/server/wellfed-store/.sqlx/`) committed, so builds need no database. `make sqlx-prepare` regenerates it from a scratch database the `sqlite3` command line builds from the migrations. SQL lives only in `wellfed-store`.
- **Time** is stored as integer Unix milliseconds (UTC), with zones as IANA names. `jiff` converts at the store boundary.
- **Tables (first cut):**
  - `household` (one row: name, units, time format), `users`, `sessions`, `tokens`
  - `feeders` (identity, key material, cursor) and `feeder_state` (cached plans, settings and status, with generation)
  - `events` (raw, keyed by feeder, epoch and `seq`, so the cursor sync is idempotent and a re-created ring's history stays apart)
  - `samples` (feeder, metric, time and value) and `sample_rollups` (hourly)
  - `pets` and `foods`
  - `releases`
  - `audit` (who did what through the server)
- **Volume.** About 100k sample rows a day per feeder is 1–2 GB a year, which SQLite handles. Retention runs as a background job under the owner's setting. Export streams rows, so it never loads the database into memory.
- **Secrets.** The server's feeder keys are stored encrypted with a key from the config file, so a copied database alone cannot impersonate the server.

## Server runtime

- **One task per feeder.** It owns the TCP connection and the `wellfed-client` session, reconnects with jittered backoff, and syncs events from the stored cursor before anything else. It applies events and samples through the store writer and publishes them to an in-process broadcast that SSE subscribers read. Commands reach it through a bounded channel and return through a oneshot channel with a timeout. A timeout reports "unknown, retry with the same key", never "failed", because the feeder may have run the command.
- **Discovery** browses mDNS (`mdns-sd`) for paired feeders whose address changed.
- **Shutdown and cancellation** go through one `CancellationToken` tree. Every task ends on it, and the store flushes before exit.
- **Determinism.** Time, randomness and the network sit behind small injected interfaces, so service tests run on a virtual clock against the host-simulated feeder.

## Server first slice

Built Oct 24 (host simulation only; no server has met a real feeder yet).

- **Resources:** `/health`, `/setup`, `/session` (GET for the logged-in user, POST to log in, DELETE to log out), `/feeders` (list, and adopt by invite), `/feeders/{id}` (live status, or the last copy marked `live: false`), `/feeders/{id}/settings` (GET with `ETag`, PATCH with `If-Match`), `/feeders/{id}/lock`, `/feeders/{id}/events` and `/stream`. Every other resource in [the list above](#first-resources) is still to come: household, users and tokens, feed, plans, samples, clients, food, intake, audit, releases, export and import.
- **Feeder connections** follow [server runtime](#server-runtime): a task per feeder subscribes after the newest stored event before anything else (from the feeder's oldest event when its epoch is a new one; `/feeders/{id}/events` reads the current epoch, or an older one with `?epoch=`), then sets the clock (again every 6 h), and records every event, gap and sample raw. A feeder that drops the server's key shows as `refused`. mDNS finds a feeder whose stored address fails, unless `--no-discovery`.
- **Writes** take `Idempotency-Key` as a UUID. The feeder remembers writes by request ID alone and every user shares the server's key, so the device request ID is a hash of the user, the feeder, the kind of write and the key: one key reused for two kinds of write, or by two users, never returns another write's answer. A write without a key gets a fresh ID. A settings change re-reads the settings afterwards, so its answer carries the new generation; if that read fails, the answer is `outcome_unknown`, since the change was made.
- **A refusal is final for its key:** `409 feeder_refused` with the feeder's reason, `busy` included. A refused write is final for its key: the feeder replays the refusal while it remembers the request ID (its per-client ring keeps the last 32 writes), so a retry with the same key is refused again; trying later is a new request with a new key. Nothing ran, so a refused feed is a new intent the user confirms again, unlike an `outcome_unknown` one, which only the same key may retry.
- **A command** is refused as offline until the connect sequence finishes, and then waits only behind an earlier command; one whose caller has stopped waiting is dropped unsent. A feeder heard from not at all for 90 s (time for a keepalive and its answer, with margin) counts as gone, so a feeder that loses power shows offline within that time rather than when the kernel gives up on the socket. The real feeder must answer a `Status` within that time even during flash writes; that is not yet checked on the bench.
- **Settings are read from the feeder**, never served from the cache, because a change must name the current generation; an offline feeder's settings are `503 feeder_offline`.
- **The stream's** SSE `id` is `<epoch>-<seq>`, with the epoch fixed at each start, so an id from before a restart resyncs; a `resync` carries an empty `id`. Samples are opt in with `?samples=power,rssi`. A stream rechecks its session every minute and ends once it is gone.
- **Hosts.** Every request must name the server by an IP address, `localhost` or a name given with `--host`, so a DNS-rebinding page (whose own name appears in both `Host` and `Origin`) is refused with `421 unknown_host`.
- **Samples** are always subscribed and kept: the simulator's rate (power every 5 s, signal every 30 s) is about 20,000 rows a day per feeder. Retention comes with the samples endpoint.
- **The store** has one writer connection and a read pool, rather than a writer task. Before pending migrations run, `VACUUM INTO` copies the database beside itself as `server.db.before-<version>-<unix ms>`.
- **Secrets:** the sealing key is `config.json` in the data directory, made on the first start (mode 0600). Each feeder key is sealed with ChaCha20-Poly1305, bound to the feeder's row ID and identity. Adoptions run one at a time, so two invites for one feeder cannot both enroll a key; a key the feeder enrolled but the server failed to store is logged with its fingerprint, to revoke.
- **Logins** hash at most two passwords at once (argon2id takes about 19 MiB each), and an unknown name costs the same hash as a wrong password.
- **Shutdown** on Ctrl-C or SIGTERM ends the streams, stops every feeder task (cancelling a connect sequence or command in flight) and closes the database.
- **Tests** follow the layers: the store against real SQLite, the service and the end-to-end test against `wellfed-feeder-sim` over TCP (through its `harness` module), and the HTTP API in process. The service tests fix the wall clock but run timers in real time with short intervals: tokio's paused clock would jump while sqlx's SQLite worker threads are busy, and fire timeouts that never happen.
- **Not yet:** phone tokens, `Secure` cookies behind an HTTPS proxy, login rate limiting, audit, and command results on the stream.

## Common crates

Pinned in `[workspace.dependencies]`; a new dependency needs a reason in its pull request.

| Purpose | Crate |
| --- | --- |
| HTTP | `axum` (already), `tower-http` (trace, request ID, timeout, body limit, compression), `axum-extra` (cookies, typed headers) |
| Async | `tokio` (already), `tokio-util` (`CancellationToken`), `futures-util` (already) |
| Data | `serde` and `serde_json` (already), `postcard` (device), `uuid` v7 (already) |
| Database | `sqlx` with SQLite, migrations and macros |
| Time | `jiff` |
| Crypto | a `no_std` Noise implementation (chosen in FW45), `argon2`, `chacha20poly1305` (the Noise cipher, reused for keys at rest), `sha2`, `getrandom` |
| OpenAPI and types | `utoipa`, `utoipa-axum`, `flutter_rust_bridge`; `ts-rs` (already) stays for the bench's views only |
| Discovery | `mdns-sd` |
| Releases | `reqwest` with rustls (`reqwest` already in `wellfed-stock`), `ed25519-dalek` (already) |
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

- Answered: the Noise implementation is our own `no_std` one, checked against `snow` (FW44, FW45); frames are 1 KiB, four clients, 32 remembered write IDs per client (FW44); one handshake operation per loop pass and one frame per client per pass (FW47).
- Whether the web UI's live view needs per-metric sample throttling.
- Backups: a scheduled SQLite backup inside the server, or left to the host.
