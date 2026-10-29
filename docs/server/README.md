# The server

The Wellfed server is an optional program you run at home, on a small Linux machine or in a container. It talks to your feeders on their behalf and gives everyone in the household one place to see and control them. This page says what it adds, how it fits beside the feeder, and where to read more.

## What it is, and what it isn't

The feeder doesn't need the server. It keeps its own feed journal, event history and schedules, and the app and the `wellfed` CLI can talk to it directly on your home network. If the server is down, the feeder keeps feeding.

The server adds what a small device can't hold:

- **Remote access.** Reach your feeders from outside the home, through a reverse proxy or a VPN.
- **Accounts and audit.** Each person logs in as themselves, with a role (owner, member or viewer). The audit log names who fed, changed a setting or adopted a feeder.
- **Long history.** Every event and sample the feeder reports, kept for as long as you like, with hourly rollups for charts.
- **Owner data.** Pets, foods, weights and refills, which the feeder doesn't track.
- **Releases and installs.** It fetches or takes signed firmware releases, verifies them, and installs one when an owner asks. It also keeps each feeder's stock firmware backup, so you can go back to stock.

One server serves one household, with as many feeders as you own.

**Status: Experimental.** Everything here is built and tested against the host-simulated feeder. No server has met a real feeder yet. See the [roadmap](../roadmap.md).

## Two paths, one feed

When a server is configured, the app talks to it whenever it can reach it. That way remote access works and the audit log names the person. Each owner's or member's phone also holds its own key on every feeder. When the server is down, the phone uses that key on the home network, for reads and new requests only. A viewer's phone holds no key, so the server alone enforces a viewer's limits.

The danger is a retry that crosses paths. The server and the phone send different request ids, so a feed that timed out on one path and was retried on the other could feed twice. So **an uncertain write never switches path.** It is retried on its own path under its own key, or resolved from the feeder's history (a feed by its `Fed` event). It is never resent on the other path. The CLI follows the same rule: each command takes one path, the device API or `wellfed server`, with no fallback between them.

The server enforces its half:

1. A write whose answer was lost is answered `504 outcome_unknown` and kept unknown under its `Idempotency-Key`.
2. A retry under that key reads the server's command journal. For a feed it sends nothing new.
3. The feed is settled later from its `Fed` event. You can read the result at `GET /api/v1/commands/{key}`, and it arrives on the live stream as a `command` event.

So a client can resolve a lost feed on either path without resending it. Holding an uncertain write to its own path is the app's half.

This costs the feeder a client slot per phone. A feeder has 8 slots: the server, the CLI and up to six phones.

Every device operation the app can make has a server route, or a stated reason it has none, in `wellfed_service::parity`. It is an exhaustive match over the device API's operations, so a new operation doesn't build until it gets one. Some routes need the feeder's owner role: Wi-Fi, reboot, revoking a client, a phone's invite, the time zone rule, installs and the stock backup. They work only when the server was adopted with an owner's invite. With the `server` role, the feeder refuses them `forbidden`.

## The HTTP API

The API lives under `/api/v1`, as JSON. The complete reference is [`openapi.json`](../../src/server/wellfed-server/openapi.json), which is generated from the handlers and checked for staleness in `make check`. The web UI's types come from it too.

| Group | Routes | What they do |
| --- | --- | --- |
| Setup and accounts | `/setup`, `/session`, `/users`, `/tokens`, `/household` | The first owner, logins, people and their roles, phone tokens, the household's name, units and zone |
| Feeders | `/feeders`, `/feeders/{id}`, `/feeders/{id}/settings`, `/plans`, `/lock`, `/desiccant/reset`, `/wifi`, `/reboot`, `/play`, `/clients`, `/invites`, `/phones` | Adopt and remove feeders, live status and the next meal, settings, feeding plans and administration |
| Feeding | `/feeders/{id}/feed`, `/feeds/{feed}/cancel`, `/commands`, `/commands/{key}` | Feed, cancel, and the journal of every write |
| History | `/feeders/{id}/events`, `/samples`, `/feeds/{feed}/trace`, `/stats`, `/log`, `/audit` | Events, samples, feed traces, statistics, the live debug log and the audit log |
| Owner data | `/pets`, `/foods`, `/pets/{id}/weights`, `/pets/{id}/feeders/{feeder}`, `/feeders/{id}/food`, `/refills` | Pets, foods, weights, which feeder serves which pet, refills |
| Firmware | `/releases`, `/releases/check`, `/feeders/{id}/install`, `/feeders/{id}/stock…` | Releases, installs, the stock backup and restore |
| Server data | `/backups`, `/export`, `/import`, `/status` | Backups, a full export, import onto a fresh server, readiness |
| Live | `/stream` | Server-Sent Events for every feeder you can see |

`GET /health` sits outside `/api/v1`. It needs no login and answers the version.

Some conventions hold across the API:

- **Errors** are RFC 9457 `application/problem+json`, with a typed `code` from a closed set. There is no free-text error.
- **Writes** carry an `Idempotency-Key` header (a UUID). A feed requires one.
- **Edits** carry `If-Match` with the generation of the settings or plans they change. A stale one gets `412` with the current settings or plans.
- **The stream** resumes from `Last-Event-ID`. When that id is too old, the server sends `resync` and the client fetches again.

## Read next

| I want to… | Read |
| --- | --- |
| Install and run the server | [Running the server](running.md) |
| Understand how it's built, or change it | [Server design](design.md) |
| Know what the feeder itself serves | [Device API](../firmware/device-api.md) |
| See the web UI | [Web UI](../web/README.md) |
| See the product the server is part of | [Product architecture](../firmware/product-architecture.md) |
| Check what works today | [Roadmap](../roadmap.md) |

## Evidence

- **Simulation**, October 24–27, 2026: the server's crates and their tests run against `wellfed-feeder-sim` over real TCP and Noise. Feeds, uncertain writes, installs, the stock backup and restore, export and import were exercised this way.
- **Static analysis**, October 27, 2026: the route groups above match the committed `openapi.json`, and the role rules match `Permission::can` in `wellfed-service`.
