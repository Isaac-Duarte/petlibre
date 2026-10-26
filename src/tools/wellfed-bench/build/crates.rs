// Shared by `build.rs` and the test that holds this list to what Cargo
// resolves for `wellfed-itest`.

/// Relative to this crate. The suite is included so that a change to it
/// alone also moves the fingerprint.
const CRATES: &[&str] = &[
    ".",
    "../wellfed-itest",
    "../wellfed-relay",
    "../wellfed-formats",
    "../wellfed-stock-api",
    "../wellfed-dashboard",
    "../wellfed-build",
    "../wellfed-flash",
    "../wellfed-emu",
    "../../client/wellfed-client",
    "../../client/wellfed-model",
    "../../sdk/wellfed-core",
    "../../product/wellfed-proto",
    "../../sdk/wellfed-session",
];
