# Stock behavior and local protocol

Findings apply to the inspected PLAF103 stock **1.1.15** image and research unit. Other models/releases may differ. Instruction-level behavior is distinguished from physical acceptance in the [hardware guide](../hardware/README.md).

## Network protocol

The observed feeder uses plaintext MQTT on TCP 1883 with device-specific credentials. Its initial CONNECT uses legacy MQTT 3.1 (`MQIsdp`); the server normalizes that CONNECT for an MQTT 3.1.1 broker and forces a clean session. It preserves the credentials, client identifier, keepalive, and later packets. Topic access stays within the configured device namespace.

Topics have the form `dl/PLAF103/<device-id>/<group>/<event>/post` or `/sub`. The local server handles startup, attributes, heartbeat, clock, binding, captured plan replies, and manual feeding. Consult the Rust protocol tests for complete synthetic wire examples. The API is described in [local-server setup](../development/local-server.md).

The broker is authenticated, has no upstream bridge, and uses nonpersistent sessions. Manual feed commands have durable caller UUIDs, QoS 0, and no retained messages. Replies must correlate to the request ID. An uncertain operation is not automatically republished. Stock firmware's limited raw-payload/result caching is not a substitute for durable no-replay handling.

## Feeding and device features

Recovered feeding code tracks feedback events, logical direction, single/double-bowl progress, direction pauses, and reverse recovery. The replacement controller uses bounded deadlines and fault stops; its policy is not a byte-for-byte reimplementation of every stock branch. Physical quantities and mechanical mapping remain uncalibrated.

Food/outlet optical filtering, power-dependent emitter duty, current estimates, battery lookup logic, four button roles, indicator selection, and audio completion paths have been investigated. A recovered branch does not establish physical signal polarity, actual timing, or complete feature support in replacement firmware.

## Clock and schedules

The server responds with the captured authoritative plan snapshot; it does not expose schedule editing or automatically push plans. Nonempty plans remain unvalidated for this release, and clock calibration is held when plans exist. Responses use the configured IANA timezone, including DST transition information. The full stock timezone/timer activation and cloud plan-construction contracts remain unresolved.

Replacement schedules still need durable occurrence identities, timezone/DST policy, restart reconciliation, and explicit handling of interrupted meals. A successful manual feed does not establish scheduled or unattended reliability.

## Reliability limits

A stock-only server-restart experiment reproduced repeated startups and a KM4 watchdog reset indication. The exact stall trigger is unresolved. WAN isolation, sustained reconnect, cold boot, time recovery, battery behavior, and portion weight require independent physical acceptance. Home Assistant integration remains deferred.
