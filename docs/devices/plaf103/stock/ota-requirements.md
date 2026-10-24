# Stock OTA image requirements

Verified offline on October 23, 2026, against **PLAF103 stock 1.1.15**. This describes the stock MQTT-triggered HTTP updater. The replacement firmware has a different [OTA protocol and trial policy](../../../firmware/ota-design.md).

**Static analysis:** stock can fetch a locally hosted image, but that image must fit the Realtek OTA format and the device's boot layout. The application checks an additive byte-sum before activation. Its mandatory MQTT `sha256` field does not establish cryptographic verification: no comparison against that digest was found in the traced HTTP download and activation path. Activation invalidates the previous bank's marker, so stock provides no automatic recovery from a new image that fails to boot.

The practical image contract is:

| Requirement | Evidence and scope |
| --- | --- |
| A Realtek OTA container containing an application-bank payload | Observed stock format; checked by our parser. A raw full-flash backup is not this format. U01, U04, U06 |
| Consistent lengths and a correct 32-bit sum of payload bytes | The stock updater reads back the staged payload and checks that sum. U04 |
| Compatible KM0/KM4 images, headers and load addresses | Required by the boot layout; the complete supported structural profile is checked by our tools. Passing download validation does not establish successful boot. U05, U06 |
| Payload at most **1,376,256 bytes**, sector aligned | Our conservative reviewed profile, **not a proven maximum enforced by stock**. The complete OTA file adds 32 bytes. U06 |
| A reachable plain HTTP endpoint serving the complete file | Direct `200`, explicit `Content-Length`, short URL, no compression or chunked body. U03, U04 |
| An acceptable MQTT request and device state | Model, version, connectivity, idle/power conditions, time and memory checks occur before download. U02 |

