ALTER TABLE migration_tests ADD COLUMN lookup_rebuild_elapsed INTERVAL;

CREATE TABLE lookup_rebuild_timings (
	check_id BIGINT NOT NULL REFERENCES migration_tests (check_id) ON DELETE CASCADE,
	ordinal INTEGER NOT NULL,
	name TEXT NOT NULL,
	rows BIGINT,
	elapsed INTERVAL NOT NULL,
	PRIMARY KEY (check_id, ordinal)
);
