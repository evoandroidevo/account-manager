-- Jobs someone is waiting for (signup, a plan change, a payment) run before
-- background re-syncs from the periodic reconcile. Lower runs first.
ALTER TABLE sync_jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;

CREATE INDEX sync_jobs_priority_idx ON sync_jobs (priority, run_after);
