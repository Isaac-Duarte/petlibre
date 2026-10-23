# Third-party material and provenance

The MIT license covers Wellfed's first-party source and documentation. It does not relicense dependencies, downloaded SDKs, compiler binaries, vendor firmware, or linked output.

| Material | Source and treatment |
| --- | --- |
| Rust dependencies | Versions/checksums are recorded in Cargo.lock; package metadata records each crate's license. Inspect the selected packages and their source notices before distributing a combined product. |
| Realtek AmebaD SDK | [Upstream repository](https://github.com/Ameba-AIoT/ameba-rtos-d), revision pinned in src/firmware/vendor/sdk.lock.json. Fetched into ignored storage; component source and binary notices remain upstream. |
| Realtek ASDK compiler | Official release URL and archive checksum are pinned in src/firmware/vendor/sdk.lock.json. Downloaded separately; no compiler is vendored here. |
| Unicorn and Capstone | Exact Python versions are pinned in src/tools/requirements-probe.txt. These packages retain their own licenses and notices. |
| pyserial | Optional serial dependency pinned in src/tools/requirements-hardware.txt; retains its own license. |
| Stock firmware, loader/ROM inputs, device backups | Not distributed. Optional checks accept contributor-supplied inputs, with exact integrity restrictions where applicable. |

SDK-backed builds retain vendor startup, RTOS, radio libraries, linker scripts, and packaging. Review the notices of the actual components linked into an image before distributing it; a successful build does not establish redistribution permission for every component.

[Import provenance](docs/development/provenance.md) records the curated migration. Hardware and stock-behavior documentation summarizes research findings; it does not contain decompiled listings or vendor code. No device photographs or vendor artwork are included in this initial repository.
