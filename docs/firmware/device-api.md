# Device-hosted API (FW47)

Status: physical (October 24, 2026), LAN recovery with the owner at the panel. The feeder serves its own Noise-encrypted API over TCP from the firmware's loop task. Every request, the event history and the live samples go over it, and the normal role no longer opens an MQTT session. The wire contract is in [API design](../api/README.md#device-protocol); this page records how the firmware serves it and what has run on the feeder.

## In the firmware

- **One loop task.** The feeder runs two tasks, the button service and the loop ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)), and `wellfed-fw-feeder`'s `api` module steps the API inside the loop, after the panel's owner. Every socket call is non-blocking (`wellfed_api_*` in `example_entry.c`): the loop takes what is ready and moves on.
- **Sans-IO server.** `wellfed_feeder::api::Server` holds four session slots. The firmware's driver copies received bytes in and sends what a slot holds; the server never touches a socket, so host tests drive it with `wellfed-client` directly.
- **One handshake operation per pass,** across all slots, and none while a feed runs (the input is there; there is no feeding owner yet). A slot reads one frame per pass and writes again only once its last frame is out, so a flooding client costs its own turn.
- **Sessions are Noise IK** under the feeder's identity: the client's key travels encrypted in the first message, and the feeder checks it against the enrolled clients the settings writer publishes (`wellfed_hal::keys::publish_clients`). When the table changes, every session whose key is gone closes.
- **Writes are remembered per client key**: the last 32 write ids and their compact outcome, so a retry after a reconnect gets the first answer. A settings or Wi-Fi write names the generation it was based on and is refused `Stale { current }` when the store has moved. The loop is the only requester since the MQTT path went, so the check and the queueing cannot interleave with another writer.
- **Roles.** An owner does everything. A member, or the server, may feed, change settings and the lock, play, sync the clock, list clients and invite members; not Wi-Fi, revocation or reboot. An update-signer key never opens a session.
- **Events.** The loop queues records in `wellfed_hal::events` (16 in RAM; overflow is counted and recorded as a `Lost` event). The settings writer appends them to the ring in storage sectors 13–24 (`wellfed_core::store::events`: 32-byte records, 1,408 kept at least) and answers the API's reads. It polls every 5 ms while either waits. Recorded now: boot, lock changes (panel, idle relock, or which client), settings and Wi-Fi writes with their client, clock syncs, invites redeemed, recovery and revocations.
- **Samples** go only to subscribers: power every 5 s, Wi-Fi signal every 30 s. A sample not sent before the next is replaced.
- **mDNS.** `wellfed_core::mdns` answers `_wellfed._tcp` on one UDP socket in the loop: PTR, SRV (port 4747), TXT (`v=1`, `id=` the identity key) and A, for host `wellfed-<first 4 key bytes>.local`. It announces twice on every address change and answers legacy unicast queries. The vendor responder (33.5 KB and its own task) stays unlinked.
- **Invites and recovery** (from FW45). `CreateInvite` mints a secret, which is the PSK of the new client's XXpsk3 handshake. Its third message carries the label, and the key enrolls through the keys mailbox. The recovery window runs FW45's setup exchange (XX plus the code proof) over the stream, so the owner keys the client's code in before it becomes an owner. A LAN neighbour cannot slip in while the window is open, and the window closes at the first offer. Recovery takes no Wi-Fi.

## What went away

- **Requests over MQTT.** The firmware's request parser, its reply writers, request signing (`request_auth`) and the health nonce are deleted. The normal role starts no `wellfed-net` task.
- **Confirmation.** FW11 confirmed a trial once the MQTT session was ready. It now confirms on FW07's network check: DHCP, then a DNS lookup. The OTA watch still uses the broker until FW48, so a build that confirms but cannot reach the broker would need a UART recovery. That risk is accepted until offers move to the API.
- **Kept until FW48:** the OTA watch session, bank 0's updater and its recovery-role health, and the broker credentials (with the now unused request key, so existing private files still parse).

## Resources

From the `feeder` builds (manifest audits) and `PLAPI1` records:

| | Before (FW45, v109) | FW47 (v119) |
| --- | ---: | ---: |
| Payload | 868,352 | 942,080 |
| Loop stack bound | 3,912 of 6,144 | 4,552 of 8,192 |
| Heap free, steady | 146,464 (FW44) | 125,088 |

- **Stack:** the loop's stack is now 8 KiB. At 6 KiB the bound sat 56 bytes under the audit's 25 % margin. The recovery responder's 1.2 KB frame runs out of line (`rescue_op`) so it never nests under the handshake step's 1.1 KB.
- **Heap:** flat at 125,088 free over minutes and through a garbage flood; the difference from FW44 is not yet attributed.
- **Sockets:** one listener, four sessions, the mDNS socket and, until FW48, the OTA watch session.

## Physical (October 24, no owner action)

Each install below used a bounded `serve` for the FW10 offer.

