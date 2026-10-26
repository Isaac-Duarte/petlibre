//! `private/bench.lease`: the one claim on the bench that every tool driving
//! the relays, the feeder's serial port, the hotspot, an over-the-air install
//! or a flash write takes before it starts.
//!
//! A lease names its holder (process, its start time, the boot and host it
//! ran on, its command line and when it started), so a second tool can say
//! who has the bench rather than fail on a busy port halfway through a
//! sequence. It is never taken over silently: a lease whose process is gone
//! means a run was killed and may have left relays on or an update half
//! done, so the next tool refuses and names it, and only an explicit
//! [`break_stale`] (after the operator has looked at the bench) frees it.
//!
//! Liveness is the process, not just its pid: the kernel reuses pids, so a
//! holder is alive only while `/proc/<pid>/stat` shows the same start time
//! on the same boot. A process descended from the holder (the bench's
//! `wellfed-flash` children) runs on the holder's lease: its claim is
//! [`Claim::Inherited`] and it removes nothing when it ends, so a chained
//! tool never deadlocks on its parent.
//!
//! Taking, breaking and releasing the lease happen under an advisory lock on
//! a separate guard file, so two tools racing for a free or stale lease
//! cannot both win, and a breaker cannot remove a lease another tool has
//! just taken. The lease itself is written atomically (temporary file and
//! rename), so a reader that takes no lock (`wellfed-bench status`) never
//! sees it torn.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::private::{SYSTEM_RANDOM, random, write_json};
use crate::require;

/// Root-relative: the lease file.
pub const LEASE: &str = "private/bench.lease";
/// Root-relative: the guard every change to the lease is made under. It
/// stays behind empty; only its advisory lock matters.
const GUARD: &str = "private/bench.lease.lock";

/// How far up the process tree a holder is looked for. A bench chain is
/// two or three processes deep; the bound only ends a walk that /proc
/// changes under.
const ANCESTRY: usize = 64;

/// The process holding the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    /// When the process started, in clock ticks after boot
    /// (`/proc/<pid>/stat` field 22). With `boot`, it tells the holder from
    /// a later process given the same pid.
    pub start_ticks: u64,
    /// `/proc/sys/kernel/random/boot_id` of the boot the holder ran on.
    pub boot: String,
    pub host: String,
    /// The holder's command line, as the operator would recognise it.
    pub command: Vec<String>,
    pub started: DateTime<Utc>,
    /// Ties a release to the lease it took: a tool removes the lease only
    /// while it still carries its own token.
    pub token: String,
}

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pid {} (`{}`, since {} on {})",
            self.pid,
            self.command.join(" "),
            self.started.format("%Y-%m-%d %H:%M:%S UTC"),
            self.host
        )
    }
}

/// Whether a recorded holder is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The same process (pid and start time, this boot) still exists.
    Alive,
    /// It exited, was killed, or the host rebooted since.
    Gone,
    /// It ran on another host, whose processes this one cannot see.
    Unknown,
}

/// How this process holds the bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// It took the lease and removes it when dropped.
    Owned,
    /// An ancestor holds it; this process runs as part of that run.
    Inherited,
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Another live process holds the bench.
    #[error("The bench is held by {0}; wait for it to finish or stop it")]
    Held(Box<Holder>),
    /// The holder is gone without releasing the bench: it was killed.
    #[error(
        "The bench lease of {0} outlived its process: that run was killed and may have left \
         relays on or an update half done. Check `wellfed-bench status`, then free the bench \
         with `wellfed-bench status --break-lease`"
    )]
    Stale(Box<Holder>),
    /// The holder ran on another host; nothing here can tell if it is alive.
    #[error(
        "The bench is held by {0}, on another host; if that run is over, remove {LEASE} by hand"
    )]
    Elsewhere(Box<Holder>),
    /// A break was asked for, but the holder still runs.
    #[error("The bench lease of {0} is live; it is not broken")]
    Live(Box<Holder>),
    /// A break was asked for, but nothing holds the bench.
    #[error("No bench lease to break")]
    Free,
    /// The lease file exists but is not a lease; nothing can say who holds it.
    #[error("{}: not a bench lease ({source}); remove it by hand once the bench is idle", path.display())]
    Unreadable { path: PathBuf, source: crate::Error },
    /// This process's own identity could not be read from /proc.
    #[error("{}: no process identity in it", path.display())]
    Identity { path: PathBuf },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Formats(#[from] crate::Error),
}

