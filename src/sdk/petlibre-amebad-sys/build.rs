//! Generates AmebaD SDK bindings for target builds with the `sdk` feature.
//!
//! Only KM4 target builds run bindgen; host and KM0 builds generate nothing.
//! The function and type allowlists keep the generated surface to exactly
//! what the ports call, so a new SDK use has to be added here deliberately.

fn main() {
    println!("cargo:rerun-if-changed=sdk_wrapper.h");
    println!("cargo:rerun-if-env-changed=PETLIBRE_SDK_DIR");
    println!("cargo:rerun-if-env-changed=PETLIBRE_NEWLIB_INCLUDE");
    #[cfg(feature = "sdk")]
    sdk::generate();
}

#[cfg(feature = "sdk")]
mod sdk {
    use std::env;
    use std::path::PathBuf;

    /// Function and type allowlist: exactly the SDK surface the ports use.
    const FUNCTIONS: &[&str] = &[
        "GPIO_PortWrite",
        "GPIO_PortRead",
        "GPIO_Init",
        "GPIO_WriteBit",
        "GPIO_ReadDataBit",
        "GPIO_INTConfig",
        "GPIO_INTMode",
        "EFUSERead8",
        "PAD_CMD",
        "PAD_PullCtrl",
        "ADC_StructInit",
        "ADC_Init",
        "ADC_Cmd",
        "ADC_ClearFIFO",
        "ADC_SWTrigCmd",
        "ADC_Readable",
        "ADC_Read",
        "SYSTIMER_TickGet",
        "RCC_PeriphClockCmd",
        "RTIM_TimeBaseStructInit",
        "RTIM_TimeBaseInit",
        "RTIM_INTConfig",
        "RTIM_Cmd",
        "RTIM_INTClear",
        "irq_register",
        "irq_unregister",
        "irq_enable",
        "irq_disable",
        "pmu_acquire_wakelock",
        "pmu_release_wakelock",
        "WDG_Init",
        "WDG_Cmd",
        "WDG_Refresh",
        "xTaskCreate",
        "vTaskDelay",
        "vTaskDelete",
        "_rtl_printf",
        "vTaskSuspendAll",
        "xTaskResumeAll",
        "xTaskGetSchedulerState",
        "xTaskGetTickCount",
        "xTaskGetCurrentTaskHandle",
        "xPortGetFreeHeapSize",
        "xPortGetMinimumEverFreeHeapSize",
        "BOOT_Reason",
        "BKUP_Read",
        "FLASH_RxCmdXIP",
        "FLASH_EraseXIP",
        "FLASH_TxData256BXIP",
        "BKUP_Write",
    ];

    const TYPES: &[&str] = &[
        "GPIO_InitTypeDef",
        "GPIO_TypeDef",
        "ADC_InitTypeDef",
        "ADC_TypeDef",
        "RTIM_TypeDef",
        "RTIM_TimeBaseInitTypeDef",
        "WDG_InitTypeDef",
        "WDG_TypeDef",
        "IRQn_Type",
        "IRQ_FUN",
        "RSIP_REG_TypeDef",
        "PINMUX_TypeDef",
    ];

    pub fn generate() {
        // KM0 runs only the SDK-independent flash-park and hook routines.
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none")
            || env::var("CARGO_FEATURE_KM4").is_err()
        {
            return;
        }

        let sdk = PathBuf::from(
            env::var("PETLIBRE_SDK_DIR").expect("PETLIBRE_SDK_DIR names the staged SDK"),
        );
        let newlib = env::var("PETLIBRE_NEWLIB_INCLUDE")
            .expect("PETLIBRE_NEWLIB_INCLUDE names newlib headers");
        let km4 = env::var("CARGO_FEATURE_KM4").is_ok();
        assert!(
            km4 != env::var("CARGO_FEATURE_KM0").is_ok(),
            "select exactly one of km0/km4"
        );
        let (core, port) = if km4 {
            ("hp", "RTL8721D_HP/non_secure")
        } else {
            ("lp", "RTL8721D_LP")
        };

        let soc = sdk.join("component/soc/realtek/amebad");
        let rtos = sdk.join("component/os/freertos/freertos_v10.2.0/Source");
        let includes = [
            sdk.join(format!("project/realtek_amebaD_va0_example/inc/inc_{core}")),
            sdk.join("component/os"),
            sdk.join("component/os/freertos"),
            rtos.join("include"),
            rtos.join("portable/GCC").join(port),
            soc.join("cmsis"),
            soc.join("cmsis/device"),
            soc.join("fwlib/include"),
            soc.join(format!("fwlib/rom_map_{core}")),
            soc.join("app/monitor/include"),
            soc.join("app/xmodem"),
            soc.join("swlib/include"),
            soc.join("swlib/string"),
            soc.join("misc"),
            soc.join("swlib/os_dep/include"),
            sdk.join("component/common/api"),
            sdk.join("component/common/api/platform"),
            sdk.join("component/os/os_dep/include"),
            sdk.join("component/common/network/ssl/mbedtls-2.4.0/include"),
            sdk.join("component/common/network/ssl/ssl_ram_map/rom"),
        ];

        // Layouts only: parse with a soft-float triple so CMSIS FPU asm is not
        // instantiated; no binding takes or returns floating-point values.
        let mut args: Vec<String> = vec![
            if km4 {
                "--target=thumbv8m.main-none-eabi"
            } else {
                "--target=thumbv8m.base-none-eabi"
            }
            .into(),
            "-ffreestanding".into(),
            "-mcmse".into(),
            "-DCONFIG_PLATFORM_8721D".into(),
            if km4 {
                "-DARM_CORE_CM4"
            } else {
                "-DARM_CORE_CM0"
            }
            .into(),
            "-isystem".into(),
            newlib,
        ];

        if km4 {
            args.push("-mfpu=none".into());
        } else {
            args.push("-D__ARM_FEATURE_DSP=0".into());
        }
        args.extend(includes.iter().map(|path| format!("-I{}", path.display())));

        let mut builder = bindgen::Builder::default()
            .header("sdk_wrapper.h")
            .clang_args(args)
            .use_core()
            .ctypes_prefix("core::ffi")
            .layout_tests(false)
            .derive_default(true)
            .allowlist_type("petlibre_sdk_constant|IRQn")
            .constified_enum_module("petlibre_sdk_constant")
            .prepend_enum_name(false)
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

        for name in FUNCTIONS {
            builder = builder.allowlist_function(name);
        }
        for name in TYPES {
            builder = builder.allowlist_type(name);
        }

        let bindings = builder.generate().expect("generate AmebaD SDK bindings");
        let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("sdk.rs");
        bindings.write_to_file(out).expect("write SDK bindings");
    }
}
