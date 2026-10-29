//! Where a running bench server can be found, whatever the caller's
//! working directory or key file: one pointer per user,
//! `$XDG_RUNTIME_DIR/wellfed-bench.json`, or
//! `~/.local/state/wellfed/wellfed-bench.json` without XDG.
//!
//! The server writes it once it listens and removes its own when it quits.
//! A client reads it before opening any device session of its own: while
//! the server it names answers, the feeder's session is the server's. A
//! pointer whose holder is gone (a killed server) names no server.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::lease::{Holder, Liveness};

/// The pointer's file name, in [`directory`].
pub const FILE: &str = "wellfed-bench.json";

/// The pointer: the server's socket, its checkout, and the process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rendezvous {
    pub socket: PathBuf,
    pub root: PathBuf,
    pub holder: Holder,
}

/// The per-user directory the pointer lives in, from the environment:
/// `XDG_RUNTIME_DIR` when it is an absolute path, else
/// `HOME/.local/state/wellfed`. `None` with neither.
pub fn directory() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    runtime.or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join(".local/state/wellfed"))
    })
}

/// The pointer in `directory` whose holder still runs.
pub fn live(directory: &Path) -> Option<Rendezvous> {
    let pointer: Rendezvous = crate::load(&directory.join(FILE)).ok()?;
    matches!(pointer.holder.liveness(), Ok(Liveness::Alive)).then_some(pointer)
}

/// Point `directory`'s file at `pointer`, replacing whatever was there.
pub fn publish(directory: &Path, pointer: &Rendezvous) -> crate::Result<()> {
    std::fs::create_dir_all(directory)?;
    crate::private::write_json(&directory.join(FILE), pointer)
}

/// Remove `directory`'s pointer if it is still `holder`'s, so a quitting
/// server never removes a later server's.
pub fn withdraw(directory: &Path, holder: &Holder) {
    let path = directory.join(FILE);
    let ours = crate::load::<Rendezvous>(&path).is_ok_and(|pointer| pointer.holder == *holder);
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_pointer_is_found_and_only_its_holder_withdraws_it() {
        let dir = tempfile::tempdir().unwrap();
        let holder = Holder::current().unwrap();
        let pointer = Rendezvous {
            socket: "/run/bench.sock".into(),
            root: "/checkout".into(),
            holder: holder.clone(),
        };
        publish(dir.path(), &pointer).unwrap();

        assert_eq!(live(dir.path()), Some(pointer));
        withdraw(dir.path(), &Holder::current().unwrap());
        assert!(live(dir.path()).is_some(), "another holder's token");
        withdraw(dir.path(), &holder);
        assert!(live(dir.path()).is_none());
    }

    #[test]
    fn a_dead_holder_s_pointer_names_no_server() {
        let dir = tempfile::tempdir().unwrap();
        let mut holder = Holder::current().unwrap();
        holder.start_ticks += 1;
        let pointer = Rendezvous {
            socket: "/run/bench.sock".into(),
            root: "/checkout".into(),
            holder,
        };
        publish(dir.path(), &pointer).unwrap();

        assert!(live(dir.path()).is_none());
    }
}
