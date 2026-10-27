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
| Crates | Layered, so the compiler enforces the boundaries: a crate per runtime or dependency boundary, a module per domain inside it, and the [allowed dependencies](#allowed-dependencies) checked from `cargo metadata` | One server crate: layering by review only, and the HTTP types couldn't be shared with the app. A crate per domain: a dozen crates whose boundaries keep moving as the domains do |
| Service types | `wellfed-service` returns `wellfed-api` bodies until a second API version exists | Service-owned types mapped in the server: a second copy of every body, for one API version |
| Migrations | Squashed freely until the first server release, append-only from then on | Append-only now: a dozen migrations that only ever ran on test databases |
| App path | Server first when one is configured, direct to the feeder as a fallback, and an uncertain write never switches path ([two paths](#two-paths-one-feed)) | Server only: no feeding while the server is down. Direct only: no remote access and no per-person audit |
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
                              wellfed-model and wellfed-api
```

The server's crates follow one rule: **a crate per runtime or dependency boundary, a module per domain inside it.** A new domain (pets, alerts) is a module and some tables, not a crate, unless it brings its own runtime (tasks), a heavy dependency (`reqwest`) or a consumer outside the server. That is why the mirror (the feeder tasks), the releases (`reqwest` and the verifier) and the HTTP client (the CLI) are crates and accounts or alerts are not.

The map is the target ([server board](../../SERVER_BOARD.md#target-architecture)). Today the server is `wellfed-api`, `wellfed-store`, `wellfed-mirror`, `wellfed-releases`, `wellfed-service` and `wellfed-server`, with `wellfed-server-client` and `wellfed-server-testkit` beside them.

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

`cargo xtask check-repository` enforces the arrows from `cargo metadata` (all features, every platform), and each error names the edge or path that breaks them:

- **SQL stays in the store.** No crate but `wellfed-store` depends on sqlx, in its tests included.
- **HTTP stays in the binary.** Nothing below `wellfed-server` (the api, store, mirror, releases, service and the server client) links axum, or any of axum's own crates (`axum-core`, `axum-extra`), by any path.
- **The arrows as drawn.** Each server crate's normal and build dependencies on workspace crates come from its row below; a crate outside the server takes a server crate only where drawn: the CLI takes `wellfed-server-client`, and the app's `wellfed-app-bridge` takes `wellfed-api`. Dev-dependencies are exempt, since a test may build a simulated feeder or a whole server. The rows name crates that do not exist yet, so each is checked from its first manifest; changing a row changes this table and the check together.
- **`wellfed-api` on Android.** Its own dependencies come from an allow-list (`jiff`, `wellfed-model`, `serde`, `utoipa`, `uuid`), so it keeps building for the app's Android library. `make check` builds no Android target.
- **Files stay small.** A Rust source under `src/server/` has at most 400 lines, or 600 under a `tests/` directory (an integration test and its modules; a `tests.rs` beside the code it tests keeps 400), so a growing domain gets a module of its own. The error names the file and its count. Generated files (`openapi.json`, the sqlx metadata) are not Rust and are not counted.

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

- **`wellfed-session`** is the session layer the feeder and every client run: the Noise handshakes and transport, the setup code's proof and the frames that carry them, with the strict hex and base64 text forms. It knows nothing of the feeder's hardware or storage, so the server's crates and the app take it without the firmware's SDK (`wellfed-core`).
- **`wellfed-proto`** is used by the firmware (`wellfed-device-api`, `wellfed-feeder` and the feeder firmware; no SDK crate) and by every host. It holds only data types, framing and version rules: no IO, no crypto.
- **`wellfed-model`** is what a host means by the device API's values: owned, with no lifetimes or wire buffers, so they cross into Dart through flutter_rust_bridge and into the server's store and HTTP API unchanged. `wellfed-proto`'s borrowed, positional types stay the wire; `wellfed_model::convert` is the one place the two meet (and the way back, for an op sent or a body the server stores in the feeder's own encoding). `wellfed-api` serves the model's status, settings and event bodies as they are, adding only what the server knows (when it received them, its own ids), so their JSON and OpenAPI schemas are the model's derives.
- **`wellfed-client`** holds no sockets, timers or radios. Its callers feed it bytes and the current time and send what it returns. Its `Connection` (`handle_input`, `poll_transmit`, `poll_timeout`/`handle_timeout`, `request`, `subscribe`, `poll_event`) takes a model `Op` and yields model `Answer`s and pushed `Event`s, `Gap`s and `Sample`s (as does the lower-level `Session` under it: nothing borrowed from a frame crosses the crate's surface), and owns every session rule: a `Status` keepalive after 45 s quiet, under the feeder's 90 s idle limit; the end of a session heard from not at all for 90 s; one answer or `Unanswered` per request; and the event cursor with its epoch. `Recovery` runs the recovery window. The server and the CLI drive it with tokio through `wellfed-connect`, the app with Dart IO, and the bench with blocking sockets over the lower-level `Session`.
- **`wellfed-connect`** is a thin tokio loop over that `Connection`: it moves bytes, keeps the clock and queues the events and samples that arrive while a request waits for its answer. It takes its randomness from the caller (request ids and ephemeral keys are arguments), so the server can inject its generator and tests can fix it.
- **`wellfed-feeder-sim`** runs `wellfed-feeder`'s own API server, panel and pairing policy, and its stores on a flash image laid out like the feeder's storage region, so service, end-to-end and web tests meet the feeder's behavior rather than a mock of it. Its standard input stands in for the panel's buttons. It has no motor, but a feeding owner on the feeder's own feed journal (`feeding`): a feed is queued under the journal's next id, which is its number, reserved at the owner's next step (only then is `Feeding` answered), "driven" for a fixed time per portion, and ends in a `Fed` event; a restart in the middle resolves it at the next boot as uncertain under its own number. A test can stall a feed in flight (`Control::StallFeeds`), and the harness's relay can hold back only the feeder's answers or cut every session, dropping what it held.
- **`wellfed-api`** builds for every target, including the app's Android library, so it carries no server dependencies. It has a module per resource.
- **`wellfed-store`** stays one crate, so the sqlx offline metadata and the migrations stay in one place, with a module per table group (`users`, `feeders`, `history`, `projections`, `samples`, `owner`, `commands`, `releases`, `jobs`). It keeps a single writer connection beside a read pool ([storage](#storage)).
- **`wellfed-mirror`** owns every task that holds a feeder session: connect (at the stored address, else over mDNS), sync from the cursor, clock sync, ingest, reconnect with jittered backoff, and the command channel. Its surface is small: `Mirror::start` and `stop` a feeder (given its id, identity, unsealed key and address; both return once the old task has ended, its last update `offline`), `link`, `ask` (one answer, or `AskError::Unknown` when the answer does not come in time; a command whose caller left is dropped unsent), `operation` (a long-operation turn, one per feeder, whose steps go out one at a time on a lane of their own: the task pipelines a short command beside a step in flight and holds the next step while a short command is in flight or waiting, so a short command waits at most one step; each lane has one request in flight, matched to its answer by id, and a cut session leaves both unknown), `locate` and `enroll` for adoption, a per-feeder broadcast of typed `MirrorEvent`s (events, gaps, samples, link changes): an event or gap after the store holds it, a sample as it arrives, and `follow_log` (the debug log). Its collector reads each feed's trace and the statistics on the long lane and stores them ([traces, statistics and the debug log](#traces-statistics-and-the-debug-log)). Ingest is its own: it calls the store to record each raw row, its projections and the cursor in one transaction, and samples in small batches (32, or `Timing::sample_wait` after the first, and at the session's end), each with its number, and `stored_event` is the one way back from a stored body; `refresh_projections` hands the store that decoder to rebuild the projections when their version changes, before any feeder starts, and `refresh_samples` does the same for the samples' numbers with `stored_sample`. It also holds the injected `Clock` and `Random` every server crate uses. The service never sees a socket, and the mirror never sees a user, an alert or a stream id.
- **`wellfed-releases`** fetches releases from the project's GitHub releases or takes an owner's upload, and verifies both the same way, with the verifier the feeder's own checks are tested against, never a copy: `wellfed_client::release::verify` runs `wellfed_core::ota::offer::admit_offer` (host-safe, so no crate was split out) and then checks the image against the manifest. See [releases](#releases).
- **`wellfed-service`** is one crate with a module per domain: accounts, household, commands and the journal, owner data, derived views, alerts, audit and export. Each domain has its own error enum, wrapped in the crate's `Error`. It owns the auth rules (argon2, issuing and verifying sessions and tokens, expiry, the login-limit state, and a `Permission` enum with one `can(role, permission)` table), the hub, and each job's body as a method that takes `now`. Derived views are a pure module over the store's projections, with "now" and the household zone passed in, so they are tested without a database. It returns `wellfed-api` bodies until a second API version exists. It never sees an HTTP type.
- **The hub** is the service's: it merges the mirror's per-feeder events with command results, alerts and install progress, and assigns the stream ids `GET /stream` resumes from. A task per feeder forwards its broadcast, recording the feeder's link before the hub hears of it, so the feeder list never shows a state the stream has not, and drops the entry when the mirror lets the feeder go; a forwarder that falls behind drops its backlog, publishes `resync` and then the current link.
- **`wellfed-server`** never sees SQL. A handler parses, authorizes through an axum extractor that calls the service's auth, calls one service method and maps the result. The jobs supervisor lives here, scheduling each job's service method on the cancellation tree with the injected `Clock`.
- **`wellfed-server-client`** lives in `src/client/` beside the device API's clients: the CLI's `wellfed server` commands and the server's own tests go through it, so neither can disagree with the routes. A typed method accepts only its route's success status; a refusal comes back as the server's `Problem`, a stale edit as the current settings, and nothing retries, so an uncertain write is the caller's to resend under the same key. It authenticates with the session cookie a login sets or a bearer token, sends the server's own `Origin` (it is not a page another site could drive), and follows `GET /stream` with its SSE ids. Its TLS is reqwest's rustls with its aws-lc provider. A request the typed methods cannot make (a malformed one, in a test) goes through `Client::request`, with the same base, origin and credentials.
- **`wellfed-server-testkit`** is a dev-dependency of the server's crates: the service on a temporary store with a fixed `Clock` and a seeded `Random` (each start its own seed, so a reopened service sends the feeder no request id its first run sent), the simulated feeder and the owner's invites, and the HTTP server in process on a localhost port, reached through `wellfed-server-client`. Service and API tests are a file per domain over it.
- **The export archive's** manifest and row types live in `wellfed-formats` (`wellfed_formats::export`), since export writes the archive and import reads it; the manifest's hashes go through `wellfed_formats::canonical`. Each row type is its table's columns under their database names, so the store reads and inserts them with checked queries itself, and the service only frames them. See [export and import](#export-and-import).
- **`wellfed-stock`** holds stock 1.1.15's protocol, and its binary is the bench's server for stock ([tools roadmap](../development/tools-roadmap.md)). The catch-all bench server it came out of was deleted on October 26, 2026: the replacement's requests and offers go over the device API, through `wellfed-client` and the owner's `wellfed` CLI, and the broker stays only for stock. The installer's steps (FW42) and the rollback to stock (FW43) go to `wellfed-client` when those cards land.
- **Workspace members** gain `src/client/*` and `src/server/*`. Every server dependency's version is pinned in `[workspace.dependencies]`, and a crate's manifest only picks features.

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

  The feed ops, the `Feeding` answer, the `Fed` event and the `Fault` refusal are behind `Capabilities::FEED`. A feeder that serves feeds says so in its welcome; it answers a client whose hello lacks the bit `Unavailable`, and never pushes it a `Fed` event. `Fed` carries the feed's number, the portions asked for and confirmed, an outcome (`Done`, `Cancelled`, `Expired`, `Failed`, or `Uncertain` when the owner stopped without a durable result, or a reset interrupted the feed and the next boot resolved it) and a reason. The request id makes a retry return the same `Feeding` answer instead of feeding twice, from the moment the feed is queued, on any session of the same client, within one boot ([device API](../firmware/device-api.md)). So the server's feed route never re-asks a feed at all: a retry under the same idempotency key is answered from the journal, and a feed whose answer was lost is settled from the history ([feeds](#server-first-slice)), never sent again.

  Behind `Capabilities::READINESS`, `Status` is answered `FeederStatus { status, readiness }`: `Ready`, or `Blocked` with why (`Starting`, `Journal`, `Fault`, `Resolving`, `Feeding`, `Joining`, `Updating`, `Setup`, `Unserved`, and behind `Capabilities::POWER` `Battery`, which an older session reads as `Starting`). A `Feed` obeys the same decision, and a change of a lasting reason is a `FeedingBlocked { reason }` event.

  A `Put` against a stale generation is refused `Stale { current }`. The time settings and the clock's state are behind `Capabilities::TIME`: `GetTime`, answered `Time(TimeView)` (the zone's rule, the NTP server, the clock with its error bound or `None` while the feeder needs the time, the learned rate, the last source and the last NTP result), and `PutTime { base, changes }`, an owner's only ([clock](../firmware/clock.md)). A `TimeSync` from an owner may step the clock; anyone else's may only narrow it, and is refused `Time` when it disagrees with a valid clock beyond both error bounds. Plans (FP03) join behind a capability bit, as updates did (`Capabilities::UPDATE`: `OfferUpdate`, `UpdateChunk`, `UpdateState`, `CancelUpdate`, `EnrollUpdateKey`, `UpdateKeyStatement`; [OTA design](../firmware/ota-design.md#updates-over-the-device-api)). Statistics are behind `Capabilities::STATS`: `Stats { section }`, answered `Stats`, and four more metrics ([device API](../firmware/device-api.md#statistics)). Per-feed traces are behind `Capabilities::TRACES` and `STATS` together: `FeedTrace { feed, offset }`, answered `FeedTrace(TracePage)` a page of the feed's trace body at a time (the format is `wellfed_proto::trace`), and the statistics' `Feeds` section ([device API](../firmware/device-api.md#feed-traces)). The stock backup is behind `Capabilities::BACKUP`: `ReadBank1 { offset }`, owners only and served only by the bank-0 recovery role, answered `Bank1(Bank1Page)` a page of bank 1 at a time ([device API](../firmware/device-api.md#stock-backup)).
- **Debug log** (`Capabilities::LOG`, owners only): `TailLog { after }` asks for the record lines the feeder logged after line `after` of this boot (`None`: the oldest its RAM tail keeps), then each new one, as `Log` batches of whole lines that fit one frame. `dropped` counts lines the tail overwrote before they were sent. The text is diagnostics, not a contract ([device API](../firmware/device-api.md#debug-log)).
- **Subscribe** asks for every event after a cursor (`None`: the oldest kept), then each new one, and the live samples if asked.
- **Results** are typed enums of outcomes and refusals. There is no free-text error.
- **Events** are the durable records described in [data](../firmware/product-architecture.md#data). `seq` is monotonic across reboots within the ring's epoch; a ring re-created by a storage wipe draws a new epoch and counts from 1 again.
- **Samples** go only to subscribers and are never stored on the feeder. `Power` (every supply sample, 5 s) and `Rssi` (30 s) are major version 1's; `Heap`, `Loop`, `Gateway` and `Wifi` (30 s) go only to a subscriber whose hello claims `STATS`, and `Supply` (the supply judged, with every power sample) to one that claims `POWER`, which also gates the `Supply` event ([power](../firmware/power.md)). The feeder holds one of each kind per session, so one not sent before the next of its kind is replaced by it.

### Evolution

The rules, and the tests that enforce them, are in the [device API](../firmware/device-api.md#evolution). In short: the prologue never changes and a new major is negotiated in the hello; every addition is a new variant behind a capability bit both sides claim; no existing message gains a field, which the pinned byte fixtures check; the history carries an epoch; and clients adapt to a feeder's capabilities for as long as it runs, since an owner may never update it.

## Server HTTP API

### Shape

- **Resources** live under `/api/v1`. They use JSON bodies, `snake_case` fields (serde's default for Rust field names, so no rename attributes; the generated TS and Dart types use the same names), UUIDv7 IDs and RFC 3339 timestamps with offsets.
- **Errors** use RFC 9457 `application/problem+json`, with a typed `code` from a closed enum in `wellfed-api` and no catch-all string.
- **Writes** carry an `Idempotency-Key` header, which the server forwards as the device request ID. A route that requires one (a feed) answers a request without it `400 invalid`, as it does a key that is not a UUID. Edits carry `If-Match` with the generation, and a stale one gets `412` with the current resource.
- **Live updates.** `GET /api/v1/stream` is Server-Sent Events for every feeder the user can see: events, samples (opt in per metric), connection state and command results. It resumes with `Last-Event-ID`, mapped to the server's own event sequence; when that ID is too old to resume, the server sends a `resync` event and the client refetches. Every event payload is a typed schema in `openapi.json` (a `StreamEvent` enum).
- **OpenAPI** is generated from the handlers (utoipa) into a committed `openapi.json`. A test fails when it is stale, as with `bench.gen.ts`. The web UI's types and client come from that file through `openapi-typescript` and `openapi-fetch` ([web UI](../web/README.md)), so `wellfed-api` carries no ts-rs derives. Dart types come through flutter_rust_bridge, and a test checks the app's hand-written endpoint list against the spec.

### Two paths, one feed

Most owners will run a server, so the app talks to it whenever it is reachable: remote access works, and the audit log names the person. Each owner's or member's phone also keeps its own key on every feeder, and uses it on the LAN when the server is down, for reads and new intents only. A viewer's phone, and one whose account expires, holds no key, so the server alone enforces its limits.

The danger is a retry that crosses paths: the server and the phone send different device request ids, so a feed sent directly that timed out, then retried through the server, could feed twice. So **an uncertain write never switches path.** It is retried on its own path with its own key, or resolved from the feeder's history (a feed by its `Fed` event), never resent on the other. The CLI follows the same rule: a command takes one path, the device API or `wellfed server`, never a fallback between them.

This costs the server an operation for everything the app does, and the feeder a client slot per phone (8 slots: the server, the CLI and up to six phones).

The server enforces its half. A write whose answer was lost is answered `504 outcome_unknown` and kept unknown under its `Idempotency-Key`: a retry under that key reads the journal and sends nothing new for a feed. A feed is settled from its `Fed` event (by its number, or the one unclaimed `Fed` event after it), readable at `GET /commands/{key}` and reported on the stream as a `command` event, so a client resolves a feed it lost on either path without resending it. The app's half (holding an uncertain write to its path) is the app's.

Every device operation the app makes has a server route, or a stated reason it has none, in `wellfed_service::parity`: an exhaustive match over `Op`, so a new operation does not build until it is given one, and a test checks each route is in `openapi.json`. Routes that need the feeder's owner role (Wi-Fi, reboot, revoking a client, a phone's invite, the zone's rule, installs) need the server adopted with an owner's invite; with the `server` role the feeder refuses them `forbidden` ([open question](../../SERVER_SPEC.md#open-questions)).

### First resources

```text
GET    /api/v1/setup                        whether a fresh server awaits its first owner
POST   /api/v1/setup                        the first owner and the household's zone (refused once any account exists)
POST   /api/v1/session                      log in (cookie)   DELETE: log out
GET    /api/v1/household                    name, units (grams or ounces), time format, IANA zone   PATCH (owner)
GET    /api/v1/users                         owners only   POST, PATCH, DELETE /users/{id}
GET    /api/v1/tokens                        one's own (an owner: everyone's)   POST (from a login; the secret shown once), DELETE /tokens/{id}
GET    /api/v1/feeders                       paired feeders with connection state
POST   /api/v1/feeders                       adopt a feeder by redeeming an invite
GET    /api/v1/feeders/{id}                  status, firmware, clock, power
POST   /api/v1/feeders/{id}/feed             Idempotency-Key required
GET    /api/v1/feeders/{id}/plans            PUT with If-Match
GET    /api/v1/feeders/{id}/settings         PATCH with If-Match
GET    /api/v1/feeders/{id}/events           ?after=&limit=&kind=
GET    /api/v1/feeders/{id}/samples          ?metric=&from=&to=&resolution=raw|hour
GET    /api/v1/feeders/{id}/clients          clients the feeder knows, and whose each is   DELETE /clients/{key} revokes, POST /clients/{key}/keep
POST   /api/v1/feeders/{id}/invites          an invite for the caller's own phone (a token of an owner or member with no expiry)
GET    /api/v1/phones                        the phones' keys the server invited, and any revoke still owed
PUT    /api/v1/feeders/{id}/wifi             If-Match with the status's wifi_generation (owners)
POST   /api/v1/feeders/{id}/reboot           never resent when unknown (owners)   POST /play: a tune
DELETE /api/v1/feeders/{id}                  remove: revoke the server's key, archive; adopting again restores it
GET    /api/v1/feeders/{id}/food             food remaining and refill estimate (derived)
GET    /api/v1/pets/{id}/intake              grams and calories against target  ?from=&to=
GET    /api/v1/commands                      the command journal  ?feeder=&state=&after=&limit=
GET    /api/v1/commands/{key}                one write, by its Idempotency-Key
GET    /api/v1/audit                         who did what through the server  ?after=&limit=
GET    /api/v1/releases                      fetched and verified releases  ?after=&limit=
POST   /api/v1/releases                      upload a release file (octet-stream)
POST   /api/v1/releases/check                check GitHub releases now
POST   /api/v1/feeders/{id}/install          owner-approved install of a release   GET: the running or last one
GET    /api/v1/feeders/{id}/stock            the kept stock backup, the feeder's banks, the last restore
GET    /api/v1/feeders/{id}/stock/backup     export the kept backup (octet-stream)
POST   /api/v1/feeders/{id}/stock/read       read bank 1 from the feeder and keep it   /stock/upload: an owner's copy
POST   /api/v1/feeders/{id}/stock/restore    "Restore stock", signed by the owner
GET    /api/v1/pets    /foods                CRUD, owner data the feeder doesn't hold
GET    /api/v1/pets/{id}/weights             a pet's weight log  POST, DELETE /weights/{weight}
PUT    /api/v1/pets/{id}/feeders/{feeder}    a feeder serves the pet   DELETE: no longer
PUT    /api/v1/feeders/{id}/food             the feeder's food from now on   GET /food/history
GET    /api/v1/feeders/{id}/refills          POST records one (grams, or a full bag)
GET    /api/v1/export                        everything, as a JSON or (?format=csv) CSV archive (owner)
POST   /api/v1/import                        a JSON archive onto a fresh server (no login; no account yet)
GET    /api/v1/stream                        SSE
GET    /api/v1/status                        readiness (owner): the store, each feeder's link, the jobs' last runs
GET    /health                               no auth; liveness and version
```

### Auth

- **Users** log in with argon2-hashed passwords. Browsers get a session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` when a trusted proxy reports HTTPS). Every cookie-authenticated write must carry a JSON body and an `Origin` matching the server, and `Sec-Fetch-Site: same-origin` when the browser sends it; browsers send Fetch Metadata only over HTTPS, and most owners use plain HTTP on the LAN.
- **The first owner** is created by the first visitor to a fresh server through `/setup`, which closes once any account exists.
- **Phones** and the CLI get a bearer token (`POST /tokens`, from a login only): `plt_` and 64 hex digits, 32 random bytes shown once and stored as their SHA-256 (high entropy needs no slow hash, and a copied database holds no live token), named for its device, with its last use (to the minute) and an optional expiry. It lasts until revoked unless given one. It carries its user, not a role: each use reads the user, so a role change applies at once, deleting the user deletes their tokens, and an account past its expiry has its tokens refused `403 account_disabled` (kept, and working again once an owner extends it). A request with `Authorization` is judged by it alone, never by a cookie beside it, so a token-authenticated write skips the `Origin` check (no other site's page can send the header without a CORS preflight the server never grants). Each user lists and revokes their own; an owner lists and revokes anyone's, and another user's token is `404` to a member. A password change leaves the account's tokens. Only a login makes a token, sets any password (`PATCH /session`, or `PATCH /users/{id}` with one) or adds an account; from a token each is `403 session_required`, so a leaked token cannot mint others, nor a password or an owner account to log in with. The journal records the token a write came through.
- **Roles** are owner, member and viewer, a closed enum. The service's `Permission` enum and its one `can(role, permission)` table hold the whole matrix ([spec](../../SERVER_SPEC.md#permissions)): a viewer reads status and history and writes nothing, a member also feeds and changes settings and the lock, and an owner does everything, including users, the household, adopting feeders and installs. Each route names the permission it needs as a type (`Allowed<need::Control>`), an axum extractor that asks `can`; a refusal is `403 forbidden`. Every account sees every feeder.
- **Sessions** last 30 days from their last use (the cookie is kept 400 days; the server ends the session). An account may carry an expiry, for a pet sitter; past it the account is disabled, not deleted: its login and sessions are refused `403 account_disabled` until an owner extends or deletes it. Owners never expire, and the last owner can be neither demoted nor deleted (`409 last_owner`). Deleting an account ends its sessions.
- **Failed logins** are limited to 5 a minute per name and 20 a minute per address, in token buckets the service keeps on its injected clock: an attempt spends from both before its password is checked (so concurrent guesses cannot all slip past one check), and a login that succeeds gives it back. A limited login is `429 login_limited` with `Retry-After` in whole seconds, the right password included. No account is ever locked, and sessions and tokens already open carry on. The buckets live in one map of at most 10,000, so a flood of names or addresses costs bounded memory. A bucket is dropped only once it is full again; while every bucket still holds state, an attempt that needs a new one is refused `429` until the first refills (at most a minute), so a flood can delay others' logins but never pushes out the bucket of the name under attack; a name counts trimmed and lowercased, and an IPv6 address by its /64. A failed login with an account's name is recorded in that account's audit entries with the client's address; one with no account's name names nobody and is not recorded, so failures add at most five rows a minute per account.
- **Passwords** have at least 10 characters and no other rule. An owner adds an account with a temporary password and can set another for anyone, which ends that user's sessions. Any user changes their own with `PATCH /session` and the current password, which ends their other sessions and keeps the one asking. A sole owner locked out runs `wellfed-server --data DIR reset-owner` on the host, which prints a new temporary password.
- **The server has its own feeder key** per feeder, enrolled by invite like any client, so the feeder's audit shows "server (on behalf of user X)".
- **The audit log** (`GET /audit?after=&limit=`, `{ items, next }` oldest first) is the command journal interleaved with the account changes: setup, logins and failed logins (with the client's address), users created, changed (role or expiry), deleted, passwords set (one's own, an owner's reset, or the host's `reset-owner`, which names no user), tokens made and revoked, and household changes. Each entry names its user (and their name then, kept after the account is deleted), the token it came through, when, and what: a command entry carries the journal's entry as it stands now, its state and outcome included. Each user reads their own entries; an owner reads everyone's (`Permission::OwnHistory` and `AllHistory`). A journal row joins the log through a trigger in the statement that records it, so no write reaches a feeder unlogged, and a write never sent leaves with its row; an account change is recorded after it is made, and a change made but not recorded is logged. Owner data (pets, foods, refills) and adoptions are not in it yet. Kept forever, like the journal.

### The CLI on the server

`wellfed server …` speaks this API through `wellfed-server-client` alone (it reaches the bodies as `wellfed_server_client::api`), beside the device commands, and each command takes one path ([two paths, one feed](#two-paths-one-feed)).

- **Login.** `wellfed server login --server URL --name NAME` reads the password from standard input, opens a session, makes a token, keeps it in `private/wellfed-server.json` (`--token-file`) mode 0600 beside the device API's key file, and ends the session. `logout` revokes the token and deletes the file. Every other verb sends the token.
- **Verbs.** `feeders`, `status`, `settings`, `lock`, `feed` and `cancel-feed` (each takes `--feeder` with an id or a name when the server holds more than one), `events` (with `--follow` over the stream), `command <key>` and `commands` (the journal), `users` (list, a role or an expiry, delete; adding an account needs a login), `tokens` (list, revoke), `releases` (list, upload, check), `install` (a release, followed on the stream to its end; without one, the last install), `stock` (the kept backup, the feeder's banks and the last restore; `read`, `upload <file>`, `export --output <file>`, and `restore --version <n>`, which signs here, see [stock](#stock-backup-and-restore)), `clients` (whose each client is), `revoke` (a client by at least 8 hex digits of its key) and `reboot` (never sent again when its outcome is unknown). `export <file>` saves the archive (`--csv` for spreadsheets), written beside the file and renamed once whole.
- **Output.** Text by default; with `--json`, the API's bodies, one per line, a refusal's problem included. The exit status is 0 on success, 1 on an error and 3 when an outcome is unknown.
- **Uncertain writes.** A write goes out once under a fresh `Idempotency-Key`. When its outcome is unknown (`504 outcome_unknown`, a `500`, or an answer lost on the way) the command prints the key and `wellfed server command <key>`, exits 3, and sends nothing more: not again under a new key, and never over the device API. `command <key>` reads the entry the feeder's history settles, exiting 3 until it does. A feed waits on the stream for its result, opened before the feed is sent; an `uncertain` or `unrecorded` result exits 3 too.

## Storage

- **SQLite in WAL mode.** One writer connection beside a small read pool. Every write goes through that connection, so writes never contend; a household's one to five feeders need no writer task in front of it.
- **Migrations** are sqlx's, embedded in the binary and applied at startup after a backup copy of the database file. Until the first server release they are squashed freely, since no owner's database has run them; from then on they are append-only, and a released migration is never edited.
- **Queries** use `sqlx::query!` and `query_as!`, with the offline metadata (`src/server/wellfed-store/.sqlx/`) committed, so builds need no database. `make sqlx-prepare` regenerates it from a scratch database the `sqlite3` command line builds from the migrations. SQL lives only in `wellfed-store`.
- **Time** is stored as integer Unix milliseconds (UTC), with zones as IANA names. `jiff` converts at the store boundary.
- **Tables (first cut):**
  - `household` (one row: name, units, time format, the IANA zone), `users` (with an optional expiry), `sessions` (with their last use), `tokens`
  - `feeders` (identity, key material, cursor) and `feeder_state` (cached plans, settings and status, with generation)
  - `events` (raw, keyed by feeder, epoch and `seq`, so the cursor sync is idempotent and a re-created ring's history stays apart)
  - projections of the events, written in the raw insert's transaction and rebuilt from `events` when `projection_version` changes: `feeds` (one row per feed, its latest event), `lock_changes`, `boots`, `clock_syncs` and `blocked_changes`
  - `samples` (feeder, metric, time, the raw value and its number, decoded at ingest), `sample_rollups` (count, min, max and sum per feeder, metric and UTC hour) and `sample_marks` (the newest sample rolled up, and the number code's version)
  - beside the history, never rebuilt from it: `feed_traces` (each feed's trace, keyed as `feeds` is, or the feeder's refusal when it kept none), `feeder_stats` (the latest read of each statistics section) and `food_readings` (every read of the food section, typed)
  - `pets` and `foods`
  - `releases`: a directory in the data directory rather than a table ([releases](#releases))
  - `audit` (account changes as typed JSON, and a row per journal entry that the journal's own insert trigger adds)
- **Volume.** About 100k sample rows a day per feeder is 1–2 GB a year, which SQLite handles. Retention runs as a background job under the owner's setting. Export streams rows, so it never loads the database into memory.
- **Secrets.** The server's feeder keys are stored encrypted with a key from the config file, so a copied database alone cannot impersonate the server.

## Server runtime

- **One task per feeder.** It owns the TCP connection and the `wellfed-client` session, reconnects with jittered backoff, and syncs events from the stored cursor before anything else. It applies events and samples through the store's writer and publishes them on its feeder's broadcast; the service's hub merges those with command results and alerts, and SSE subscribers read the hub. Commands reach it through a bounded channel per lane (short commands, and a long operation's steps) and return through a oneshot channel with a timeout. A timeout reports "unknown, retry with the same key", never "failed", because the feeder may have run the command.
- **Discovery** browses mDNS (`mdns-sd`) for paired feeders whose address changed.
- **Shutdown and cancellation** go through one `CancellationToken` tree. Every task ends on it, and the store flushes before exit.
- **Determinism.** Time, randomness and the network sit behind small injected interfaces, so service tests run on a virtual clock against the host-simulated feeder. No server crate reads the wall clock outside `SystemClock`: `src/server/clippy.toml` refuses jiff's `Timestamp::now` and `Zoned::now`, std's `SystemTime::now`, `SystemTime::elapsed`, `Instant::now` and `Instant::elapsed`, and uuid's `Uuid::now_v7` and `Timestamp::now` (ids are UUIDv7 built from the `Clock` and `Random`) (timeouts use tokio's `Instant`, which is monotonic and follows a paused test runtime), and `SystemClock`'s `#[expect]` fails the build if that rule stops applying.

### Configuration

The server keeps everything in its data directory (`--data`, or `WELLFED_SERVER_DATA`):

| Path | What |
| --- | --- |
| `server.db` | The database. |
| `server.toml` | The settings: optional, every key has a default. |
| `keys/server.key` | The sealing key, its 32 bytes as they are (mode 0600, in a 0700 directory). It seals every feeder key in the database, so back it up apart from the database's backups, which leave it out. |
| `backups/` | The database's [backups](#backups) (mode 0700, each copy 0600), with a `README.txt` saying where the key is and how to restore. |
| `stock/` | Each feeder's [stock backup](#stock-backup-and-restore): `stock/<identity>/stock-1.1.15.bin`, the vendor's container, and its record (`.json`: SHA-256, size, source, when), never in the database. |

`server.toml` holds what is neither a secret nor a household setting (the household's zone is in the database). [`server.example.toml`](../../src/server/wellfed-server/server.example.toml) lists every key with its default, and a test parses it:

```toml
listen = "0.0.0.0:8080"          # default 127.0.0.1:8080, this computer only
hosts = ["feeder.lan"]           # names besides IP addresses and localhost
label = "wellfed-server"        # the label enrolled on each feeder, 1-16 bytes
discovery = "off"                # "mdns" (default), or adopt by address only
trusted_proxies = ["127.0.0.1"]  # proxies (addresses or CIDR ranges) whose forwarded headers are believed

[retention]
raw_sample_days = 90             # default: keep raw samples forever

[releases]
fetch = false                    # check GitHub releases daily
project_keys = []                # the project's update keys, hex
owner_keys = []                  # the owner's enrolled update keys, hex

[backups]
keep = 7                         # daily backups kept
```

- **Overrides.** Each key has a flag, and each flag an environment variable: `--listen` (`WELLFED_SERVER_LISTEN`), `--host` (`WELLFED_SERVER_HOSTS`, comma-separated), `--label`, `--discovery` (or `--no-discovery`), `--trusted-proxy` (`WELLFED_SERVER_TRUSTED_PROXIES`), `--raw-sample-days`, `--releases-fetch true|false`, `--release-project-key` and `--release-owner-key` (`WELLFED_SERVER_RELEASE_PROJECT_KEYS` and `..._OWNER_KEYS`, comma-separated) and `--backups-keep`, each variable `WELLFED_SERVER_` and the flag's name. A flag beats its variable, and either replaces the file's value whole: `--host` given once replaces the file's `hosts`. An override can replace a list but not clear it (no flag or empty variable means "not given"), so to drop every host or proxy, edit the file. `--no-discovery` beats `--discovery` and its variable.
- **A bad file stops the start.** An unknown key, a wrong type or an out-of-range value (a 17-byte label, `raw_sample_days = 0`, a host name with a port) is refused with the key's path (`retention.raw_sample_days`, `hosts[1]`) and its line; a missing file is the defaults.
- **The key.** The first start makes it, staging it beside its name and hard-linking it into place, so a second start racing the first is refused rather than replacing its key; an existing `keys` directory others could read or enter is tightened to 0700, with a warning. A server from before the settings file kept it in `config.json`: the first start moves it to `keys/server.key` and deletes `config.json`, which held nothing else. A start interrupted between the two finishes the move on the next; two different keys are refused. A `server.db` with no key file is refused too, since a new key could not open its feeder keys: that is a backup restored without its key.
- **Trusted proxies** are addresses or CIDR ranges, logged at the start; see [behind a reverse proxy](#behind-a-reverse-proxy).
- **Who reads them.** `releases` is read by [releases](#releases), `retention` by the [samples](#samples)' retention job and `backups.keep` by [backups](#backups).

### Behind a reverse proxy

The server speaks plain HTTP. To reach it from outside the home, put a reverse proxy in front that terminates HTTPS, and tell the server to believe that proxy:

```caddyfile
feeder.example.org {
    reverse_proxy 127.0.0.1:8080 {
        # A request id both logs share (the server keeps one plain id).
        header_up X-Request-ID {http.request.uuid}
    }
}
```

```toml
listen = "127.0.0.1:8080"         # only the proxy reaches the server
hosts = ["feeder.example.org"]    # the public name, besides IPs and localhost
trusted_proxies = ["127.0.0.1"]   # the proxy's address, or its range
```

- **What the proxy gives the server.** Caddy obtains and renews the certificate, preserves `Host` and sets `X-Forwarded-For` and `X-Forwarded-Proto`. From a peer in `trusted_proxies` the server reads the client's address from `X-Forwarded-For` (or `Forwarded` when that is absent), right to left: each proxy appends the address it heard from, so the first hop that is not itself a trusted proxy is the client, and anything a client wrote to the left is ignored. The scheme is `X-Forwarded-Proto`'s last value (or the last `Forwarded` element's `proto`), and `https` makes the session cookie `Secure`. From any other peer these headers are ignored: the address is the connection's and the scheme is plain HTTP. The header is chosen by presence: whenever `X-Forwarded-For` is sent at all, `Forwarded` is ignored, so the proxy must set or append to `X-Forwarded-For` (Caddy does, as does nginx with `$proxy_add_x_forwarded_for`); each comma-separated entry is read on its own, so a client's undecodable entry never hides the proxy's. The scheme is the one the last proxy reports: behind a chain, the proxy in front sets `X-Forwarded-Proto` from the client's connection and each one behind passes it on, since no hop's value is checked against the one before. The client's address drives the per-address login limit and is recorded with each login in the audit log.
- **List only the proxy.** Anyone in `trusted_proxies` can claim to be any client, so it names the proxy's own address (a container network's range, such as `172.16.0.0/12`, when the proxy runs in one) and never the LAN or the internet. A server reached both directly and through the proxy keeps its direct clients' real addresses, since they are not trusted.
- **The name.** The public name goes in `hosts`, since the server refuses names it was not given (`421 unknown_host`, which stops DNS rebinding). The `Origin` check compares the browser's `Origin` with `Host`, both the public name over HTTPS, so the pages work through the proxy unchanged.
- **Request ids.** A request's own `x-request-id` is kept when it is one plain id (at most 64 bytes of letters, digits, `.`, `_` and `-`), so a proxy's (Caddy's `{http.request.uuid}` above) appears in both logs; anything else gets a fresh UUID. A client behind the proxy can send its own plain id too, which the proxy passes along unless it replaces it as above.
- **What a member's phone needs.** The server's public URL (`https://feeder.example.org`), an account (an owner adds it with a temporary password; the member logs in and sets their own), and a token made from that login for the phone (`POST /tokens`, shown once), which the app sends as `Authorization: Bearer`. A token is judged alone, needs no cookie or `Origin`, and is revoked on its own if the phone is lost. A phone that should keep feeding with the server down also holds its own feeder key, which works only on the home network ([two paths](#two-paths-one-feed)).
- **Without a proxy.** On the LAN the server is reached directly (`listen = "0.0.0.0:8080"`), over plain HTTP, with no `Secure` cookie; leave `trusted_proxies` empty. A VPN into the home network is the other way in from outside, and needs no proxy.

### Packaging

Each GitHub release carries the server as one static binary per target, `wellfed-server-x86_64-unknown-linux-musl` and `wellfed-server-aarch64-unknown-linux-musl`, and the same binary in a container image. Both run the server as described above; what differs is where the data directory lives and who owns it.

- **The data directory.** Everything is in it: `server.db`, `server.toml`, `keys/server.key`, `releases/` and `backups/` ([configuration](#configuration)). The key file is linked into place with a hard link, so the directory must be on a filesystem that has them: any local one does, while some network and FUSE mounts do not, and the first start then fails with a write error. Keep it on local disk, or on a volume whose driver supports hard links.
- **Backups and the key.** The daily copies and those an owner asks for are in `backups/` inside the data directory, beside the database, and they leave out the key ([backups](#backups)). Back up the data directory as a whole, or at least `keys/server.key` once and `backups/` regularly, to somewhere off the machine: a restored `server.db` opens only with its key. Under the systemd unit the directory is `/var/lib/wellfed-server`; in the image it is the volume at `/data`.
- **Building the release.** `make server-dist` builds both binaries into `dist/server/`; attaching them to a GitHub release (and pushing the image to a registry) is the owner's step, which nothing in the repository does. `make server-image` builds the image (`CONTAINER=podman` for podman). Both run [`packaging/Containerfile`](../../src/server/wellfed-server/packaging/Containerfile): a `rust:1.99.1-alpine3.24` stage on the build machine's own platform links both targets with zig (`cargo zigbuild`), so the aarch64 binary and the arm64 image build on an x86_64 host with no emulation (`docker buildx build --platform linux/amd64,linux/arm64` for a multi-arch image). rustls with its aws-lc provider and the bundled SQLite both build for musl this way (checked October 27, 2026, both targets). Without a container, a host with zig and `cargo-zigbuild` runs the same `cargo zigbuild --release --locked -p wellfed-server --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl` after `rustup target add` for both; a plain `cargo build --target x86_64-unknown-linux-musl` also needs a musl C compiler (`musl-gcc`, or Alpine's own gcc) for aws-lc and SQLite.
- **The release check.** `wellfed-server`'s `tests/release.rs` starts the server as a separate process on a fresh temporary data directory, sets the household up, adopts the simulated feeder by address with `--no-discovery`, reads readiness and the feeder's live status, takes a backup, stops it with SIGTERM (a clean exit, as systemd and container runtimes stop it), and starts it again to reconnect by the stored address. `make check` runs it against the debug build. `make check-server-release` runs it against this host's binary in `dist/server/` (`WELLFED_RELEASE_SERVER=<binary>`) and then against the image (`WELLFED_RELEASE_IMAGE=<image>`, `WELLFED_RELEASE_RUNTIME=docker|podman`, an ignored test, so the gate never needs a container runtime). Nothing in either is left running.

#### systemd

[`packaging/wellfed-server.service`](../../src/server/wellfed-server/packaging/wellfed-server.service) is an example unit for the static binary:

```sh
sudo install -m 0755 wellfed-server-x86_64-unknown-linux-musl /usr/local/bin/wellfed-server
sudo install -m 0644 wellfed-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now wellfed-server
```

- **Its user.** `DynamicUser=yes` makes a user for the unit alone, which owns only `StateDirectory=wellfed-server` (`/var/lib/wellfed-server`, mode 0700, files 0600 through `UMask=0077`). The rest of the system is read-only or hidden (`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `PrivateDevices`, no capabilities, `@system-service` system calls only); `systemd-analyze security` rates it 1.3. Address families are limited to IPv4, IPv6, Unix and netlink, which mDNS needs to list the interfaces it browses on.
- **Settings.** Put `server.toml` in `/var/lib/wellfed-server/` as root (`sudo install -m 0644 server.toml /var/lib/wellfed-server/`; the server only reads it, and it holds no secret, so a root-owned file is fine; the next start reads it), or set `WELLFED_SERVER_*` variables with `systemctl edit wellfed-server`. It listens on `127.0.0.1:8080` until `listen` says otherwise.
- **Discovery works.** The unit shares the host's network, so mDNS finds feeders whose address changed.
- **`reset-owner`** runs as the unit's user, with the server stopped: `sudo systemctl stop wellfed-server`, then `sudo systemd-run --pty --wait -p DynamicUser=yes -p User=wellfed-server -p StateDirectory=wellfed-server -p UMask=0077 /usr/local/bin/wellfed-server --data /var/lib/wellfed-server reset-owner`, then start it again.

#### Container

The image (`FROM scratch`) holds the binary, the CA certificates its GitHub release checks need and an empty `/tmp`; the zone database is built into the binary. It runs as uid 65532, keeps its data in the volume at `/data` (`WELLFED_SERVER_DATA`) and listens on `0.0.0.0:8080` (`WELLFED_SERVER_LISTEN`); arguments after the image name are the server's flags.

- **Host networking, for discovery.** mDNS is multicast, which a container's bridge network does not pass, so run it on the host's network to keep discovery:

  ```sh
  docker run -d --name wellfed-server --restart unless-stopped \
      --network host -v wellfed-server:/data wellfed-server
  ```

  It then listens on the host's port 8080 directly (set `WELLFED_SERVER_LISTEN=127.0.0.1:8080` when a proxy on the host is the only way in).
- **A bridged network, by address.** Without host networking, turn discovery off and give every feeder's address when adopting it (`address` in `POST /feeders`: an IP address, the API's port by default, or `IP:port`; a DNS name is not resolved):

  ```sh
  docker run -d --name wellfed-server --restart unless-stopped \
      -p 8080:8080 -v wellfed-server:/data wellfed-server --no-discovery
  ```

  The server then adopts and reconnects at the stored address only, and adopting with no address is refused (`400 invalid`). A feeder whose address changes stays offline until it is back at that address, so give each feeder a fixed address (a DHCP reservation). The feeders must be reachable from the container: a bridge network routes out to the LAN, but not back in, which the feeder API never needs.
- **The volume.** A named volume (as above) or a bind mount of a local directory owned by uid 65532 (`--user` picks another uid; with rootless podman, `--userns=keep-id` maps yours). It must support hard links (above).
- **Behind a proxy in another container,** list that container network's range in `trusted_proxies` ([behind a reverse proxy](#behind-a-reverse-proxy)).

## Releases

Built Oct 26 (host simulation). `wellfed-releases` keeps the signed updates an owner may install; nothing installs from here (that is `POST /feeders/{id}/install`).

- **A release file** is the version 2 manifest (160 bytes) followed by the image it names (`image_len` bytes: the bank's contents, without the OTA container's header), which is exactly what `wellfed_client::update::Image` offers. On GitHub it is a release asset whose name ends in `.plota`; drafts and pre-releases are skipped.
- **Verification** is `wellfed_client::release::verify`: the feeder's own `admit_offer`, with the project's keys as built-in keys and the owner's as enrolled ones, no key retired, a floor of zero and nothing running; then the image's length and SHA-256 against the manifest, and that it starts with the bank's validity marker, as the feeder's sector writer requires. A stock image or a version-0 one always needs a statement bound to one feeder, so neither is a release. Uploads and fetched assets go through the same call, and reading a release back for an install verifies it again.
- **Keys** are settings: `releases.project_keys` (the key built into the feeders' firmware, or the offline release key their release slot holds) and `releases.owner_keys` (the owner's enrolled update keys). A server with neither keeps no release. Nothing is built into the server: today the firmware's built-in key (`WELLFED_OTA_PUBLIC_KEY`, which `wellfed-build` sets only for firmware builds) is the bench key from `private/`, and which key signs public releases is an [open question](../../SERVER_SPEC.md#open-questions). A release names its signer, `project` or `owner`, by today's keys; the list leaves out a release whose key is no longer listed, and an install's read refuses it.
- **The cache** is `releases/` in the data directory: `<id>.plota` and its record `<id>.json`, the id being the manifest's SHA-256 in hex (the name a feeder's update state gives it). Each file is written to a `.part`, synced and renamed, the release before its record; the next start removes a `.part` or a release with no record. There is no table: the directory is the index. A record that does not parse, or whose release file is gone, is logged and skipped; uploading the release again repairs it.
- **Fetching** is off unless `releases.fetch`. A check reads the newest 30 releases from the GitHub API over rustls, skips assets the cache holds or this process already refused, refuses an asset larger than a manifest and a full bank without downloading it, and stops reading one that grows past that. A release list GitHub does not serve ends the check (`502 releases_unreachable`); what it added stays. An asset that fails to download is reported under `failed` and tried again by the next check. One check runs at a time. The daily `release_check` job runs it at 04:00 household time while `Service::fetching_releases`.
- **Routes** are owners only (`Permission::ServerData`): `GET /releases` (newest version first, paged by release id), `POST /releases` with the file as `application/octet-stream` (`201` new, `200` already held, `413 too_large`, `422 release_refused` with `release` naming the reason: `size`, `manifest`, `needs_statement`, `unknown_key`, `signature`, `refused` or `image`), and `POST /releases/check` (`409 fetch_disabled` while fetching is off). The upload route alone takes a body past 64 KiB.

## Samples

- **A number per sample.** Each stored sample keeps the feeder's bytes and one number, the figure its metric is charted by (`wellfed_api::sample_number`): `power` the battery's millivolts, `rssi` dBm, `heap` free bytes, `loop` the 99th-percentile pass in µs, `gateway` the round trip in ms, `wifi` the access-point changes, `supply` the battery's millivolts while it reads one; none when the sample lacks it (no battery, no round trip). The mirror decodes it once at ingest. When what a number is changes, `SAMPLE_NUMBER_VERSION` moves and the next start numbers every stored sample again and rebuilds the hours their raw rows cover.
- **Hourly rollups** (count, min, max, mean over the samples with a number, per UTC hour) are folded in by sample id: the hourly `rollups` job adds every sample stored since its last run, so the hours a down server missed are filled by its first run after, and a late sample joins its hour. A read at hour resolution adds the samples past the last run from the raw rows, so it is whole between runs. Rollups are kept forever.
- **Retention.** Raw samples are kept forever unless `retention.raw_sample_days` is set; then the daily `retention` job (03:00) rolls up what is not yet and deletes the raw samples older than that many days, in whole UTC hours, leaving their rollups.
- **`GET /feeders/{id}/samples?metric=&from=&to=&resolution=raw|hour&after=&limit=`** (anyone who reads; `metric` required, `from` inclusive, `to` exclusive, `raw` by default) answers `{ resolution, items, next }`, oldest first, in pages of 1–1000 (default 100): `raw` items are `{ received, uptime_ms, value, number }`, `hour` items `{ hour, count, min, max, mean }`, an hour being in when it starts in the span. A bad metric, span, limit or cursor is `400 invalid`.
- **Volume.** A month of five feeders at the simulator's rates (3,024,000 rows) reads a month of one feeder's hours in about 5 ms, a page of 1,000 raw samples in about 4 ms and a whole day of one metric's raw samples, in pages, in about 220 ms, on a debug build (`wellfed-store` `tests/sample_volume.rs`, ignored in the gate for its 25 s of writes: `cargo nextest run -p wellfed-store --test sample_volume --run-ignored only --no-capture`).

## Backups

- **`VACUUM INTO`** copies the database into `backups/` in the data directory, on a read connection so ingest goes on: daily at 03:30 household time (the `backup` job) and on an owner's `POST /backups` (`Permission::ServerData`, `201` with `{ name, taken, bytes }`). A copy is written under a hidden partial name, synced and renamed into place, named by the second it was taken (`server-20261027T033000Z.db`, then `-2`, … within one second), and the newest `backups.keep` (default 7) are kept, whichever way they were taken. `GET /backups` lists them, newest first, as `{ items, next }` with `next` always null, since they are at most `backups.keep`. The copy just taken is never pruned, even when the clock stepped back and older names sort after it.
- **The key is not copied.** A copy holds the feeder keys sealed; `backups/README.txt` says that the key is `keys/server.key`, to back up once apart from the copies, and how to restore: stop the server, copy a backup to `server.db` (deleting any `-wal` and `-shm`), put the key back, start. The server refuses a `server.db` without its key file rather than making a new key.

## Export and import

Built Oct 27 (host simulation). An owner takes everything the server holds as one archive, and a fresh server takes it back.

- **The archive** (`wellfed_formats::export`, format version 1) is a POSIX tar: `manifest.json` first (canonical JSON: the format version, the encoding, the server's version, and each table's row count, size and SHA-256), then a file per table in a fixed order, each after the tables its rows name. `GET /export` (`Permission::ServerData`) writes `<table>.jsonl`, one canonical JSON object per row; `?format=csv` writes `<table>.csv` with a header row instead, for a spreadsheet. Rows are the database's own columns in its units (Unix milliseconds, the stored spelling of each closed set), with ids as UUIDs and other bytes as lowercase hex, in each table's primary key order, and every tar header has the same mode, owner and time, so a quiet server's database (one taking no writes between) exports the same bytes each time.
- **No secrets.** Accounts go without their password hashes and feeders without the server's sealed key; sessions and tokens are not exported, nor are the jobs' last runs (this server's schedule). Everything else is: the household, each feeder's cached state, raw events, gaps, samples, rollups and their marks, the projections and their version, the journal and the audit log (as it was: an imported journal entry gets no second audit row), installs, pets, foods and refills.
- **Streaming.** A tar needs each file's size first and the manifest leads, so the export reads one snapshot (a read transaction on a reader, while ingest writes on) twice: once to count, size and hash each table, and again to write it, checking it wrote what it counted. The second pass hands the route chunks of at most 64 KiB through a queue of four, so memory stays flat however large the tables; a reader that stops reading stops it, and so does shutdown. One export is written at a time, so exports hold at most one reader and one snapshot: another meanwhile is `409 export_running`.
- **`POST /import`** takes a JSON archive as the body, read as it arrives into one transaction on the writer: the manifest first (this format, every table), then each row, each table's count and hash checked at its end. Anything wrong (`422 archive_invalid`, a CSV archive included; the detail names the table whose row the database refused) rolls it all back, and so does an upload that runs past the length its manifest declares, goes 30 s without a byte, or is cut by the server stopping. Like `/setup`, it needs no login and runs only while no account exists (`409 import_closed` otherwise, for an owner too); a setup racing it either wins first or finds the imported accounts. It answers `201` with each table's rows and logs no one in.
- **After an import** every account has no password (an empty hash: a login costs the decoy hash, as for an unknown name, and is `bad_credentials`): `wellfed-server reset-owner` on the host gives an owner a temporary one, and that owner sets the others'. Every feeder is archived, since no key came with it, until it is adopted again, which continues its history under its id. The service then reloads the household zone, rebuilds the projections and sample numbers if the archive's versions differ from the build's, and marks writes and installs left running as interrupted, as a start does. An archive of a quiet server's database, imported onto an empty server, exports again byte for byte (before anyone logs in there, since a login is an audit entry).

## Installs

Built Oct 27 (host simulation). An owner installs a cached release on one feeder; nothing installs without that request.

- **The driver** is `wellfed_connect::update::push`, which the `wellfed` CLI's promotion and "Restore stock" run too. It drives `wellfed_client::update::Installer` over a `Steps` trait: the CLI's steps are its own session, which it reopens itself; the server's are `wellfed-mirror`'s `Operation::install`, whose every step is an `Operation::step` on the feeder's long lane. The feeder task keeps the session, so when it drops (a cut, or the feeder restarting into bank 0 to take the release) the driver waits for the link to come back and reads the status from the new session. A short command waits at most one chunk, so feeds and status reads keep working. Chunks are idempotent by offset, so a dropped session resumes where bank 0 says; an offer is never sent again behind the owner's back.
- **`POST /feeders/{id}/install`** (`Permission::ServerData`, owners only) takes `{ "release": <id> }` and answers `202` with the `Install` as it starts; it runs in the background. One install runs at a time per server: another is `409 install_running`. Refused first, before anything is sent: an unknown feeder or release (`404 not_found`), a release that no longer verifies (`422 release_refused`), a feeder the server has no session with (`503 feeder_offline`), and a feeder whose key for the server is not an owner's (`409 feeder_refused`, `refusal: forbidden`), since the feeder takes update requests only from an owner ([open question](../../SERVER_SPEC.md#open-questions)).
- **`GET /feeders/{id}/install`** (`Permission::Read`) is the running install, or else the last one (`404 not_found` before any), a release's or a [stock restore](#stock-backup-and-restore)'s. An `Install` names its `target` (`{ "kind": "release", "release": <id> }`, or `stock`) and version, the owner who asked, its state (`running`, `activated`, `already_running` when bank 1 already ran the build and nothing was sent, or `failed`), bytes `written` of `image_len`, and once failed a typed `failure`: `update_refused` (with the feeder's `UpdateRefusal`), `refused` (with a `FeederRefusal`), `stopped` (with the feeder's `UpdateStop`), `lost`, `unexpected`, `unreachable` (with whether the offer went out), `interrupted` or `server`.
- **The stream** carries `install` updates: the start, progress each 32 KiB written, and the end.
- **Storage** is the `installs` table: each install is written when it starts and when it ends; its progress lives in memory, in the one slot that also makes the one-at-a-time claim. Each start marks a row a stopped server left `running` as failed, `interrupted`, so after a restart the state is defined and the slot free. An end the store refuses twice is kept in memory for `GET` until then. The feeder ends a transfer that hears nothing for a minute, and a new install of the same release within it takes the transfer up.

## Stock backup and restore

Built Oct 27 (host simulation, against the simulator's banks and a synthetic stock; nothing has run on a feeder). The server keeps each feeder's own stock firmware, the copy promotion would otherwise destroy ([rollback](../firmware/consumer-installation.md#rollback)), and offers "Restore stock" from it.

- **Where the copy comes from.** `POST /feeders/{id}/stock/read` (`Permission::ServerData`) reads bank 1 over the device API (`ReadBank1`, a page per step on the long lane) while the replacement runs from bank 0 with stock still in bank 1, before promotion; the request waits for the read. Refused first, as the feeder would: a key that is not an owner's (`409 feeder_refused`, `forbidden`), a feeder offline (`503`), a session without `Capabilities::BACKUP` (`409 feeder_refused`, `unavailable`) and an install running on the feeder (`409 install_running`); bank 1 answers `unavailable` itself. A verified copy kept already is answered `200` without reading. After promotion the feeder holds no copy, so `POST /feeders/{id}/stock/upload` (octet-stream, the release upload's size limit) takes the one the installer kept, or an export: the vendor's container or its bare payload.
- **What is kept.** Either is checked with `wellfed-client`'s `StockBackup` against stock 1.1.15's pinned payload and container (`422 stock_refused` otherwise, nothing written), then kept by `wellfed-releases`' `Stock` under `stock/<identity>/` as the vendor's container, with its record, written whole. A kept copy is never replaced: the same copy again is `200` (and writes a damaged file back), another is `409 stock_differs`. Every read checks the file again; one changed on disk reads `damaged` and is neither exported nor restored (`404 no_stock_backup`). The pins are fixed on every real server; the test kit's are a synthetic stock's, and a copy checked against those never yields the `VerifiedBackup` an installer's promotion waits for.
- **`GET /feeders/{id}/stock`** (`Permission::Read`): `backup` (version, `verified` or `damaged`, SHA-256, size, source `feeder` or `upload`, when kept), `feeder_state` read for the request (the bank, the floor a restore's statement is signed for, and whether it serves bank 1 now; `null` offline or where the server's key is not an owner's), and `restore`, the running or last one. **`GET /feeders/{id}/stock/backup`** (`ServerData`) exports the file byte for byte.
- **`POST /feeders/{id}/stock/restore`** (`ServerData`) takes `{ "manifest", "downgrade" }`, hex: the kept backup signed as the stock image and a downgrade statement bound to the feeder and its floor. The server holds no signing key, so the owner signs on their computer (`wellfed server stock restore --version <n>` downloads the copy, checks it against the pins with `Frozen::stock`, signs with the update key and reads the floor from `GET /stock`). Nothing is sent unless the copy still verifies and the manifest is a stock-kind offer of exactly its payload (`422 restore_mismatch`); the feeder checks the signature and the statement. It then runs as an install does (`202`, an `Install` whose `target` is `{ "kind": "stock", "sha256" }`): the same slot, so one install or restore at a time (`409 install_running`), the same driver on the long lane, the same `install` updates on the stream and the same `installs` rows (`target`, `0013_stock_restores.sql`). Once activated the feeder runs stock, which serves no device API; a feeder that stops answering after the offer ends `unreachable` with `offered`, uncertain, and nothing is offered again.
- **Promotion waits for it.** As the installer's does: `POST /feeders/{id}/install` on a feeder idle on bank 0 (stock may still be in bank 1, its only copy) is `409 stock_backup_needed` until a verified copy is kept; an install already writing bank 1 goes on. `GET /stock`'s `feeder_state` is read for owners only.
- **Who.** Only an owner's key reads bank 1, reads the update state or offers an update on the feeder, so on a feeder adopted for the `server` role the read and the restore are refused before anything is sent ([open question](../../SERVER_SPEC.md#open-questions)); an uploaded copy is still kept and exported.

## Server first slice

Built Oct 24 (host simulation only; no server has met a real feeder yet).

- **Resources:** `/health`, `/status`, `/setup`, `/session` (GET for the logged-in user, POST to log in, PATCH to change one's own password, DELETE to log out), `/users` and `/users/{id}`, `/household`, `/feeders` (list, and adopt by invite), `/feeders/{id}` (live status, or the last copy marked `live: false`), `/feeders/{id}/settings` (GET with `ETag`, PATCH with `If-Match`), `/feeders/{id}/lock`, `/feeders/{id}/feed` and `/feeders/{id}/feeds/{feed}/cancel`, `/feeders/{id}/events`, `/feeders/{id}/samples`, `/commands` and `/commands/{key}` (the command journal), `/tokens` and `/tokens/{id}`, `/releases`, `/backups`, `/export`, `/import` and `/stream`. Every other resource in [the list above](#first-resources) is still to come: plans, clients, the derived food view, intake and audit.
- **Feeder connections** follow [server runtime](#server-runtime): a task per feeder subscribes after the newest stored event before anything else (from the feeder's oldest event when its epoch is a new one; `/feeders/{id}/events` reads the current epoch, or an older one with `?epoch=`), then sets the clock (again every 6 h), and records every event, gap and sample raw. A feeder that drops the server's key shows as `refused`. mDNS finds a feeder whose stored address fails, unless `discovery = "off"` (`--no-discovery`).
- **Writes** take `Idempotency-Key` as a UUID. The feeder remembers writes by request ID alone and every user shares the server's key, so the device request ID is a hash of the user, the feeder, the kind of write and the key: and the journal refuses one key reused for another request. A write without a key is journalled under a key the server draws, where the route allows that; a malformed key is `400 invalid`. A settings change re-reads the settings afterwards and records them as the write's outcome, so its answer (and a retry's) carries the new generation; if that read fails, the answer is `outcome_unknown`, since the change was made, and a retry reads them again.
- **The command journal** records every write before it is sent: its key, user, the token it came through (null for a session), feeder, kind, the SHA-256 of the request's canonical JSON (`wellfed_formats::canonical`), the device request ID, the feeder's boot count as last read, the newest event of the feeder's history it held, and its state (`sent`, `answered`, `unknown`, `resolved`) with the outcome and, for a feed, its number. A retry under the same key reads the journal first: an answered write returns its recorded outcome without reaching the feeder, so a retry after the feeder's ring of 32 write IDs has moved on never runs it twice; a `sent` one is in flight (each start marks leftovers `unknown`), so a retry meanwhile is answered `outcome_unknown` and sends nothing; an `unknown` one is sent again under the same device request ID when its kind allows (settings, lock and a cancel; a feed never is, since the feeder forgets ids at a reboot, so an uncertain feed is settled from its history, below). A settings change resent after the feeder's ring has moved on runs again against a base it already moved past, so it comes back `412` stale although the change was made; a replayed stale refusal reads the current settings, so while the feeder is offline it is `503 feeder_offline`. A key that already names another request (another body, kind of write, feeder or user) is refused `422 key_reused`, unsent. The send runs in its own task, so the entry reaches its answer even when the caller stops waiting; a server that stops first leaves it `sent`, and the next start marks it `unknown`, never failed. A write the feeder never received (it was offline) leaves no entry. `GET /commands/{key}` reads one (the caller's own; an owner's any, and another user's is `404` to a member), and `GET /commands?feeder=&state=` lists them oldest first in `{ items, next }` pages (`?after=&limit=`, 1–1000, default 100), so `state=unknown` is the uncertain list. The journal is kept forever.
- **Feeds.** `POST /feeders/{id}/feed` (`{ "portions": n }`, owner or member) requires `Idempotency-Key` and answers `202` with the journal entry once the feeder has reserved the feed: state `answered`, outcome `feeding` with the feeder's number for it, also in the entry's `feed`. There are no server-side limits; the feeder's refusals (`busy` while another feed runs, `invalid` past its cap) are the policy. The result is the feeder's `Fed` event, which settles the entry (state `resolved`, outcome `fed` with the portions asked for and confirmed, the outcome and reason) and reaches the stream as a `command` update and `GET /commands/{key}`. The feed number is the feeder's journal id, which keeps counting across restarts, so it names one feed: an answered feed is settled by the `Fed` event with its number, including the `uncertain` one a restart mid-feed leaves. A feed whose answer was lost has no number and is never sent again (`outcome_unknown`, `504`, for every retry): it is settled by the one `Fed` event after the history the server held when it was sent, by the server's key, or by the feeder as `uncertain` (an interrupted feed), whose number no other entry holds, when that match is unambiguous (it is the feeder's only unknown feed without a number and none is in flight); another client's or the panel's feed never settles it, and anything ambiguous stays `unknown`. Once the feeder has restarted since (its boot count changed), opened its journal (readiness not `starting`, `resolving` or `feeding`), and the server holds its whole history (through the live status's newest event, in the live session's epoch) with no gap and no `lost` events after the feed, a feed the history still does not settle is `resolved` as `unrecorded`: it never ran or its record was lost, so whether food was dispensed is not known, and it is never resent. A `Fed` event found later replaces `unrecorded`. The history is read again on every event, gap and link change of the feeder while a feed is open. `POST /feeders/{id}/feeds/{feed}/cancel` goes through the journal (kind `cancel`, resent under its device id when unknown, since a repeat stops nothing new): `204` while the feed runs, `409 feeder_refused` `not_found` once it has ended (so a retry of an unknown cancel may be refused `not_found` although the first attempt stopped it); the feed's result is still its `Fed` event, `cancelled`.
- **A refusal is final for its key:** `409 feeder_refused` with the feeder's reason, `busy` included. A refused write is final for its key: the feeder replays the refusal while it remembers the request ID (its per-client ring keeps the last 32 writes), so a retry with the same key is refused again; trying later is a new request with a new key. Nothing ran, so a refused feed is a new intent the user confirms again, unlike an `outcome_unknown` one, which only the same key may retry.
- **A command** is refused as offline until the connect sequence finishes, and then waits only behind an earlier short command and at most one step of a long operation (the feeder reads one request per client per pass and answers in order, so a short command sent beside a step is answered right after it); a read whose caller has stopped waiting is dropped unsent, while a write runs in its own task so the journal records its answer. A feeder heard from not at all for 90 s (time for a keepalive and its answer, with margin) counts as gone, so a feeder that loses power shows offline within that time rather than when the kernel gives up on the socket. The real feeder must answer a `Status` within that time even during flash writes; that is not yet checked on the bench.
- **Settings are read from the feeder**, never served from the cache, because a change must name the current generation; an offline feeder's settings are `503 feeder_offline`.
- **The stream** carries a `command` update for every change to a journal entry (answered, unknown, resolved), each user's own and everyone's to an owner. Its SSE `id` is `<epoch>-<seq>`, with the epoch fixed at each start (the `Clock`'s time when the service starts), so an id from before a restart resyncs; a `resync` carries an empty `id`. Samples are opt in with `?samples=power,rssi`. A stream rechecks its session or token every minute and ends once it is gone.
- **Hosts.** Every request must name the server by an IP address, `localhost` or a name given with `--host`, so a DNS-rebinding page (whose own name appears in both `Host` and `Origin`) is refused with `421 unknown_host`.
- **Samples** are always subscribed and kept: the simulator's rate (power every 5 s, signal every 30 s) is about 20,000 rows a day per feeder. See [samples](#samples).
- **The store** has one writer connection and a read pool, as [storage](#storage) describes. Before pending migrations run, `VACUUM INTO` copies the database beside itself as `server.db.before-<version>-<unix ms>`, the time being the one `Store::open` is given (the server passes its `Clock`'s), so the store never reads the wall clock. Until the first release the schema is one migration, edited in place.
- **Secrets:** the sealing key is `keys/server.key` in the data directory, made on the first start (mode 0600; [configuration](#configuration)). Each feeder key is sealed with ChaCha20-Poly1305, bound to the feeder's row ID and identity. Adoptions run one at a time, so two invites for one feeder cannot both enroll a key; a key the feeder enrolled but the server failed to store is logged with its fingerprint, to revoke.
- **Logins** hash at most two passwords at once (argon2id takes about 19 MiB each), and an unknown name costs the same hash as a wrong password.
- **The zone.** `POST /setup` takes the household's IANA zone; `GET /setup` carries the host's for the setup page to prefill. The household stores it by its canonical name, and every feeder keeps it twice: each clock sync carries its offset and next change, and the feeder's time settings hold its POSIX rule, so the feeder's clock and plans follow DST with the server down. The rule comes from the tzdb bundled with `wellfed-client` (the app and the CLI send the same one) (checked against that bundled zone, the one the syncs use) and is written only when the feeder holds another, on each connect, each time sync and each change of zone, so a feeder offline at a change gets it when it returns. A real zone no rule describes (one that changes irregularly) is refused `422 zone_unsupported`, an unknown name `400 invalid`, both by name. The feeder lets only an owner's key write its time settings, so a feeder adopted by a `server` invite keeps the syncs' offsets alone and the refusal is logged ([open question](../../SERVER_SPEC.md#open-questions)).
- **Shutdown** on Ctrl-C or SIGTERM ends the streams, stops the jobs (a job cut short is not recorded and runs at its next time), stops every feeder task (cancelling a connect sequence or command in flight) and closes the database.
- **Jobs.** One supervisor task in the binary (`wellfed_server::jobs`) runs the background jobs, one at a time, waiting on the service's `Clock` (`Clock::sleep_until`, which a test's manual clock wakes when it moves). `expiry` and `rollups` run hourly; `retention` at 03:00, `backup` at 03:30 and `release_check` at 04:00 in the household's zone (UTC before there is one). A job's next time follows its last recorded run (the `job_runs` table, one row per job), so a job missed while the server was down runs once at the next start; an hourly job that never ran runs at once, a daily one at its next time. `expiry` deletes idle sessions and every session of an account past its expiry; the account stays, disabled by its expiry, and every request still checks both, so the job only clears what a request would refuse. `rollups` folds the samples stored since its last run into their hours, and `retention` drops raw samples past `retention.raw_sample_days` ([samples](#samples)); `backup` copies the database into `backups/` ([backups](#backups)); `release_check` runs only while `releases.fetch` is on. Each job's body is a service method that takes the time it runs at (`Service::run_expiry`, …); `Service::run_job` runs one and records it.
- **Health and logs.** `GET /health` is liveness for anyone (a container or service manager probe): it reads nothing and answers the version. `GET /api/v1/status` is readiness for an owner: the version and start time, whether the store answers a trivial query on its writer and a reader (`ok` or `failing`, the error in the log rather than the body; the writer is one connection, so `/status` may wait for it behind a write in progress (up to the pool's acquire timeout), where `/health` never waits), each adopted feeder's link, and each background job's last run (`job`, `started`, `finished`, `outcome`: `succeeded` or `failed`, the reason in the log), once it has run. Every log line from a feeder's task (its session, its commands and the service's forward of its updates) is written under a `feeder{id=…}` span of its own (never under the request that adopted the feeder), and every HTTP request under a `request{id=… method=… path=…}` span; the id comes back in `x-request-id`, on refusals too. A request that brings one plain `x-request-id` (a proxy's: at most 64 bytes of letters, digits, `.`, `_` and `-`) keeps it, so the two logs share it; any other gets a fresh UUID. The query string is not logged. There is no metrics endpoint: a household has no scraper to feed.
- **Tests** follow the layers: the store against real SQLite, the service and the end-to-end test against `wellfed-feeder-sim` over TCP (through its `harness` module), and the HTTP API in process. The service tests fix the wall clock but run timers in real time with short intervals: tokio's paused clock would jump while sqlx's SQLite worker threads are busy, and fire timeouts that never happen.
- **Owner data (SV40).** `/pets` (name, species, birth date, daily kcal target, and the feeders that serve it) and `/foods` (brand, name, kcal per kg, bag grams, `grams_per_cup` defaulting to 110) are every role's to read and an owner's to write (`Permission::OwnerData`). A pet's weight log is `/pets/{id}/weights`, oldest reading first (a reading may be backdated, never set in the future); pets and feeders are many-to-many through `PUT`/`DELETE /pets/{id}/feeders/{feeder}`, and the links keep their history, so a past feed's intake is split among the pets the feeder served then. Each feeder serves one food at a time: `PUT /feeders/{id}/food` starts a period now, never dated before the last change, and the food at a time is the last change made by then, so a past feed's calories use the food it served; `GET /feeders/{id}/food/history` lists the periods with `from` and `to`, and a food any history or refill names is `409 food_in_use` to delete. `POST /feeders/{id}/refills` records grams or a full bag of the current food (`409 no_current_food` without one). `GET /feeders/{id}/food` answers the current period (the newest change, whatever the clock reads) and the newest refill; SV41 adds what remains, mapping each feed to a period by the feeder's clock (`at_s`) when valid, else its receipt time. Refills are never dated before the feeder's last, and every stored time is whole milliseconds. Weights, refills and the history page as `{ items, next }`. Photos wait for the UI.
- **Traces, statistics and the debug log** (SV13, SV14): see [below](#traces-statistics-and-the-debug-log).
- **Not yet:** command results on the stream.

### Traces, statistics and the debug log

- **What is read, and when.** Each feeder task has a collector beside it, which reads on the long-operation lane, so a step goes out only with no short command in flight or waiting: a feed or a status read waits at most one step. After each `Fed` event, at each connect and with each statistics read, it fetches the trace (`FeedTrace`, a page at a time through `wellfed_client::trace::Pages`) of every feed of the session's epoch that has none stored, among the 16 highest-numbered (the feeder's ring keeps the last 8–12, so an older feed is never asked for). A feeder that answers with a refusal (`not_found`: its ring moved on, or a restart lost the RAM trace) has that answer stored instead, so the feed is not asked for again; no answer, a trace that changed under the read (read again twice from its start) or `busy` leaves it to the next look. Every statistics section the session's capabilities allow (`Feeds` with `TRACES`, `Portions` with `CALIBRATION`) is read at each connect and then hourly (`Diagnostics::Read { stats_every }`), and the food section alone after each `Fed` event, ahead of its trace, so the hopper's state lags one feed at most; a refused section (`Memory` is an owner's, and busy during a feed) keeps its last read. Reads change nothing on the feeder, so asking again is always safe.
- **What is kept.** A trace is its summary (JSON) and its body in the feeder's own encoding, which the model's `Trace::decode` reads; the mirror stores only one that decodes. The latest read of each section is kept whole, with the feeder's boot count it was read in (counts "since this boot" start again at each), and every read of the food section is also a typed `food_readings` row (`present`, `emptied`, `refilled`, `feeds_since_refill`): the only way the food beam reaches the server, which the food views and the empty-hopper alert read (`Service::food_readings` over a window of receive times). None of it is a projection, so a projection rebuild leaves it alone.
- **Routes.** `GET /feeders/{id}/feeds/{feed}/trace` (`?epoch=`, else the newest epoch with that feed's trace) answers `{ epoch, feed, fetched, record }`, `record` being `{ "state": "stored", trace }` or `{ "state": "missing", refusal }`, and `404 not_found` before it is fetched. `GET /feeders/{id}/stats` answers `{ sections: [{ section, received, boot_count, stats }] }` in the sections' order. Both are every role's (`Permission::Read`).
- **The debug log.** `GET /feeders/{id}/log` is Server-Sent Events of `LogEvent`s (`{ "kind": "lines", seq, dropped, lines }`, or `{ "kind": "lagged", batches }` for a follower that fell behind), for owners only (`Permission::FeederAdmin`). The feeder has no op that stops a tail, so the mirror never asks for the log on the feeder task's session: the first follower opens a second session with the same key and sends `TailLog { after: None }` (the lines its RAM tail keeps, then each new one); a follower who joins a running tail gets the lines from then on. The tail's session closes within a second of the last follower leaving, when the feeder's task stops, or when the feeder drops it, and every follower's stream then ends: follow again. The feeder serves four sessions at once, so a followed log costs one; the server's own goes first, and a reconnect refused `Full` while a tail runs ends the tail. Nothing of it is stored. The feeder serves the log only to an owner's key, so on a feeder that enrolled the server as `server` the route answers `409 feeder_refused` `forbidden`; a feeder that is offline is `503 feeder_offline`.

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
| Settings | `toml` (already, in `wellfed-build`), `serde_path_to_error` (already, under axum), `clap` with `env` |
| Tests | `insta` (snapshots), `proptest` (codec round trips), `tower::ServiceExt` (in-process requests), `sqlx::test` |
| Hygiene | `cargo-deny` (licenses, advisories, duplicates) and `cargo-nextest`, both in place ([Rust standard](../development/rust.md)) |

## Quality standard

The repository's [Rust conventions](../../AGENTS.md#rust-conventions) apply in full. On top of them:

- **Boundaries are crate boundaries.** If a change wants SQL in a handler or axum in the service, the design is wrong. Move the code; don't add the dependency.
- **One error enum per crate** (`thiserror`), split by domain where a crate has several (the service's `AccountError`, `CommandError`, …, each wrapped whole in its `Error`), as the [Rust standard](../development/rust.md) asks. The server maps service errors to `problem+json` in one `IntoResponse`. No `anyhow` below the binary.
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
- Answered: backups are a scheduled `VACUUM INTO` inside the server, without the sealing key, whose place a README beside them names ([backups](#backups)).
