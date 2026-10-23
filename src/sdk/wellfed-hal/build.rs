//! Exposes the reviewed build wrapper's source identity as `WELLFED_BUILD_ID`.

fn main() {
    println!("cargo:rerun-if-env-changed=WELLFED_BUILD_ID");
    // The recovery updater's signing key (`updater::public_key`).
    println!("cargo:rerun-if-env-changed=WELLFED_OTA_PUBLIC_KEY");

    let identity = std::env::var("WELLFED_BUILD_ID").unwrap_or_else(|_| "host".into());
    assert!(
        identity == "host"
            || (identity.len() == 64
                && identity
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    );
    assert!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") || identity != "host",
        "target builds require the reviewed build wrapper's identity"
    );

    println!("cargo:rustc-env=WELLFED_BUILD_ID={identity}");
}
