# Synthetic import inputs

These JSON files show the exact shape the stock server's importer reads. They are not recordings from a device. The configuration regression tests use them.

They contain invented credentials, a synthetic feeder ID, a locally administered MAC and an empty plan response.

Run the [synthetic initialization example](../../../../docs/development/stock-server.md#try-it-with-synthetic-inputs) to write a configuration under the ignored `private/`. For a real feeder, supply the actual CONNECT fields and an authoritative captured plan snapshot. A fake empty snapshot does not show that a device has no stored schedules.
