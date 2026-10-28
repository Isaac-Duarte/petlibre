# Running the server

This guide is for an owner who wants to run the Wellfed server at home. It covers installing it, where it keeps its data, what to back up, its settings, reaching it from outside, and adding people. For what the server is, start with [the server](README.md).

The server is one static binary. You can run it under systemd or in a container. Either way it keeps everything in one data directory, and it never needs a separate database.

## Install it

Each GitHub release carries one static binary per target, `wellfed-server-x86_64-unknown-linux-musl` and `wellfed-server-aarch64-unknown-linux-musl`, and the same binary in a container image. Pick the way that suits your machine.

### With systemd

[`packaging/wellfed-server.service`](../../src/server/wellfed-server/packaging/wellfed-server.service) is an example unit for the static binary.

1. Install the binary and the unit:

   ```sh
   sudo install -m 0755 wellfed-server-x86_64-unknown-linux-musl /usr/local/bin/wellfed-server
   sudo install -m 0644 wellfed-server.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now wellfed-server
   ```

2. Run `curl http://127.0.0.1:8080/health`. You should see the server's version.

How the unit runs it:

- **Its own user.** `DynamicUser=yes` makes a user for the unit alone. It owns only `StateDirectory=wellfed-server`, which is `/var/lib/wellfed-server` (mode 0700, files 0600 through `UMask=0077`).
- **Locked down.** The rest of the system is read-only or hidden (`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `PrivateDevices`, no capabilities, `@system-service` system calls only). `systemd-analyze security` rates it 1.3.
- **Discovery works.** The unit shares the host's network, so mDNS finds feeders whose address changed. Address families are limited to IPv4, IPv6, Unix and netlink, which mDNS needs to list the interfaces it browses on.
- **Local only at first.** It listens on `127.0.0.1:8080` until `listen` says otherwise.

### In a container

The image (`FROM scratch`) holds the binary, the CA certificates its GitHub release checks need, and an empty `/tmp`. The zone database is built into the binary. It runs as uid 65532, keeps its data in the volume at `/data` (`WELLFED_SERVER_DATA`), and listens on `0.0.0.0:8080` (`WELLFED_SERVER_LISTEN`). Arguments after the image name are the server's flags.

mDNS is multicast, which a container's bridge network doesn't pass. So you have two choices.

**Host networking keeps discovery.** This is the simplest:

```sh
docker run -d --name wellfed-server --restart unless-stopped \
    --network host -v wellfed-server:/data wellfed-server
```

It listens on the host's port 8080 directly. If a proxy on the host is the only way in, set `WELLFED_SERVER_LISTEN=127.0.0.1:8080`.

**A bridged network works by address.** Turn discovery off:

```sh
docker run -d --name wellfed-server --restart unless-stopped \
    -p 8080:8080 -v wellfed-server:/data wellfed-server --no-discovery
```

The server then adopts and reconnects at the stored address only. You give each feeder's address when you adopt it (`address` in `POST /feeders`: an IP address, the API's port by default, or `IP:port`; a DNS name is not resolved). Adopting with no address is refused (`400 invalid`).

- **Give each feeder a fixed address,** with a DHCP reservation. A feeder whose address changes stays offline until it is back at that address.
- **The feeders must be reachable from the container.** A bridge network routes out to the LAN but not back in, which the feeder API never needs.
- **The volume** is a named volume (as above) or a bind mount of a local directory owned by uid 65532. `--user` picks another uid. With rootless podman, `--userns=keep-id` maps yours.

## The data directory

The server keeps everything in one directory, given by `--data` or `WELLFED_SERVER_DATA`. Under systemd it's `/var/lib/wellfed-server`. In the image it's the volume at `/data`.

| Path | What |
| --- | --- |
| `server.db` | The database. |
| `server.toml` | The settings. Optional: every key has a default. |
| `keys/server.key` | The sealing key: 32 bytes, mode 0600, in a 0700 directory. It seals every feeder key in the database. |
| `backups/` | Daily copies of the database (mode 0700, each copy 0600), with a `README.txt` saying where the key is and how to restore. |
| `releases/` | Verified firmware releases, each with a record. |
| `stock/` | Each feeder's stock firmware backup: `stock/<identity>/stock-1.1.15.bin` (the vendor's container) and its `.json` record. |

The key file is placed with a hard link, so the directory must be on a filesystem that has them. Any local one does. Some network and FUSE mounts don't, and the first start then fails with a write error. Keep it on local disk, or on a volume whose driver supports hard links.

The first start makes the key. If you start the server on a `server.db` with no key file, it refuses, because a new key couldn't open the feeder keys. That usually means a backup was restored without its key.

## Back it up

The server copies its database into `backups/` every day at 03:30 household time. An owner can take one any time with `POST /api/v1/backups`. The newest 7 are kept (`backups.keep`).

> [!WARNING]
> The backups leave out `keys/server.key`. A restored `server.db` opens only with its key, and without it the server can no longer reach your feeders. Copy `keys/server.key` somewhere off the machine once, and copy `backups/` off the machine regularly. Or back up the whole data directory.

To restore from a backup:

1. Stop the server.
2. Copy a backup to `server.db`, and delete any `server.db-wal` and `server.db-shm` beside it.
3. Put `keys/server.key` back.
4. Start the server.

## Settings

`server.toml` in the data directory holds what is neither a secret nor a household setting. The household's time zone lives in the database. [`server.example.toml`](../../src/server/wellfed-server/server.example.toml) lists every key with its default and a comment. Copy it and keep only the keys you change.

| Key | Default | What |
| --- | --- | --- |
| `listen` | `127.0.0.1:8080` | Where the API listens. `0.0.0.0:8080` serves the household's other devices. |
| `hosts` | none | Names the server is reached by, besides IP addresses and `localhost`. Any other name is refused `421 unknown_host`, which stops DNS rebinding. |
| `label` | `wellfed-server` | The label the server enrolls under on each feeder, 1-16 bytes. |
| `discovery` | `mdns` | `off` adopts and reconnects by address only. |
| `trusted_proxies` | none | Proxies (addresses or CIDR ranges) whose forwarded headers are believed. |
| `retention.raw_sample_days` | keep forever | Days raw samples are kept. Hourly rollups are always kept. |
| `releases.fetch` | `false` | Check the project's GitHub releases daily, at 04:00. |
| `releases.project_keys` | `[]` | The project's update keys, 64 hex digits each. |
| `releases.owner_keys` | `[]` | Your own enrolled update keys, 64 hex digits each. |
| `backups.keep` | `7` | Backups kept. |

Every key has a flag, and every flag an environment variable: `--listen` (`WELLFED_SERVER_LISTEN`), `--host` (`WELLFED_SERVER_HOSTS`, comma-separated), `--label`, `--discovery` (or `--no-discovery`), `--trusted-proxy` (`WELLFED_SERVER_TRUSTED_PROXIES`), `--raw-sample-days`, `--releases-fetch true|false`, `--release-project-key` and `--release-owner-key` (`WELLFED_SERVER_RELEASE_PROJECT_KEYS` and `..._OWNER_KEYS`, comma-separated) and `--backups-keep`. Each variable is `WELLFED_SERVER_` and the flag's name.

- **A flag beats its variable,** and either replaces the file's value whole. `--host` given once replaces the file's `hosts`.
- **An override can't clear a list.** No flag or an empty variable means "not given". To drop every host or proxy, edit the file.
- **`--no-discovery` beats** `--discovery` and its variable.
- **A bad file stops the start.** An unknown key, a wrong type or an out-of-range value (a 17-byte label, `raw_sample_days = 0`, a host name with a port) is refused with the key's path (`retention.raw_sample_days`, `hosts[1]`) and its line. A missing file means the defaults.

Under systemd, put `server.toml` in `/var/lib/wellfed-server/` as root: `sudo install -m 0644 server.toml /var/lib/wellfed-server/`. The server only reads it, and it holds no secret, so a root-owned file is fine. The next start reads it. Or set `WELLFED_SERVER_*` variables with `systemctl edit wellfed-server`.

## Reach it from outside

The server speaks plain HTTP. On your home network you can reach it directly: set `listen = "0.0.0.0:8080"`, leave `trusted_proxies` empty, and use plain HTTP. A VPN into your home network is one way in from outside, and needs nothing more.

The other way is a reverse proxy that terminates HTTPS. Here is Caddy:

```caddyfile
feeder.example.org {
    reverse_proxy 127.0.0.1:8080 {
        # A request id both logs share (the server keeps one plain id).
        header_up X-Request-ID {http.request.uuid}
    }
}
```

And the matching `server.toml`:

```toml
listen = "127.0.0.1:8080"         # only the proxy reaches the server
hosts = ["feeder.example.org"]    # the public name, besides IPs and localhost
trusted_proxies = ["127.0.0.1"]   # the proxy's address, or its range
```

Caddy obtains and renews the certificate, preserves `Host` and sets `X-Forwarded-For` and `X-Forwarded-Proto`. The server reads those headers only from a peer in `trusted_proxies`:

- **The client's address** comes from `X-Forwarded-For` (or `Forwarded` when that is absent), read right to left. The first hop that isn't a trusted proxy is the client, and anything a client wrote to its left is ignored. If `X-Forwarded-For` is present at all, `Forwarded` is ignored, so your proxy must set or append to `X-Forwarded-For`. Caddy does, as does nginx with `$proxy_add_x_forwarded_for`. Each comma-separated entry is read on its own. The address drives the per-address login limit and is recorded with each login in the audit log.
- **The scheme** is `X-Forwarded-Proto`'s last value (or the last `Forwarded` element's `proto`). `https` makes the session cookie `Secure`. Behind a chain of proxies, the front one sets it from the client's connection and each one behind passes it on.
- **From any other peer** these headers are ignored: the address is the connection's and the scheme is plain HTTP. So a server reached both directly and through the proxy keeps its direct clients' real addresses.

> [!WARNING]
> Anyone listed in `trusted_proxies` can claim to be any client. List only the proxy's own address, or a container network's range such as `172.16.0.0/12` when the proxy runs in one. Never list your LAN or the internet.

The public name goes in `hosts`, since the server refuses names it wasn't given. The `Origin` check compares the browser's `Origin` with `Host`, so the pages work through the proxy unchanged.

A request's own `x-request-id` is kept when it is one plain id: at most 64 bytes of letters, digits, `.`, `_` and `-`. So the proxy's id appears in both logs. Anything else gets a fresh UUID.

## Add the first owner

A fresh server has no accounts. The first visitor creates the first owner. The server doesn't serve its web UI yet, so you do this over the API.

1. Run `curl http://127.0.0.1:8080/api/v1/setup`. You should see `"open": true`, and the host's time zone as `zone` if it has one.
2. Create the owner with a name, a password of at least 10 characters, and the household's IANA zone:

   ```sh
   curl -X POST http://127.0.0.1:8080/api/v1/setup \
       -H 'Content-Type: application/json' -H 'Origin: http://127.0.0.1:8080' \
       -d '{"name": "alex", "password": "…", "zone": "Europe/Berlin"}'
   ```

3. Run `wellfed server login --server http://127.0.0.1:8080 --name alex` and type the password. You should see the CLI keep a token, and `wellfed server feeders` then works.

Setup closes as soon as any account exists.

The zone must be one the feeder can follow on its own, because the server writes its daylight-saving rule to each feeder. A zone with irregular changes that no rule describes is refused `422 zone_unsupported`. An unknown name is `400 invalid`.

## Add a member's phone

1. As an owner, add an account for the person with a temporary password (`POST /api/v1/users`, from a login), and give it the member role. A viewer can only read. An account can carry an expiry, for a pet sitter.
2. The person logs in and sets their own password (`PATCH /api/v1/session`).
3. From that login, the phone gets a token of its own (`POST /api/v1/tokens`). The token is shown once. The phone sends it as `Authorization: Bearer`.

Outside the home the phone also needs the server's public URL, such as `https://feeder.example.org`. A token needs no cookie or `Origin`. If the phone is lost, revoke its token on its own.

A phone that should keep feeding while the server is down also holds its own feeder key. That key works only on the home network ([two paths, one feed](README.md#two-paths-one-feed)).

## Reset a locked-out owner

If the only owner forgets their password, run `reset-owner` on the host with the server stopped. It prints a new temporary password.

```sh
wellfed-server --data DIR reset-owner
```

Under the systemd unit, run it as the unit's user:

1. `sudo systemctl stop wellfed-server`
2. `sudo systemd-run --pty --wait -p DynamicUser=yes -p User=wellfed-server -p StateDirectory=wellfed-server -p UMask=0077 /usr/local/bin/wellfed-server --data /var/lib/wellfed-server reset-owner`
3. `sudo systemctl start wellfed-server`

After an [import](design.md#export-and-import), every account has no password, so `reset-owner` is also how the first owner gets back in.

## Packaging

This part is for whoever builds the release.

- **`make server-dist`** builds both static binaries into `dist/server/`.
- **`make server-image`** builds the container image. Set `CONTAINER=podman` for podman.
- **Publishing** the binaries to a GitHub release, and pushing the image to a registry, is the owner's step. Nothing in the repository does it.

Both run [`packaging/Containerfile`](../../src/server/wellfed-server/packaging/Containerfile). A `rust:1.99.1-alpine3.24` stage on the build machine's own platform links both targets with zig (`cargo zigbuild`). So the aarch64 binary and the arm64 image build on an x86_64 host with no emulation. Use `docker buildx build --platform linux/amd64,linux/arm64` for a multi-arch image. rustls with its aws-lc provider and the bundled SQLite both build for musl this way.

Without a container, a host with zig and `cargo-zigbuild` runs the same build after `rustup target add` for both targets:

```sh
cargo zigbuild --release --locked -p wellfed-server --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
```

A plain `cargo build --target x86_64-unknown-linux-musl` also needs a musl C compiler (`musl-gcc`, or Alpine's own gcc) for aws-lc and SQLite.

### The release check

`wellfed-server`'s `tests/release.rs` checks the server as it ships. It starts the server as a separate process on a fresh temporary data directory, sets the household up, and adopts the simulated feeder by address with `--no-discovery`. It reads readiness and the feeder's live status, and takes a backup. Then it stops the server with SIGTERM, as systemd and container runtimes do, and expects a clean exit. Last, it starts the server again and checks that it reconnects by the stored address.

- **`make check`** runs it against the debug build.
- **`make check-server-release`** runs it against this host's binary in `dist/server/` (`WELLFED_RELEASE_SERVER=<binary>`), then against the image (`WELLFED_RELEASE_IMAGE=<image>`, `WELLFED_RELEASE_RUNTIME=docker|podman`). The image run is an ignored test, so the gate never needs a container runtime.

Nothing in either is left running.

## Evidence

- **Simulation**, October 27, 2026: the release check above runs in `make check` against the debug build. Both musl targets were built through the Containerfile, rustls with aws-lc and the bundled SQLite included.
- **Static analysis**, October 27, 2026: the settings table matches `server.example.toml`, which a test parses; the unit's hardening matches `packaging/wellfed-server.service`.
