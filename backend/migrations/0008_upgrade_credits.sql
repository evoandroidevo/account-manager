-- The promo code an upgrade's credit was issued as, one per user, current
-- membership and target plan. Checkout reuses it until it expires, so
-- repeated upgrade clicks can't mint a pile of discounts.
CREATE TABLE upgrade_credits (
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    from_membership_id TEXT NOT NULL,
    to_plan_id TEXT NOT NULL,
    promo_code TEXT NOT NULL,
    credit_cents INTEGER NOT NULL,
    expires_at TEXT NOT NULL,
    -- The membership bought with it, once the upgrade went through.
    redeemed_by_membership TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, from_membership_id, to_plan_id)
);