- **v114** (`fw47-api-02`): `PLAPI1 listen socket=api open=1`. `wellfed status` opened an IK session as Owner. Settings writes moved the generation 3031 → 3032 → 3033. Lock, unlock and a clock sync were recorded, each with the client's fingerprint, and `events` read all six records.
- **Invite and revoke:** an owner minted a member invite, and `wellfed join` redeemed it (XXpsk3) with a scratch key, which then opened a session as Member. With that member's `events --follow` session open, `revoke` closed the session at once, and the key was refused afterwards (`OpenRefusal::Unknown`). The history showed `Enrolled { path: Invite }`.
- **Samples:** a subscriber received power samples every 5 s.
- **Client limit:** with four sessions held open, a fifth connection was closed at accept.
- **Flood** (`fw47-flood-01`, about 35 s in all): 112 connections, each of 4,000 random bytes, were closed as `malformed` one by one. Meanwhile an owner's `status` kept answering in about 0.1 s. There was no supervisor stop, and the heap was unchanged. The Wi-Fi driver logged `Cannot allocate pbuf` four times during the burst.
- **History across a cold boot** (`fw47-mdns-01`): `PLCFG1 events open=1 last=10 oldest=1`.
- **mDNS:**
  - The first socket opened on the SDK's static pre-DHCP address (192.168.1.80) and never saw a multicast query. The responder now waits for a DHCP address.
  - `wifi_register_multicast_address` returned -95 and is not needed.
  - The feeder then received the host's queries and answered them all (`mdns_in=15 mdns_out=13 mdns_failed=0`). A legacy unicast query from the host got a correct A record.
  - Discovery from the bench host failed at first: its firewall's input chain dropped `udp dport 5353` under a drop policy, so answers never arrived. Once the owner allowed UDP 5353 on the hotspot interface, `wellfed status` found the feeder without `--address`.
- **The bench on the API** (`itest`, the owner key, the address from the neighbour table):
  - `fw47-itest-01`, smoke tier on v119: `ota-update`, `requests` (the clock 3 ms from the host's after a sync) and `request-auth` passed.
  - `fw47-itest-02`, standard tier on a fresh build (v120): `ota-update`, `requests`, `request-auth`, `settings-persist` and `wifi-fallback` passed. `network-bounce` failed because the outage showed as a DNS loss, which `Status` did not report, so `dns_losses` was added.
  - `fw47-itest-03` (v121) then passed `ota-update` and `network-bounce`.
  - `qa-20261024-01` (from `ccf3e2d`): `ota-update`, `requests`, `request-auth`, `settings-persist`, `wifi-fallback` and `network-bounce` passed. `qa-20261024-03` and `-04` passed `ota-recovery` and `ota-power-cuts`: a cut mid-download made bank 0 start over, and a cut early in trial boot 1 counted boot 1 again before the build confirmed. `ota-power-cuts` installs `speaker`, which serves no API, so the bench takes its UART confirmation.
  - `Status.boot_count` reported KM0's whole register 4, trial bits included (`2533425155` = `0x97010003`); it now reports bits 0-15.
- **MQTT-free normal role** (v118, `fw47-api-06`): confirmed on its trial boot with no `PLMQT1` line. The OTA watch started and saw the pending offer, and the API answered. **v119** (`fw47-api-07`) was then offered to v118, which gave way through its watch, and v119 confirmed.

## Recovery with the owner at the panel (October 24)

- **`fw47-recover-05`, passed:** the owner unlocked the panel and held Lock and Dispense (6.3 s): `pair recovery state=opened`. `wellfed recover` connected, the window closed at its offer (`pair confirming path=recovery`), and the owner keyed the six presses in: `pair recovery state=entered`, `PLCFG1 keys change=enroll result=done enrolled=2`, the session ended `joined`. The new key opened an Owner session and listed both clients; the bench key then revoked it.
- **`fw47-recover-04`, refused as it should be:** only five presses registered (one of two Dispense presses in a row never reached the button service, `dropped=0`), and 60 s after the offer the feeder refused it as `unconfirmed`, enrolling nobody.
- **Earlier tries:** a 4.9 s hold, under the 5 s threshold, opened no window, and the connection was turned away (`ended reason=recovery`), as it is for a closed window.
- **Fixed from these runs:** presses taken by a code are now panel activity, so the panel no longer relocks 30 s after the unlock while the code is keyed in (it did in `fw47-recover-04`, harmlessly). Code presses are logged under their path (`pair recovery keyed=N`); they were all labelled `setup`.

## Host-tested only

- **Recovery:** a wrong code is refused and enrolls nobody; one owner per window (`wellfed-client/tests/api.rs`).
- **The rest of the suite:**
  - stale writes and retried ids;
  - role limits;
  - one handshake operation per pass, and none while feeding;
  - idle and handshake timeouts;
  - the event ring's wrap, gaps and a power cut at every mutation (`wellfed-core/tests/event_log.rs`);
  - mDNS against `simple-dns`;
  - IK against `snow`;
  - the protocol's byte fixtures (`wellfed-proto/tests/fixtures`).

## Left

- **Bank-0 anomalies after short power cuts.** Twice, bank 0's recovery role (`qa` `ef76b85e…`, from before FW47) failed within about 50 s of a cold boot that followed the bench's default 2 s power-off. Each time the next boot found the attempt marker set and skipped, as the no-retry rule requires.
  - `fw47-itest-05`: a hard fault 6 s into the clean download, an undefined instruction in the Wi-Fi driver's `cmd_thread` (`PLFLT1 cfsr=10000`). Static: the faulting PC holds a valid instruction (a `b.n` in the driver's rate-string `sprintf`), so the fetch from flash returned the wrong bytes. No KM4 task runs during a flash lease, and the SDK's RAM erase and program routines wait and flush the cache before unlocking, so no gap in the write path is known.
  - `qa-20261024-02`: a silent KM4 watchdog reset (`reset=00100000`, no `PLSUP1 stop`) just after Wi-Fi associated, before the range round's download.
  - Clean: `fw47-itest-06` (2 s off), and `qa-20261024-03` and `-04` with `--off-seconds 10`: `ota-recovery` and `ota-power-cuts` passed with no fault. The Secure faults the captures show at the cuts themselves are the CPU running as the supply decays.
  - Not explained. Leading guess (unproven): 2 s does not always give a clean power-on reset. Next: `ota-recovery` repeated at 2 s and at 10 s, to put a rate on each.
- **The bring-up firmwares** (`qa`, `optics` and the rest) serve no API and open no MQTT session, so after their install the bench has only the UART confirmation (`wellfed-bench ota` says so).
