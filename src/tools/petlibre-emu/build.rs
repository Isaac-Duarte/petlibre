//! Unicorn's TLB uses 16-byte atomics, which clang lowers to libatomic calls
//! that the unicorn crate does not link itself.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=atomic");
    }
}
