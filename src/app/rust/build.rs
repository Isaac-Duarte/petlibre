//! flutter_rust_bridge's macros test `cfg(frb_expand)` while the codegen
//! expands the crate; declaring it keeps `unexpected_cfgs` quiet.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(frb_expand)");
}
