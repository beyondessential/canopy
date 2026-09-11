-- What the migration runner said when a migration failed, sanitised by the
-- consumer before it is sent.
ALTER TABLE migration_tests ADD COLUMN error TEXT;
