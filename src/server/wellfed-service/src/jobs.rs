//! The background jobs' bodies, and the record of each one's last run.
//! The server's supervisor decides when each runs, one at a time; each
//! body here takes the time it runs at, so a test runs it at any time.

use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_api::{Job, JobOutcome, JobRun, ReleaseCheck};
use tracing::{info, warn};

use crate::{Clock, Result, Service};

impl Service {
    /// Run `job` now, log what it did and record its run, which a failure
    /// to record does not change: the supervisor schedules on the run it
    /// returns.
    pub async fn run_job(&self, job: Job) -> JobRun {
        let started = self.shared.clock.now();
        let result = match job {
            Job::Retention => self.run_retention(started).await,
            Job::Rollups => self.run_rollups(started).await,
            Job::Expiry => self.run_expiry(started).await.map(|ended| {
                info!(job = job.code(), "ended {ended} sessions");
            }),
            Job::Backup => self.run_backup(started).await,
            Job::ReleaseCheck => self.run_release_check(started).await.map(|check| {
                info!(
                    job = job.code(),
                    "{} releases added, {} refused, {} failed to download",
                    check.added.len(),
                    check.refused.len(),
                    check.failed.len()
                );
            }),
        };

        let outcome = match result {
            Ok(()) => JobOutcome::Succeeded,
            Err(error) => {
                warn!(job = job.code(), "failed: {error}");
                JobOutcome::Failed
            }
        };
        let run = JobRun {
            job,
            started,
            finished: self.shared.clock.now(),
            outcome,
        };
        if let Err(error) = self.shared.store.record_job_run(&run).await {
            warn!(job = job.code(), "record the run: {error}");
        }
        run
    }

    /// Whether `job` runs on its schedule: the release check only while
    /// the owner allows fetching, and the backup only once it copies
    /// something, so the readiness view never shows a backup that is not
    /// there.
    pub fn job_scheduled(&self, job: Job) -> bool {
        match job {
            Job::Retention | Job::Rollups | Job::Expiry => true,
            Job::ReleaseCheck => self.fetching_releases(),
            Job::Backup => false,
        }
    }

    /// Every job's last run, in [`Job::ALL`]'s order.
    pub async fn job_runs(&self) -> Result<Vec<JobRun>> {
        Ok(self.shared.store.job_runs().await?)
    }

    /// The zone the household's daily jobs run in: the feeders' zone, or
    /// UTC before there is a household.
    pub fn household_zone(&self) -> TimeZone {
        self.mirror
            .zone()
            .map_or(TimeZone::UTC, |zone| zone.tz().clone())
    }

    /// The clock the service reads, for whatever schedules its work.
    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.shared.clock)
    }

    /// End the sessions that have idled out and those of every account
    /// past its expiry at `now`; how many. An expired account stays,
    /// disabled by its expiry, until an owner extends or deletes it.
    pub async fn run_expiry(&self, now: Timestamp) -> Result<u64> {
        let idle = now
            .checked_sub(self.session_lifetime())
            .unwrap_or(Timestamp::MIN);
        Ok(self.shared.store.delete_ended_sessions(idle, now).await?)
    }

    /// Drop the raw samples past the owner's retention. Samples carry no
    /// value to roll up yet, so there is nothing to drop that a rollup
    /// would keep: this succeeds with nothing done.
    pub async fn run_retention(&self, now: Timestamp) -> Result<()> {
        let _ = now;
        Ok(())
    }

    /// Catch the hourly rollups up to `now`. Like retention, there is
    /// nothing to roll up yet.
    pub async fn run_rollups(&self, now: Timestamp) -> Result<()> {
        let _ = now;
        Ok(())
    }

    /// Copy the database into the data directory. Not scheduled until it
    /// does ([`Self::job_scheduled`]).
    pub async fn run_backup(&self, now: Timestamp) -> Result<()> {
        let _ = now;
        Ok(())
    }

    /// Fetch and verify the project's releases the cache lacks, as of
    /// `now`. Refused while fetching is off.
    pub async fn run_release_check(&self, now: Timestamp) -> Result<ReleaseCheck> {
        Ok(self.releases.check(now).await?)
    }
}
