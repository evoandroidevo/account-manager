-- How many promo codes an upgrade's credit has been issued as. Whop can't tie
-- a code to a buyer, so this caps how many can be handed on to someone else.
ALTER TABLE upgrade_credits ADD COLUMN issued_count INTEGER NOT NULL DEFAULT 1;
