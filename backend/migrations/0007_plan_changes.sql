-- Billing period details, for working out the unused value of a membership
-- when upgrading, and whether it is already set to stop renewing.
ALTER TABLE whop_memberships ADD COLUMN current_period_start TEXT;
ALTER TABLE whop_memberships ADD COLUMN cancel_at_period_end INTEGER NOT NULL DEFAULT 0;

-- A downgrade the user chose: their paid membership stops renewing and, once
-- it ends, they move to `to_tier` (buying it, unless it's the free tier).
CREATE TABLE pending_downgrades (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    membership_id TEXT NOT NULL,
    to_tier TEXT NOT NULL,
    created_at TEXT NOT NULL
);
