# Stock OTA image requirements

What an image must look like for the PLAF103's stock 1.1.15 updater to download and activate it, and what the request and HTTP server must do. Wellfed's installer relies on this to put Wellfed on a stock feeder with no soldering. Wellfed's own updates use a different [OTA protocol and trial policy](../../../firmware/ota-design.md).

The short version: stock fetches any locally hosted image that fits the Realtek OTA format and the boot layout. It checks only an additive byte-sum before activating. Its mandatory MQTT `sha256` field is never compared with the download. Activation invalidates the previous bank, so stock cannot recover by itself from a new image that fails to boot.

The IDs U01–U08 point into the [evidence register](#evidence-register) at the end.

## The contract

| Requirement | Evidence and scope |
| --- | --- |
| A Realtek OTA container holding an application-bank payload | The observed stock format, checked by our parser. A raw full-flash backup is not this format. U01, U04, U06 |
| Consistent lengths and a correct 32-bit sum of the payload bytes | The stock updater reads back the staged payload and checks that sum. U04 |
| Compatible KM0 and KM4 images, headers and load addresses | Required by the boot layout. Our tools check the complete supported structural profile. Passing download validation does not mean the image boots. U05, U06 |
| Payload at most **1,376,256 bytes**, sector aligned | Our conservative reviewed profile, **not a proven maximum enforced by stock**. The complete OTA file adds 32 bytes. U06 |
| A reachable plain HTTP endpoint serving the complete file | Direct `200`, explicit `Content-Length`, short URL, no compression or chunked body. U03, U04 |
| An acceptable MQTT request and device state | Model, version, connectivity, idle and power conditions, time and memory are checked before download. U02 |

`wellfed-bench stock-ota` ([bench](../bench.md#stock-to-a-replacement-stock-ota)) applies this contract to a Wellfed build, and stock 1.1.15 has accepted one through it. The image port must be reachable, or stock reports `OTA_INFORM` error 2003. See [consumer installation](../../../firmware/consumer-installation.md).

## The MQTT request

Precheck function `0x0e018e30` reads these fields from the command it receives (U02). Our [stock OTA trial controller](../../../../src/tools/wellfed-stock/src/ota/trial.rs) builds the enclosing service command.

| Field | Check in stock 1.1.15 |
| --- | --- |
| `upgradeType` | Required number; its integer value must be 1, 2 or 3. Our reviewed requests use 1; the other values are not established installer modes. |
| `url` | Required nonempty string, at most 256 bytes in the first check, with prefix `http://` or `https://`. The URL parser after it has a smaller buffer; see below. |
| `targetSoftwareVersion` | Required nonempty string parseable as three decimal integers with `%d.%d.%d`. Copied into a 64-byte field, at most 63 bytes. Not a strict semantic-version parser. |
| `sha256` | Required string of length 44 or 64, with a character-class check allowing alphanumerics and `+`, `/`, `=`. A normal 64-character hex digest fits. Accepting this field does not mean the downloaded file is hashed. |
| `msgId` | Required nonempty string, at most 63 bytes. |
| `targetPid` | Optional string. When nonempty it must match `PLAF103`, copied with a 31-byte bound. The installer should always send the exact model. |

**Version.** For `upgradeType = 1`, stock refuses a target numerically equal to the running `1.1.15`. It tests equality, not whether the target is newer. A return from a modified `1.1.16` to the original `1.1.15` confirmed a downgrade in that one cycle (U07). A `downgrade forbidden` string elsewhere in the binary does not show an active downgrade check here.

**Device state.** The precheck also requires:

- no active OTA;
- a clear busy flag from `0x0e017648`, and a clear suspend flag from `0x0e01219c`;
- acceptable results from `0x0e01e154` and `0x0e01e5e8`;
- a device power-state byte other than 2;
- MQTT connected (`0x0e0124c0 == 1`) and Wi-Fi connected (`0x0e00806c == 2`);
- another device-state getter clear (`0x0e012780 == 0`), and the time-valid getter returning 1 (`0x0e015dc8`);
- at least **32,768 bytes** of free heap.

The predicates are exact, but the meaning of every opaque state value is not established. An installer should require DC power and an idle, online feeder, and keep the actual precheck error.

**A request is not a probe (U08).** A rejection can take the cleanup path that erases OTA-status sector `0x3ef000`. The precheck also updates message and state bookkeeping before it checks for an active OTA. Discover devices through ordinary identity and status reports, with an installer-side allowlist of tested model and version pairs. Never resend an uncertain OTA command automatically.

## HTTP server and URL requirements

The traced task calls `0x0e02b1f4`, which opens a plain IPv4 TCP socket through `0x0e02ad84`, resolves the host and sends (U03, U04):

```http
GET /<resource> HTTP/1.1
Host: <host>

```

There is no vendor-domain allowlist on this path, so a local host can serve the file. The task's name includes `https_ota` and the first check accepts an HTTPS prefix, but the download path uses plain socket I/O. **That does not establish HTTPS or TLS support.** Use plain HTTP, the route demonstrated on hardware.

**The URL.** Parser `0x0e01b274` copies into a **128-byte URL buffer**, narrower than the 256-byte precheck. Its splitter `0x0e01afa4` uses small fixed buffers and tokenizes around `@`, `/` and `:`. It bounds the host at 32 bytes and the port at six characters (parsed with `atoi`), and copies at most 127 bytes of resource. These are parser limits, not a promise that boundary-length inputs work. So:

- keep the **whole ASCII URL under 128 bytes**;
- use an IPv4 literal and a short path;
- leave out credentials and fragments;
- use an ordinary decimal port in `1..65535`.

IPv6 literals are outside the reviewed path. The request's `Host` field omits the port even on a nondefault port. Your server should accept that request without redirecting it.

**The response.** Parser `0x0e02aa8c` recognizes **`Content-Length` or `CONTENT-LENGTH`**, compared case-sensitively. It needs a nonzero declared length and recognizes status 200 or a special 302 redirect path. Lowercase `content-length`, other redirects, chunked transfer coding and compression are not known to work. Serve a direct `HTTP/1.1 200 OK` with `Content-Length` equal to the exact file size and the unchanged binary body. Avoid cookies, login pages, content transformations and large headers. The image descriptor's length drives completion, so don't count on the device to reconcile contradictory HTTP lengths.

## The image container

The known single-entry profile starts with eight little-endian 32-bit words (U01, U04, U06):

| File offset | Field | Profile, and the original stock value |
| --- | --- | --- |
| `0x00` | Container version | `0xffffffff` |
| `0x04` | Descriptor count | `1` |
| `0x08` | Descriptor identifier | ASCII `OTA1` |
| `0x0c` | Descriptor size | `24` bytes |
| `0x10` | Payload checksum | Unsigned sum of every payload byte, modulo `2^32`; original `0x084fa320` |
| `0x14` | Payload size | File size minus 32; original `0x150000` |
| `0x18` | Payload file offset | `32` |
| `0x1c` | Destination field | `0xffffffff` |

The payload starts at byte 32. The checksum covers the image headers and padding, but not this outer header. Recalculate both length and checksum when you build a different payload.

Stock's descriptor parser `0x0e02a2b0` checks only the first **three** identifier bytes against `OTA`. It copies each 24-byte descriptor and replaces the destination with the selected bank's address. It does not give our parser's exact-format guarantees, and its descriptor loop does not establish a safe maximum count. Keep to the reviewed single-entry profile. Don't assume multi-image packages, other sizes or other flash destinations work.

[Our OTA parser](../../../../src/tools/wellfed-image/src/ota.rs) admits a deliberately narrower format (U06). It requires the whole eight-word profile above, an exact payload length through EOF and the correct byte-sum. Its inner images are:

| Order | Region | Runtime body address |
| --- | --- | --- |
| 1 | KM0 XIP | `0x0c000020` |
| 2 | KM0 RAM | `0x00083000` |
| 3 | KM4 XIP | `0x0e000020` |
| 4 | KM4 RAM | `0x10005000` |
| 5 | KM4 PSRAM header, empty body in this profile | `0x02000020` |

Each inner header is 32 bytes: the eight-byte ASCII marker **`81958711`**, the body length, the load address, then four `0xffffffff` words. The KM4 group starts on a 4 KiB boundary, and padding between and after the groups must be erased `0xff` bytes. Body lengths depend on the build; the original KM4 header position `0x17000` is not a fixed requirement. The marker is a validity tag, not a signature. The reserved words in this profile don't establish security-fuse state on every unit.

The [layout auditor](../../../../src/tools/wellfed-build/src/platform/amebad/audit/layout.rs) also checks 4 KiB payload alignment, placement in both banks and that nothing overlaps a known preserved range. It caps the payload at the stock footprint, **`0x150000` bytes (1.3125 MiB)**, so a complete OTA file is at most **1,376,288 bytes**. The banks start 1.5 MiB apart, but the unknown gaps between partitions are not established free space. These checks are requirements of our profile; stock does not perform them all. See the [flash layout](../flash-layout.md) and the [layout data](../../../../src/firmware/devices/plaf103/layout.json).

## Validation, activation and return to stock

**Download and check (U04, U05).** Stock picks the opposite application bank; the mapped bases are `0x08006000` and `0x08186000`. It does not write the outer OTA header into the bank. While downloading, `0x0e02af04` holds back the payload's first eight bytes and writes the rest. Then `0x0e02a580` reads the staged body back, adds the withheld bytes, and compares the result with the descriptor's checksum. The orchestrator then calls activation, `0x0e02a648`.

**Activation.** It writes the new bank's eight-byte marker, then programs one word over the start of the old bank's marker. Static analysis read it as a first byte of zero; on the research unit the word read back `00 00 21 01` over `38 31 39 35` (physical observation, October 29, 2026), so the other three bytes lose bits too. The old application's bytes may still be there, but the loader won't select it. A reported download progress of 100 is therefore no evidence of activation or a successful boot.

**Bank selection (U05).** The [bank policy](../../../firmware/bank-policy.md) documents the factory loader's fixed preference for a valid bank 1, and the give-way mechanism we tested. A failure after selecting an apparently valid bank does not trigger a retry from the other bank. The loader accepting known headers is separate from the candidate actually running its network and recovery code.

**What an OTA package is not.** It holds application-bank material, not a complete backup. Factory configuration, settings and flash identity data need a separate backup route, and OTP and eFuse data are outside any ordinary flash image. The original application package works as a return image only while a working firmware still accepts a compatible update. Recovering from a failed first boot needs its own demonstrated route. Patching stock to expose backup and recovery is a proposed design, not something these format checks make reliable.

## Evidence

- **Recovered, October 23, 2026:** this whole page, verified offline against PLAF103 stock 1.1.15. No device, phone, radio, service or routing operation was performed for the review, and no new simulation was run.
- **Recovered:** the inspected stock OTA is 1,376,288 bytes, SHA-256 `05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498`. Its KM4 executable segment starts at file offset `0x17040`, mapped at runtime address `0x0e000020`. All function addresses on this page use that runtime coordinate system.
- **Recovered:** 1,649 entries in 15 exported instruction listings were compared byte for byte with that segment, and the segment with the corresponding slice of the pinned OTA. The outer header, five inner image headers, padding, byte-sum and embedded bank table were decoded independently from the binary. The URL splitter was disassembled directly with Capstone 5.0.7. These checks tie the findings to the supplied stock bytes. They do not remove decompiler ambiguity or establish hardware reliability.
- **Observed, October 23, 2026:** in `fw40-stock-ota-05`, stock 1.1.15 accepted a `qa` build through this updater and downloaded it in about 40 s, and the build booted from bank 0. The same runs showed the `OTA_INFORM` error 2003 when the image port was unreachable.
- **Private register:** the optional ignored evidence register is `private/research/stock-updater/evidence.md`, with source paths, byte lengths and SHA-256 hashes in the adjacent `evidence-index.json`. Vendor binaries, decompiled listings, captures and device identifiers stay private.

### Evidence register

| ID | Basis | Primary material or durable reference |
| --- | --- | --- |
| U01 | Static analysis | Pinned stock OTA and exact KM4 slice; independently decoded headers, padding and bank table |
| U02 | Static analysis | Stock precheck `0x0e018e30`, matching instruction listing |
| U03 | Static analysis | URL parser `0x0e01b274`, directly disassembled splitter `0x0e01afa4`, TCP connect `0x0e02ad84`, HTTP response parser `0x0e02aa8c` |
| U04 | Static analysis | HTTP orchestrator `0x0e02b1f4`, header receiver `0x0e02ae34`, descriptor parser `0x0e02a2b0`, writer `0x0e02af04`, byte-sum checker `0x0e02a580`; OTA task `0x0e0277d0` |
| U05 | Static analysis; earlier simulation and physical observations, separately scoped | Activation `0x0e02a648`, active-bank selection `0x0e02a1d4`, [factory tests](../../../../src/tools/wellfed-emu/tests/factory.rs), [bank policy](../../../firmware/bank-policy.md). This review did not rerun the emulator or the bench. |
| U06 | Static analysis of our tooling | [Container and inner-image parser](../../../../src/tools/wellfed-image/src/ota.rs), [layout audit](../../../../src/tools/wellfed-build/src/platform/amebad/audit/layout.rs), [layout profile](../../../../src/firmware/devices/plaf103/layout.json) |
| U07 | Earlier physical observations, not repeated | Private stock OTA trial and probe records. Local HTTP and MQTT installed a minimally modified stock version marker and returned to the original 1.1.15, then a small added-code stock probe and return. That shows narrow modified-stock OTA execution only. The later [stock recovery](../recovery.md) used UART and is not evidence of stock-to-replacement installation over MQTT. |
| U08 | Static analysis | Request handler `0x0e016ba0`, rejection `0x0e0193a0`, cleanup `0x0e018b48`, sector erase `0x0e0113d0` |