pub type Result<T, E = LeaseError> = std::result::Result<T, E>;

/// The bench, held by this process until dropped.
#[derive(Debug)]
pub struct Lease {
    root: PathBuf,
    holder: Holder,
    claim: Claim,
}

impl Lease {
    /// Take the bench under `root`, or join the run of an ancestor that
    /// holds it. Refuses a live holder, a stale lease and an unreadable one.
    pub fn acquire(root: &Path) -> Result<Self> {
        let me = Holder::current()?;
        let _guard = Guard::take(root)?;

        let path = root.join(LEASE);
        let Some(holder) = read(&path)? else {
            write_json(&path, &me)?;
            return Ok(Self {
                root: root.to_path_buf(),
                holder: me,
                claim: Claim::Owned,
            });
        };

        if descends_from(me.pid, &holder)? {
            return Ok(Self {
                root: root.to_path_buf(),
                holder,
                claim: Claim::Inherited,
            });
        }
        match holder.liveness()? {
            Liveness::Alive => Err(LeaseError::Held(Box::new(holder))),
            Liveness::Gone => Err(LeaseError::Stale(Box::new(holder))),
            Liveness::Unknown => Err(LeaseError::Elsewhere(Box::new(holder))),
        }
    }

    /// The run's holder: this process, or the ancestor it inherited from.
    pub fn holder(&self) -> &Holder {
        &self.holder
    }

    pub fn claim(&self) -> Claim {
        self.claim
    }

    /// Remove the lease if it is still this process's own. Best effort: a
    /// lease left behind reads as stale next time, which the operator
    /// breaks after a look at the bench.
    fn release(&self) -> Result<()> {
        let _guard = Guard::take(&self.root)?;
        let path = self.root.join(LEASE);
        if read(&path)?.is_some_and(|held| held.token == self.holder.token) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.claim == Claim::Owned {
            let _ = self.release();
        }
    }
}

/// The lease under `root` and whether its holder still runs, without
/// taking it: for a read-only report such as `status`.
pub fn inspect(root: &Path) -> Result<Option<(Holder, Liveness)>> {
    let Some(holder) = read(&root.join(LEASE))? else {
        return Ok(None);
    };
    let liveness = holder.liveness()?;
    Ok(Some((holder, liveness)))
}

/// Free a bench whose holder is gone, returning who held it. Refuses a live
/// holder, one on another host, and an unreadable lease, which are the
/// operator's to look at.
pub fn break_stale(root: &Path) -> Result<Holder> {
    let _guard = Guard::take(root)?;

    let path = root.join(LEASE);
    let holder = read(&path)?.ok_or(LeaseError::Free)?;
    match holder.liveness()? {
        Liveness::Gone => {}
        Liveness::Alive => return Err(LeaseError::Live(Box::new(holder))),
        Liveness::Unknown => return Err(LeaseError::Elsewhere(Box::new(holder))),
    }

    fs::remove_file(&path)?;
    Ok(holder)
}

impl Holder {
    /// This process, with a fresh token.
    pub fn current() -> Result<Self> {
        let command = std::env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        Self::of(std::process::id(), command)
    }

