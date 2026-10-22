# Synthetic import inputs

These JSON files contain invented credentials, a synthetic feeder ID, a locally administered MAC, and an empty plan response. They demonstrate the exact importer shape and are used by configuration regression tests. They are not recordings from a device.

Run the [synthetic initialization example](../../../docs/development/local-server.md) to write a configuration under ignored `private/`. For a real feeder, supply the actual CONNECT fields and an authoritative captured plan snapshot. A fake empty snapshot does not establish that a device has no stored schedules.
