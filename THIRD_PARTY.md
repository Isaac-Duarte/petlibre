# Third-party material and provenance

The MIT license covers Wellfed's first-party source and documentation. It does not relicense dependencies, downloaded SDKs, compiler binaries, vendor firmware or linked output.

| Material | Source and treatment |
| --- | --- |
| Rust dependencies | Cargo.lock records versions and checksums; each crate's package metadata records its license. Check the selected packages and their source notices before you distribute a combined product. |
| Realtek AmebaD SDK | [Upstream repository](https://github.com/Ameba-AIoT/ameba-rtos-d), at the revision pinned in src/firmware/vendor/sdk.lock.json. It is fetched into ignored storage; component source and binary notices stay upstream. |
| Realtek ASDK compiler | The official release URL and archive checksum are pinned in src/firmware/vendor/sdk.lock.json. It is downloaded separately; no compiler is vendored here. |
| Unicorn | The `unicorn-engine` crate (GPL-2.0), pinned in src/tools/wellfed-emu/Cargo.toml, builds the Unicorn C library and links it into the host tools that emulate vendor code (`wellfed-emu`, `wellfed-build`, and `wellfed-flash`'s tests). Those binaries are built locally and not distributed. Distributing them would carry Unicorn's license. |
| Stock firmware, loader and ROM inputs, device backups | Not distributed. Optional checks accept inputs you supply, with exact integrity restrictions where they apply. |

SDK-backed builds keep the vendor's startup code, RTOS, radio libraries, linker scripts and packaging. Before you distribute an image, review the notices of the components actually linked into it. A successful build does not grant permission to redistribute every component.

[Import provenance](docs/development/provenance.md) records the curated migration. The hardware and stock-behavior docs summarize research findings; they contain no decompiled listings or vendor code. The repository includes no device photographs or vendor artwork. The product renders under [assets/](assets/README.md) come from our own 3D model.
