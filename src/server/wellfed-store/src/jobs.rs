//! The background jobs' last runs: one row per job, replaced by each run.

use wellfed_api::{Job, JobOutcome, JobRun};

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

impl Store {
    /// Keep `run` as its job's last run.
    pub async fn record_job_run(&self, run: &JobRun) -> Result<()> {
        let job = run.job.code();
        let started = millis(run.started);
        let finished = millis(run.finished);
        let outcome = run.outcome.code();
        sqlx::query!(
            "INSERT INTO job_runs (job, started_ms, finished_ms, outcome) VALUES (?, ?, ?, ?)
             ON CONFLICT (job) DO UPDATE SET
                 started_ms = excluded.started_ms,
                 finished_ms = excluded.finished_ms,
                 outcome = excluded.outcome",
            job,
            started,
            finished,
            outcome,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Every job's last run, in [`Job::ALL`]'s order; a job that has never
    /// run has none.
    pub async fn job_runs(&self) -> Result<Vec<JobRun>> {
        let rows = sqlx::query!("SELECT job, started_ms, finished_ms, outcome FROM job_runs")
            .fetch_all(&self.reader)
            .await?;

        let mut runs = rows
            .into_iter()
            .map(|row| {
                Ok(JobRun {
                    job: Job::from_code(&row.job).ok_or(Error::Corrupt("job"))?,
                    started: timestamp(row.started_ms)?,
                    finished: timestamp(row.finished_ms)?,
                    outcome: JobOutcome::from_code(&row.outcome)
                        .ok_or(Error::Corrupt("job outcome"))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        runs.sort_by_key(|run| Job::ALL.iter().position(|job| *job == run.job));
        Ok(runs)
    }
}
