# Contributing

Thanks for helping. You don't need a feeder for ordinary work: every check runs on the host. Start with [development setup](docs/development/README.md), then pick an area: the host tools, the portable firmware core, the AmebaD port, the server or the docs.

## Changes

- Keep each change focused, and add a regression test for any change in behavior.
- Run `cargo fmt --all` after Rust edits, and `make check` before you submit.
- Say which optional evidence or hardware checks you actually ran. A fixture you couldn't run is not a passing check.
- For docs, follow [writing docs](docs/development/writing.md).

## Hardware findings

Record the model and revision, the stock firmware version, how you observed it, how sure you are, and a redacted result. Label it with an [evidence level](docs/evidence.md).

Keep software pin assignments apart from measured electrical wiring. Describe how to reproduce the finding without needing anyone's private archive.

## Private data

Use synthetic identifiers and inputs in examples and tests. Keep real credentials, device dumps, logs, downloaded SDKs and third-party binaries in the ignored `private/` folder.

Look over your change before you share it. Ignore rules don't protect a file you add to Git on purpose.

## Safety

Preserve command identifiers, durable request and journal behavior, authentication and the recovery integrity gates. A change to feeding, flash or boot behavior needs explicit acceptance criteria and a recovery strategy that fits the board it was tested on.

## License and provenance

You submit first-party contributions under the repository's [MIT license](LICENSE). Keep third-party notices, and document the provenance of any new dependency or adapted material ([third-party provenance](THIRD_PARTY.md)). Don't redistribute vendor firmware, ROM dumps or raw device backups here.
