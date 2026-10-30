//! `openapi.json` is committed, so another client's generated types build
//! without Rust and a review sees every contract change. This test fails
//! when it is stale; `UPDATE_OPENAPI=1 cargo test -p wellfed-server
//! --test openapi` rewrites it.

use std::path::PathBuf;

#[test]
fn openapi_json_is_current() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    let mut want = wellfed_server::openapi()
        .to_pretty_json()
        .expect("the document serializes");
    want.push('\n');

    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{} is stale; run UPDATE_OPENAPI=1 cargo test -p wellfed-server --test openapi",
        path.display()
    );
}
