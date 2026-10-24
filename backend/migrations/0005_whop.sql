-- Whop memberships seen through webhooks or the daily reconcile. user_id is
-- NULL until the membership is matched to an account (by checkout metadata,
-- email, or an admin on /admin/payments).
CREATE TABLE whop_memberships (
    membership_id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT REFERENCES users (id) ON DELETE SET NULL,
    whop_user_id TEXT,
    email TEXT,
    plan_id TEXT NOT NULL,
    status TEXT NOT NULL,
    current_period_end TEXT,
    manage_url TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX whop_memberships_user_id_idx ON whop_memberships (user_id);

-- One row per webhook delivery id, so retried deliveries are processed once.
-- Payloads aren't stored; outcome and detail are enough to debug.
CREATE TABLE webhook_events (
    id TEXT PRIMARY KEY NOT NULL,
    event_type TEXT NOT NULL,
    received_at TEXT NOT NULL,
    membership_id TEXT,
    -- received | applied | unmatched | ignored | error
    outcome TEXT NOT NULL,
    detail TEXT
);

CREATE INDEX webhook_events_received_at_idx ON webhook_events (received_at);