`petlibre-bench stock-ota` ([bench](../bench.md#stock-to-a-replacement-stock-ota)) applies this contract to a replacement build (FW40). **Physical (October 23, 2026):** in `fw40-stock-ota-05` stock 1.1.15 accepted a `qa` build through this updater, downloaded it in about 40 s, and the build booted from bank 0. The runs also showed that the image port must be reachable (`OTA_INFORM` error 2003 otherwise). See [consumer installation](../../../firmware/consumer-installation.md).

## Evidence identity and verification

**Static analysis:** the inspected stock OTA is 1,376,288 bytes, SHA-256 `05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498`. Its KM4 executable segment starts at file offset `0x17040`, mapped at runtime address `0x0e000020`. All function addresses below use that runtime coordinate system.

During this review, **1,649 entries in 15 exported instruction listings** were compared byte for byte with that segment. The segment was also compared with the corresponding slice of the pinned OTA. The outer header, five inner image headers, padding, byte-sum and embedded bank table were independently decoded from the binary. The URL splitter was disassembled directly with Capstone 5.0.7. These checks establish correspondence to the supplied stock bytes; they do not remove decompiler ambiguity or establish hardware reliability.

The optional ignored evidence register is `private/research/stock-updater/evidence.md`, with source paths, byte lengths and SHA-256 hashes in the adjacent `evidence-index.json`. Vendor binaries, decompiled listings, captures and device identifiers remain private. No device, phone, radio, service or routing operation was performed for this review. No new simulation was run.

## MQTT request requirements

**Static analysis, U02:** precheck function `0x0e018e30` reads these fields from the command object it receives. The enclosing service-command format is constructed in [the stock OTA trial controller](../../../../src/tools/petlibre-local/src/ota/trial.rs).

| Field | Check recovered from stock 1.1.15 |
| --- | --- |
| `upgradeType` | Required number; integer value must be 1, 2 or 3. Our reviewed requests use 1; the other values are not established installer modes. |
| `url` | Required nonempty string, at most 256 bytes in the first check; prefix must be `http://` or `https://`. The subsequent URL parser has a smaller buffer; see below. |
| `targetSoftwareVersion` | Required nonempty string parseable as three decimal integers with `%d.%d.%d`; copied into a 64-byte field with at most 63 bytes. This is not a strict semantic-version parser. |
| `sha256` | Required string, length 44 or 64, with a character-class check permitting alphanumeric characters and `+`, `/`, `=`. A normal 64-character hex digest fits. Acceptance of this field is not proof that the downloaded file is hashed. |
| `msgId` | Required nonempty string, at most 63 bytes. |
| `targetPid` | Optional string. When nonempty, must match `PLAF103`; it is copied with a 31-byte bound. The installer should always supply the exact model. |

For `upgradeType = 1`, a target numerically equal to the running `1.1.15` is refused. This branch tests equality, not whether the target is newer. The observed return from a modified `1.1.16` to original `1.1.15` confirms a downgrade in that particular cycle (U07). A `downgrade forbidden` error string elsewhere in the binary does not establish an active downgrade check here.

The precheck also requires no active OTA; a clear busy flag from `0x0e017648`; a clear suspend flag from `0x0e01219c`; acceptable results from `0x0e01e154` and `0x0e01e5e8`; and a device power-state byte other than 2. MQTT must return connected (`0x0e0124c0 == 1`), Wi-Fi must return connected (`0x0e00806c == 2`), another device-state getter must be clear (`0x0e012780 == 0`), and the time-valid getter must return 1 (`0x0e015dc8`). Reported free heap must be at least **32,768 bytes**. The exact predicates are verified; the full meanings of every opaque state value are not established by this review. A consumer procedure should require DC power and an idle, online device and retain the actual precheck error.

**Static analysis, U08:** an OTA request is not a read-only compatibility probe. Rejection can take the cleanup path that erases OTA-status sector `0x3ef000`. The precheck also updates message/state bookkeeping before checking whether OTA is already active. Device discovery should use ordinary identity/status reports, with an installer-side allowlist of tested model and source-version combinations. Do not automatically resend an uncertain OTA command.

## HTTP server and URL requirements

**Static analysis, U03–U04:** the traced task calls `0x0e02b1f4`, which opens a plain IPv4 TCP socket through `0x0e02ad84`, resolves the host and sends:

```http
GET /<resource> HTTP/1.1
Host: <host>

```

There is no vendor-domain allowlist in this traced path. A local host can serve the file. The task's name includes `https_ota` and the initial precheck accepts an HTTPS prefix, but the selected download path uses plain socket I/O; those names and prefix checks do **not** establish HTTPS/TLS support. Use the physically demonstrated plain HTTP route for this research workflow.

The initial parser `0x0e01b274` copies into a **128-byte URL buffer**, narrower than the 256-byte precheck. Its splitter `0x0e01afa4` uses small fixed buffers and tokenizes around `@`, `/` and `:`. It has a 32-byte host bound, a six-character port bound parsed with `atoi`, and a 127-byte resource copy. These are parser limits, not a promise that boundary-length inputs are well terminated or interpreted correctly. Keep the **entire ASCII URL below 128 bytes**, use an IPv4 literal and short path, omit credentials and fragments, and use an ordinary decimal port in `1..65535`. IPv6 literals are outside the reviewed path.

The request's `Host` field omits the port even when connecting to a nondefault port. The installer controls the HTTP server and should accept that request without redirecting it.

Response parser `0x0e02aa8c` recognizes **`Content-Length` or `CONTENT-LENGTH`**, with case-sensitive comparisons. It requires a nonzero declared length and recognizes status 200 or a special 302 redirect path. Lowercase `content-length`, other redirect codes, chunked transfer coding and compression are not established compatible behavior. Serve a direct `HTTP/1.1 200 OK` with `Content-Length` equal to the exact OTA file size and the unchanged binary body. Avoid cookies, login pages, content transformations and large headers. Transfer completion is driven by the image descriptor's length; do not rely on the device to reconcile every contradictory HTTP length.

## The image container

**Static analysis, U01, U04, U06:** the known single-entry profile starts with eight little-endian 32-bit words:

| File offset | Field | Known profile / original stock value |
| --- | --- | --- |
| `0x00` | Container version | `0xffffffff` |
| `0x04` | Descriptor count | `1` |
| `0x08` | Descriptor identifier | ASCII `OTA1` |
| `0x0c` | Descriptor size | `24` bytes |
| `0x10` | Payload checksum | Unsigned sum of every payload byte, modulo `2^32`; original `0x084fa320` |
| `0x14` | Payload size | File size minus 32; original `0x150000` |
| `0x18` | Payload file offset | `32` |
| `0x1c` | Destination field | `0xffffffff` |

The payload follows at byte 32. The checksum includes image headers and padding, and excludes this outer container header. Recalculate both length and checksum when constructing a different payload.

The stock descriptor parser `0x0e02a2b0` checks the first **three** identifier bytes against `OTA`, copies each 24-byte descriptor and replaces its destination with the selected bank's address. It does not provide the same exact-format guarantees as our parser. In particular, the recovered descriptor loop does not establish a safe general maximum count. Keep the reviewed single-entry profile; do not infer support for arbitrary multi-image packages, sizes or flash destinations.

**Static analysis, U06:** [our OTA parser](../../../../src/tools/petlibre-flash/src/ota.rs) deliberately admits a narrower format. It requires the entire eight-word profile above, exact payload length through EOF and the correct byte-sum. Its inner-image profile is:

| Order | Region | Runtime body address |
| --- | --- | --- |
| 1 | KM0 XIP | `0x0c000020` |
| 2 | KM0 RAM | `0x00083000` |
| 3 | KM4 XIP | `0x0e000020` |
| 4 | KM4 RAM | `0x10005000` |
| 5 | KM4 PSRAM header, empty body in this profile | `0x02000020` |

Each inner header is 32 bytes: eight-byte ASCII marker **`81958711`**, body length, load address, then four `0xffffffff` words. The KM4 group begins at a 4 KiB boundary; inter-core and final padding must be erased `0xff` bytes. Inner body lengths depend on the build; the original KM4 header position `0x17000` is not a universal fixed requirement for all candidates. The marker is a validity tag, not a cryptographic signature. The reserved words in this admitted profile do not establish security-fuse state on every unit.

The [layout auditor](../../../../src/tools/petlibre-build/src/platform/amebad/audit/layout.rs) additionally checks 4 KiB payload alignment, placement in both banks, and non-overlap with known preserved ranges. It caps the payload at the stock footprint, **`0x150000` bytes / 1.3125 MiB**, giving a complete OTA file of at most **1,376,288 bytes**. Although the bank starts are 1.5 MiB apart, the unknown partition gaps are not established free space. These conservative checks are installer requirements in our current profile; they are not all checks performed by stock itself. See [boot and flash](../flash-layout.md) and the [layout data](../../../../src/firmware/devices/plaf103/layout.json).

## Validation, activation and return to stock

**Static analysis, U04–U05:** stock selects the opposite application bank, whose mapped bases are `0x08006000` and `0x08186000`. The outer OTA header is not written into that bank. During download, `0x0e02af04` withholds the payload's first eight bytes and writes the remaining data. Function `0x0e02a580` reads the staged body back, adds the withheld bytes and compares the result with the descriptor's additive checksum. The orchestrator then calls activation `0x0e02a648`.

That activation writes the new bank's eight-byte marker, then programs over the old bank's marker with a first byte of zero. The old application's remaining bytes may still exist, but the loader will not select it by its invalidated marker. A reported download progress of 100 is therefore insufficient evidence of activation or successful boot.

**Simulation and physical observations, previously recorded, U05:** [bank policy](../../../firmware/bank-policy.md) documents the factory loader's fixed preference for valid bank 1 and the tested give-way mechanism. Failure after selecting an apparently valid bank does not trigger a general other-bank retry. Loader acceptance of known headers is separate from the candidate actually running its network and recovery paths. This review did not rerun the emulator or the bench sequence.

**Physical observations, previously recorded, U07:** the research record reports local HTTP/MQTT installation of a minimally modified stock version marker and return to original stock 1.1.15, followed by a separate small added-code stock probe and return. Those experiments establish narrow modified-stock OTA execution, not a general consumer installation matrix for wholly replacement firmware. The later [stock recovery result](../recovery.md) used UART, and the reference replacement went back over UART; it is not additional evidence of stock-to-replacement MQTT installation.

An OTA package contains application-bank material, not a complete device backup. Factory configuration, settings and external-flash identity data require a separate preservation/backup route; OTP/eFuse data is outside an ordinary external-flash image. The original application package can be a return image only while a working firmware still accepts a compatible update. Recovery from a failed first boot needs a separately demonstrated route. Patching stock to expose backup/recovery remains a proposed installation design, not something these format checks make reliable by themselves.

## Evidence register

| ID | Basis | Primary material or durable reference |
| --- | --- | --- |
| U01 | Static analysis | Pinned stock OTA and exact KM4 slice; independently decoded headers, padding and bank table |
| U02 | Static analysis | Stock precheck `0x0e018e30`, matching instruction listing |
| U03 | Static analysis | URL parser `0x0e01b274`, directly disassembled splitter `0x0e01afa4`, TCP connect `0x0e02ad84`, HTTP response parser `0x0e02aa8c` |
| U04 | Static analysis | HTTP orchestrator `0x0e02b1f4`, header receiver `0x0e02ae34`, descriptor parser `0x0e02a2b0`, writer `0x0e02af04`, byte-sum checker `0x0e02a580`; OTA task `0x0e0277d0` |
| U05 | Static analysis; earlier simulation and physical observations separately scoped | Activation `0x0e02a648`, active-bank selection `0x0e02a1d4`, [factory tests](../../../../src/tools/petlibre-emu/tests/factory.rs), [bank policy](../../../firmware/bank-policy.md) |
| U06 | Static analysis of our tooling | [Container/inner-image parser](../../../../src/tools/petlibre-flash/src/ota.rs), [layout audit](../../../../src/tools/petlibre-build/src/platform/amebad/audit/layout.rs), [layout profile](../../../../src/firmware/devices/plaf103/layout.json) |
| U07 | Earlier physical observations; not repeated | Private stock OTA trial/probe records and manifests, [UART recovery scope](../recovery.md) |
| U08 | Static analysis | Request handler `0x0e016ba0`, rejection `0x0e0193a0`, cleanup `0x0e018b48`, sector erase `0x0e0113d0` |
