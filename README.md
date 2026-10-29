# Wellfed

<img src="assets/renders/card.png" alt="The PETLIBRO Granary PLAF103 feeder, rendered from our 3D model" width="240" align="right">

Your pet's dinner shouldn't need the internet. Wellfed is open firmware for the **PETLIBRO Granary PLAF103** pet feeder. It runs the feeder on your own Wi-Fi, with no vendor cloud and no account. A command-line client talks to it today, and an optional self-hosted server and a phone app are on the way.

> [!NOTE]
> Wellfed isn't ready to look after a real pet yet. It feeds reliably on our test feeder, but schedules aren't built and it hasn't finished its long unattended run. We're an independent project, not affiliated with PETLIBRO.

## What works

- **Feeding.** Each feed stops on time, and a power cut never makes it feed twice. The feed button and the command line both work.
- **Staying local.** The feeder joins your Wi-Fi, pairs over Bluetooth and only talks to the devices you pair with it.
- **Installing.** Wellfed installs over the air from the stock firmware. No screwdriver or soldering, and you can go back to stock the same way.
- **Updating.** If a new version fails to start, the feeder goes back to the one that worked.

Schedules, the server's web page and the phone app come next. The [roadmap](docs/roadmap.md) shows where each piece stands.

## Is it for you?

It's a good fit if you own a PLAF103, you're comfortable at a command line, and you'd like to help test or build a feeder that works without the cloud.

Wait a little longer if:

- a pet depends on the feeder for its meals;
- you want a phone app;
- you have a different feeder. Only the PLAF103 is supported for now;
- you can't risk the feeder. If an install goes badly wrong, getting it back means opening it up and wiring to its board.

## Learn more

| You want to… | Read |
| --- | --- |
| See what works and what's next | [Roadmap](docs/roadmap.md) |
| Learn how installing works | [Installation](docs/firmware/consumer-installation.md) |
| Understand how it fits together | [How Wellfed works](docs/firmware/product-architecture.md) |
| Find your way around every page | [Documentation index](docs/README.md) |
| Help out | [Contributing](CONTRIBUTING.md) |

## Build it

You don't need a feeder to build and test Wellfed, and the checks never touch one. Install Rust through rustup, plus Make, CMake and a C compiler, then run:

```sh
cargo build --locked
make check
```

The [developer guide](docs/development/README.md) covers the rest: what each folder holds, which tool does what, and how to build a full firmware image.

## License

Wellfed is [MIT licensed](LICENSE). The test bench board in `src/hardware/bench/` is [CERN-OHL-P](src/hardware/bench/LICENSE), and dependencies keep their own licenses ([third-party provenance](THIRD_PARTY.md)). The repository contains no vendor firmware or device backups.

PETLIBRO and Granary are PETLIBRO's names, used here only to say which feeder Wellfed runs on.
