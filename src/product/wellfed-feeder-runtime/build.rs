//! `cfg(build_wifi)` from the builder's `WELLFED_BUILD_WIFI`, which it sets
//! exactly when the C entry's generated header names a network. Only the
//! variable's presence counts; it carries no credential.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(build_wifi)");
    println!("cargo::rerun-if-env-changed=WELLFED_BUILD_WIFI");
    if std::env::var_os("WELLFED_BUILD_WIFI").is_some() {
        println!("cargo::rustc-cfg=build_wifi");
    }
}
