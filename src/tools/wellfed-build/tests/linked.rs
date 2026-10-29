//! The linked-code audits against a real first-boot build: they reproduce
//! its manifest, and damage to the real listings and ELFs is refused.
//!
//! Optional evidence (`make check-fixtures`): these need the ignored
//! `private/firmware-build/plaf103/firstboot` build of the current sources
//! and the pinned binutils, and fail when either is missing or stale.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use wellfed_build::identity::current_inputs;
use wellfed_build::image::audit_build;
use wellfed_build::platform::amebad::audit::{self, Binutils, elf, hex, hooks, mains, park, tool};
use wellfed_build::platform::amebad::{Core, PROJECT, binutils};
use wellfed_build::{Device, Error, Firmware, HookError, MainsError, Profile, Provisioning, Repo};

struct Build {
    repo: Repo,
    dir: PathBuf,
    manifest: Value,
    binutils: Binutils,
}

fn build() -> Build {
    let repo = Repo::here().unwrap();
    let device = Device::new(&repo, "plaf103").unwrap();
    let dir = device.output(&repo, Profile::Firstboot, Provisioning::Bench);
    let manifest: Value = serde_json::from_slice(
        &fs::read(dir.join("manifest.json")).expect("completed ignored first-boot build"),
    )
    .unwrap();
    let firmware = Firmware::new(&repo, manifest["firmware"].as_str().unwrap()).unwrap();
    let inputs = current_inputs(
        &repo,
        &device,
        Profile::Firstboot,
        Provisioning::Bench,
        &firmware,
    )
    .unwrap();
    let current = serde_json::to_value(inputs).unwrap();
    assert_eq!(
        manifest["inputs"], current,
        "stale first-boot build; rebuild it before using it as evidence"
    );
    let binutils = Binutils {
        dir: binutils(&repo).unwrap(),
    };
    Build {
        repo,
        dir,
        manifest,
        binutils,
    }
}

fn sdk(build: &Build) -> PathBuf {
    build.dir.join("sdk")
}

fn listing(build: &Build, elf: &Path, address: u64, size: u64) -> String {
    tool(
        &build.binutils.objdump(),
        &[
            "-d",
            &format!("--start-address={}", hex(address)),
            &format!("--stop-address={}", hex(address + size)),
        ],
        elf,
    )
    .unwrap()
}

fn number(value: &Value) -> u64 {
    match value {
        Value::String(text) => audit::parse_hex(text).unwrap(),
        other => other.as_u64().unwrap(),
    }
}

#[test]
#[ignore = "needs the ignored first-boot build and pinned binutils"]
fn audits_reproduce_the_build_manifest() {
    let build = build();
    let audits = serde_json::to_value(audit_build(&build.repo, &build.dir).unwrap()).unwrap();
    for (key, value) in audits.as_object().unwrap() {
        assert_eq!(value, &build.manifest[key], "{key}");
    }
}

#[test]
#[ignore = "needs the ignored first-boot build and pinned binutils"]
fn parking_code_with_calls_branches_or_flash_literals_is_refused() {
    let build = build();
    for (core, image) in build.manifest["ram_park_audit"]["images"]
        .as_object()
        .unwrap()
    {
        let elf = elf(
            &sdk(&build),
            if core == "KM0" { Core::Lp } else { Core::Hp },
        );
        for (name, function) in image["functions"].as_object().unwrap() {
            let (address, size) = (number(&function["address"]), number(&function["bytes"]));
            let listing = listing(&build, &elf, address, size);
            let minimum = if name.contains("_sem_") || park::KM4_SYMBOLS.contains(&name.as_str()) {
                1
            } else {
                3
            };
            park::check_leaf(&listing, address, size, minimum).unwrap();

            for instruction in [
                "f000 f800 bl 1010b478 <ROM_call>".to_string(),
                "e000 b.n 0e000020 <XIP_target>".into(),
                "4700 bx r0".into(),
                "e8b0 8000 ldmia.w r0!, {pc}".into(),
                "4687 mov pc, r0".into(),
                format!("f8df f000 ldr.w pc, [pc, #0] ; ({address:x} <bad_jump>)"),
                "0e000020 .word 0x0e000020".into(),
                "4800 ldr r0, [pc, #0] ; (0e000020 <bad_literal>)".into(),
                "ed90 0a00 vldr s0, [r0]".into(),
                "f000 ????".into(),
            ] {
                let damaged = format!("{listing}\n {address:x}: {instruction}\n");
                assert!(
                    park::check_leaf(&damaged, address, size, minimum).is_err(),
                    "{core} {name}: {instruction}"
                );
            }
        }
    }
}

