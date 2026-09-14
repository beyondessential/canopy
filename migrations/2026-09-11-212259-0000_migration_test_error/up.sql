-- What the migration runner said when a migration failed: the message, and
-- the DETAIL naming the row it refused.
ALTER TABLE migration_tests ADD COLUMN error TEXT;
