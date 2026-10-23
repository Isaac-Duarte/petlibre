//! With the `ui` feature, the web app in `web/apps/bench/dist` is embedded,
//! so it must be built first. Cargo never runs pnpm itself: the Rust build
//! stays offline and needs no Node unless the page is wanted.

use std::path::Path;

fn main() {
    if std::env::var_os("CARGO_FEATURE_UI").is_none() {
        return;
    }

    let dist = Path::new("../../../web/apps/bench/dist");
    // The build's file names are content hashes; a rebuild must re-embed.
    println!("cargo::rerun-if-changed={}", dist.display());
    if !dist.join("index.html").is_file() {
        eprintln!(
            "error: the `ui` feature embeds web/apps/bench/dist, which is not built.\n\
             Run `make ui` (pnpm in web/), then build again."
        );
        std::process::exit(1);
    }
}
