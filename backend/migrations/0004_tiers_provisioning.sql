-- Tier assignments other than the configured default. One row per source,
-- so a manual override and (later) a paid Whop membership can coexist; the
-- effective tier is manual, then whop, then the default.
CREATE TABLE entitlements (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    tier TEXT NOT NULL,
    -- NULL means no expiry.
    valid_until TEXT,
    note TEXT,
    updated_at TEXT NOT NULL,
    updated_by TEXT,
    PRIMARY KEY (user_id, source)
);

-- The state of each user's account in each managed service
-- (lldap groups, jellyfin, seerr).
CREATE TABLE service_accounts (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    service TEXT NOT NULL,
    -- The account's id in that service, once it exists.
    remote_id TEXT,
    -- Tier whose settings were last applied successfully.
    applied_tier TEXT,
    -- pending | ok | error | skipped
    state TEXT NOT NULL,
    detail TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, service)
);

-- Work queue: bring one user's account in one service in line with their
-- effective tier. At most one pending job per user and service.
CREATE TABLE sync_jobs (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    service TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    run_after TEXT NOT NULL,
    last_error TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, service)
);

CREATE INDEX sync_jobs_run_after_idx ON sync_jobs (run_after);
