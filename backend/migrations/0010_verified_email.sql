-- The email address the user last proved they own, and when. LLDAP lets users
-- change their own email without any check, so a paid membership is only
-- matched by email when it's this address.
ALTER TABLE users ADD COLUMN verified_email TEXT;
ALTER TABLE users ADD COLUMN email_verified_at TEXT;