#[test]
#[ignore = "needs the ignored first-boot build and pinned binutils"]
fn hook_bypasses_in_real_listings_and_tables_are_refused() {
    let build = build();
    let objdump = build.binutils.objdump();
    for core in Core::BOTH {
        let elf = elf(&sdk(&build), core);
        let names = hooks::symbols(&elf, &objdump).unwrap();
        let listing = tool(
            &objdump,
            &["-d", "-j", ".ram_image2.text", "-j", ".xip_image2.text"],
            &elf,
        )
        .unwrap();
        hooks::legacy_routes(&listing).unwrap();

        for (_, legacy) in hooks::RENAMES {
            let address = names[legacy].address;
            let bypass = format!(
                "{listing}\n00000000 <unmediated_sdk_caller>:\n 0: f000 f800 bl {address:x} <{legacy}>\n"
            );
            let error = hooks::legacy_routes(&bypass).unwrap_err();
            assert!(
                matches!(
                    &error,
                    Error::Hooks(HookError::Bypass { caller, legacy: bypassed })
                        if caller == "unmediated_sdk_caller" && *bypassed == legacy
                ),
                "{core} {legacy}: {error}"
            );
        }

        let directory = tempfile::tempdir_in(&build.dir).unwrap();
        let at =
            hooks::file_offset(&elf, &objdump, names["ipc_init_config"].address + 28, 4).unwrap();
        let mut damaged = fs::read(&elf).unwrap();
        let legacy = (names["wellfed_sdk_flash_legacy_ipc"].address | 1) as u32;
        damaged[at..at + 4].copy_from_slice(&legacy.to_le_bytes());
        let path = directory.path().join("bypassed.axf");
        fs::write(&path, damaged).unwrap();
        let error = hooks::verify_ipc_table(&path, &objdump, &names).unwrap_err();
        assert!(
            matches!(error, Error::Hooks(HookError::IpcBypass)),
            "{core}: {error}"
        );
    }
}

/// Each core's main: (core, address, size, listing, symbols, veneers).
type Main = (
    Core,
    u64,
    u64,
    String,
    HashMap<String, u64>,
    HashMap<u64, String>,
);

fn mains(build: &Build) -> Vec<Main> {
    let objdump = build.binutils.objdump();
    Core::BOTH
        .into_iter()
        .map(|core| {
            let function = &build.manifest["firstboot_startup_audit"]["mains"][core.dir()];
            let (address, size) = (number(&function["address"]), number(&function["bytes"]));
            let elf = elf(&sdk(build), core);
            let (symbols, _) = mains::listing_inputs(&elf, &objdump).unwrap();
            let veneers = mains::expected(core)
                .iter()
                .filter_map(|name| {
                    Some((*symbols.get(&format!("__{name}_veneer"))?, name.to_string()))
                })
                .collect();
            let listing = listing(build, &elf, address, size);
            (core, address, size, listing, symbols, veneers)
        })
        .collect()
}

#[test]
#[ignore = "needs the ignored first-boot build and pinned binutils"]
fn unknown_calls_control_flow_and_rom_pointers_in_mains_are_refused() {
    let build = build();
    for (core, address, size, listing, symbols, veneers) in mains(&build) {
        mains::check_main(&listing, address, size, core, &symbols, &veneers).unwrap();

        for instruction in [
            "f000 f800 bl 12345678 <wlan_network>".to_string(),
            "4798 blx r7".into(),
            "4700 bx r0".into(),
            "4687 mov pc, r0".into(),
            "6001 str r1, [r0]".into(),
            format!("e7fe b.n {address:x} <main>"),
        ] {
            let damaged = format!(" {address:x}: {instruction}\n{listing}");
            assert!(
                mains::check_main(&damaged, address, size, core, &symbols, &veneers).is_err(),
                "{core}: {instruction}"
            );
        }

        let pointer = symbols["irq_register"];
        let changed = listing.replace(&format!(".word\t0x{pointer:08x}"), ".word\t0x12345679");
        assert_ne!(changed, listing, "{core}: irq_register literal");
        assert!(mains::check_main(&changed, address, size, core, &symbols, &veneers).is_err());
    }
}

#[test]
#[ignore = "needs the ignored first-boot build and pinned binutils"]
fn a_corrupted_veneer_target_is_refused() {
    let build = build();
    let objdump = build.binutils.objdump();
    let copy = tempfile::tempdir().unwrap();
    for core in Core::BOTH {
        let relative = format!(
            "{PROJECT}/project_{}/asdk/image/target_img2.axf",
            core.dir()
        );
        let target = copy.path().join(&relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(sdk(&build).join(&relative), &target).unwrap();
    }

    let elf = elf(copy.path(), Core::Hp);
    let (symbols, _) = mains::listing_inputs(&elf, &objdump).unwrap();
    let at =
        hooks::file_offset(&elf, &objdump, symbols["__vTaskStartScheduler_veneer"], 16).unwrap();
    let mut data = fs::read(&elf).unwrap();
    let wrong = ((symbols["vTaskStartScheduler"] | 1) ^ 4) as u32;
    data[at + 12..at + 16].copy_from_slice(&wrong.to_le_bytes());
    fs::write(&elf, data).unwrap();

    let error = mains::audit(copy.path(), &build.binutils).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Mains(MainsError::Veneer {
                name: "vTaskStartScheduler"
            })
        ),
        "{error}"
    );
}