    /// The running process `pid`, recorded as running `command`. For a
    /// lease another process is to be found holding, as tests set up.
    pub fn of(pid: u32, command: Vec<String>) -> Result<Self> {
        let path = stat_path(pid);
        let stat = Stat::read(pid)?.ok_or(LeaseError::Identity { path })?;
        let token = crate::hex::encode(&random::<16>(Path::new(SYSTEM_RANDOM))?);

        Ok(Self {
            pid,
            start_ticks: stat.start_ticks,
            boot: boot_id()?,
            host: host()?,
            command,
            started: DateTime::<Utc>::from(std::time::SystemTime::now()),
            token,
        })
    }

    /// Whether this holder still runs: the same pid with the same start
    /// time, on this boot of this host. A zombie has finished.
    pub fn liveness(&self) -> Result<Liveness> {
        if self.host != host()? {
            return Ok(Liveness::Unknown);
        }
        if self.boot != boot_id()? {
            return Ok(Liveness::Gone);
        }

        let alive = Stat::read(self.pid)?
            .is_some_and(|stat| stat.start_ticks == self.start_ticks && stat.state != 'Z');
        Ok(if alive {
            Liveness::Alive
        } else {
            Liveness::Gone
        })
    }
}

/// Whether `pid`, or a process it descends from, is `holder`, alive on this
/// host and boot. That process is part of the holder's run.
fn descends_from(pid: u32, holder: &Holder) -> Result<bool> {
    if holder.host != host()? || holder.boot != boot_id()? {
        return Ok(false);
    }

    let mut next = pid;
    for _ in 0..ANCESTRY {
        let Some(stat) = Stat::read(next)? else {
            return Ok(false);
        };
        if next == holder.pid {
            return Ok(stat.start_ticks == holder.start_ticks);
        }
        if stat.ppid == 0 {
            return Ok(false);
        }
        next = stat.ppid;
    }
    Ok(false)
}

/// The lease at `path`, if there is one.
fn read(path: &Path) -> Result<Option<Holder>> {
    match crate::load(path) {
        Ok(holder) => Ok(Some(holder)),
        Err(crate::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LeaseError::Unreadable {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The advisory lock on [`GUARD`], released when dropped (or when the
/// process dies, so a killed tool never leaves the guard held).
struct Guard {
    _locked: fs::File,
}

impl Guard {
    fn take(root: &Path) -> Result<Self> {
        let path = root.join(GUARD);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        file.lock()?;
        Ok(Self { _locked: file })
    }
}

/// The fields of `/proc/<pid>/stat` a lease needs.
#[derive(Debug, PartialEq, Eq)]
struct Stat {
    state: char,
    ppid: u32,
    start_ticks: u64,
}

fn stat_path(pid: u32) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/stat"))
}

impl Stat {
    /// `pid`'s stat, or `None` once the process is gone.
    fn read(pid: u32) -> Result<Option<Self>> {
        let path = stat_path(pid);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            // ESRCH: the process went away while it was being read.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(3) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Self::parse(&text)
            .map(Some)
            .ok_or(LeaseError::Identity { path })
    }

    /// The command name (field 2) is in parentheses and may itself hold
    /// spaces and parentheses, so fields are counted after the last `)`:
    /// state is field 3, the parent field 4 and the start time field 22.
    fn parse(text: &str) -> Option<Self> {
        let (_, rest) = text.rsplit_once(')')?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        Some(Self {
            state: fields.first()?.chars().next()?,
            ppid: fields.get(1)?.parse().ok()?,
            start_ticks: fields.get(19)?.parse().ok()?,
        })
    }
}

fn boot_id() -> Result<String> {
    proc_text("/proc/sys/kernel/random/boot_id")
}

fn host() -> Result<String> {
    proc_text("/proc/sys/kernel/hostname")
}

fn proc_text(path: &str) -> Result<String> {
    let text = fs::read_to_string(path)?;
    let text = text.trim().to_string();
    require(!text.is_empty(), || LeaseError::Identity {
        path: path.into(),
    })?;
    Ok(text)
}

#[cfg(test)]
mod tests;
