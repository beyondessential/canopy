-- Safety modes are not an authorisation: every administrator may raise to
-- danger, so the allowlist carries no danger flag.
ALTER TABLE admins DROP COLUMN danger;
