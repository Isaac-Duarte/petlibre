//! Backups: `VACUUM INTO` copies of the database in the data directory's
//! `backups/`, the newest [`Backups::keep`] kept, taken by the daily job
//! and on an owner's request.
//!
//! A copy is written under a hidden partial name, synced and then renamed,
//! so the list never shows half a copy. Copies are named by the time they
//! were taken, to the second (`server-20261027T033000Z.db`); a second copy
//! in the same second gets `-2`, and so on. The sealing key is not copied:
//! the directory's README says where it is and how a restore pairs the
//! two.

use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::fmt::strtime;
use jiff::tz::TimeZone;
use tracing::{info, warn};
use wellfed_api::{Backup, BackupPage};

use crate::{Result, Service};

const PREFIX: &str = "server-";
const SUFFIX: &str = ".db";
const PARTIAL: &str = ".partial";
const STAMP: &str = "%Y%m%dT%H%M%SZ";
const README: &str = "README.txt";

/// Where backups go and how many are kept.
#[derive(Clone, Debug)]
pub struct Backups {
    /// `backups/` in the data directory.
    pub dir: PathBuf,
    pub keep: NonZeroU32,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{action} {}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the backup task stopped before it finished")]
    Task,
    #[error("{0} cannot name a backup")]
    Time(Timestamp),
}

fn io_error(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> BackupError {
    let path = path.to_owned();
    move |source| BackupError::Io {
        action,
        path,
        source,
    }
}

/// One backup file, as its name describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Name {
    taken: Timestamp,
    /// 1 for the first copy in its second, 2 for the next, and so on.
    copy: u32,
}

impl Name {
    fn file(self) -> Result<String, BackupError> {
        let stamp =
            strtime::format(STAMP, self.taken).map_err(|_| BackupError::Time(self.taken))?;
        Ok(match self.copy {
            1 => format!("{PREFIX}{stamp}{SUFFIX}"),
            copy => format!("{PREFIX}{stamp}-{copy}{SUFFIX}"),
        })
    }

    fn parse(file: &str) -> Option<Self> {
        let stem = file.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
        let (stamp, copy) = match stem.split_once('-') {
            Some((stamp, copy)) if !copy.starts_with('0') => {
                (stamp, copy.parse().ok().filter(|&copy| copy >= 2)?)
            }
            Some(_) => return None,
            None => (stem, 1),
        };
        let civil = strtime::parse(STAMP, stamp).ok()?.to_datetime().ok()?;
        let taken = civil.to_zoned(TimeZone::UTC).ok()?.timestamp();
        Some(Self { taken, copy })
    }
}

impl Service {
    /// Copy the database into `backups/` now, then drop the oldest copies
    /// past the count kept.
    pub async fn backup(&self) -> Result<Backup> {
        self.backup_at(self.shared.clock.now()).await
    }

    /// Copy the database as taken at `now`, then drop the oldest copies
    /// past the count kept. Copies are taken one at a time.
    pub(crate) async fn backup_at(&self, now: Timestamp) -> Result<Backup> {
        let _turn = self.operations.backing_up.lock().await;
        let config = self.shared.config.backups.clone();
        // Named to the second, so its time is the second it was taken in.
        let taken = Timestamp::from_second(now.as_second()).map_err(|_| BackupError::Time(now))?;

        let dir = config.dir.clone();
        let (file, partial) = blocking(move || prepare(&dir, taken)).await?;
        self.shared.store.backup_into(&partial).await?;
        let dir = config.dir.clone();
        let backup = blocking(move || finish(&dir, &file, &partial, config.keep)).await?;
        info!(
            name = backup.name,
            bytes = backup.bytes,
            "database backed up"
        );
        Ok(backup)
    }

    /// The backups kept, newest first, as one page: there are at most
    /// [`Backups::keep`] of them.
    pub async fn backups(&self) -> Result<BackupPage> {
        let dir = self.shared.config.backups.dir.clone();
        let items = blocking(move || list(&dir)).await?;
        Ok(BackupPage { items, next: None })
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, BackupError> + Send + 'static,
) -> Result<T, BackupError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| BackupError::Task)?
}

