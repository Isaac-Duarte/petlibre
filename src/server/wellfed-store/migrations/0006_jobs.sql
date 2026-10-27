-- Each background job's last run, for the readiness view and for the
-- schedule: a job due while the server was down runs once when it starts.

CREATE TABLE job_runs (
    job TEXT PRIMARY KEY NOT NULL,
    started_ms INTEGER NOT NULL,
    finished_ms INTEGER NOT NULL,
    outcome TEXT NOT NULL
) STRICT;
