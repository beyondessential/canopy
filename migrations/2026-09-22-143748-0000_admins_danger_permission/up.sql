-- The danger permission rides on the same allowlist entry as administrator:
-- presence in this table confers administrator, and this flag confers danger
-- (see the ADM spec). Existing administrators do not gain danger.
ALTER TABLE admins ADD COLUMN danger BOOLEAN NOT NULL DEFAULT FALSE;