/// Make the directory and its README, and pick the copy's name; its
/// final and partial paths.
fn prepare(dir: &Path, taken: Timestamp) -> Result<(String, PathBuf), BackupError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io_error("create", dir))?;
    // The copies hold password hashes and sealed keys: the server's user
    // alone may read them, even in a directory someone else made.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(io_error("protect", dir))?;
    let readme = dir.join(README);
    if fs::read_to_string(&readme).ok().as_deref() != Some(README_TEXT) {
        fs::write(&readme, README_TEXT).map_err(io_error("write", &readme))?;
    }

    let mut name = Name { taken, copy: 1 };
    let file = loop {
        let file = name.file()?;
        if !dir
            .join(&file)
            .try_exists()
            .map_err(io_error("look for", dir))?
        {
            break file;
        }
        name.copy = name.copy.saturating_add(1);
    };
    // Copies run one at a time, so any partial copy is one a crash or a
    // failed copy left: it holds space, and would be in `VACUUM INTO`'s way.
    for entry in fs::read_dir(dir).map_err(io_error("list", dir))? {
        let entry = entry.map_err(io_error("list", dir))?;
        let left = entry.file_name();
        let left = left.to_string_lossy();
        if left.starts_with('.') && left.ends_with(PARTIAL) {
            let path = entry.path();
            fs::remove_file(&path).map_err(io_error("remove", &path))?;
        }
    }
    let partial = dir.join(format!(".{file}{PARTIAL}"));
    Ok((file, partial))
}

/// Sync the partial copy and move it into place, then prune.
fn finish(dir: &Path, file: &str, partial: &Path, keep: NonZeroU32) -> Result<Backup, BackupError> {
    let copy = fs::File::open(partial).map_err(io_error("open", partial))?;
    copy.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error("protect", partial))?;
    copy.sync_all().map_err(io_error("sync", partial))?;
    let path = dir.join(file);
    fs::rename(partial, &path).map_err(io_error("rename", partial))?;
    fs::File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(io_error("sync", dir))?;

    // The copy just written always stays, even when the clock stepped back
    // and older names now sort after it; the rest, newest first, fill the
    // remaining places.
    let (written, others): (Vec<_>, Vec<_>) = list(dir)?
        .into_iter()
        .partition(|backup| backup.name == file);
    let room = usize::try_from(keep.get() - 1).unwrap_or(usize::MAX);
    for old in others.iter().skip(room) {
        let path = dir.join(&old.name);
        if let Err(error) = fs::remove_file(&path) {
            warn!("remove the old backup {}: {error}", path.display());
        }
    }
    written.into_iter().next().ok_or_else(|| BackupError::Io {
        action: "find",
        path,
        source: io::ErrorKind::NotFound.into(),
    })
}

/// The copies in `dir`, newest first; none when it does not exist.
fn list(dir: &Path) -> Result<Vec<Backup>, BackupError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("list", dir)(error)),
    };

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error("list", dir))?;
        let Some(file) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(name) = Name::parse(&file) else {
            continue;
        };
        let bytes = entry
            .metadata()
            .map_err(io_error("read", &entry.path()))?
            .len();
        found.push((
            name,
            Backup {
                name: file,
                taken: name.taken,
                bytes,
            },
        ));
    }
    found.sort_by(|(a, _), (b, _)| b.cmp(a));
    Ok(found.into_iter().map(|(_, backup)| backup).collect())
}

/// What the backups directory says about itself.
const README_TEXT: &str = "\
These are copies of this Wellfed server's database (server.db), taken
daily at 03:30 household time and whenever an owner asks (POST
/api/v1/backups). The newest are kept, as many as backups.keep in
server.toml says (7 unless set).

The sealing key is NOT in these copies. Each copy holds the server's
feeder keys sealed with it, and they open only with the key file in the
data directory beside this one: keys/server.key (32 bytes, mode 0600).
That file never changes, so back it up once, somewhere apart from these
copies, and keep it private: with it, a copy can act as this server
toward every adopted feeder. Without it a copy still holds the history,
but the server cannot reach its feeders again until each is adopted anew.

To restore: stop the server, copy one of these files to server.db in the
data directory (deleting any server.db-wal and server.db-shm there), put
the key back at keys/server.key, and start the server. The server refuses
a server.db with no key file beside it, rather than making a new key that
could not open the stored feeder keys.
";

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn a_name_carries_its_time_and_copy() {
        let first = Name {
            taken: at("2026-10-27T03:30:00Z"),
            copy: 1,
        };
        let second = Name { copy: 2, ..first };

        assert_eq!(first.file().unwrap(), "server-20261027T033000Z.db");
        assert_eq!(second.file().unwrap(), "server-20261027T033000Z-2.db");
        assert_eq!(Name::parse("server-20261027T033000Z.db"), Some(first));
        assert_eq!(Name::parse("server-20261027T033000Z-2.db"), Some(second));
        assert!(second > first);
    }

    #[test]
    fn other_files_are_not_backups() {
        for file in [
            "README.txt",
            ".server-20261027T033000Z.db.partial",
            "server-20261027T033000Z-02.db",
            "server-20261027T033000Z-1.db",
            "server-20261027T033000Z-x.db",
            "server-2026.db",
            "server.db",
        ] {
            assert_eq!(Name::parse(file), None, "{file}");
        }
    }
}
