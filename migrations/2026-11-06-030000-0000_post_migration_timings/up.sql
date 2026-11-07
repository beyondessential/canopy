ALTER TABLE migration_tests ADD COLUMN post_migration_elapsed INTERVAL;

CREATE TABLE post_migration_timings (
	check_id BIGINT NOT NULL REFERENCES migration_tests (check_id) ON DELETE CASCADE,
	ordinal INTEGER NOT NULL,
	name TEXT NOT NULL,
	rows BIGINT,
	elapsed INTERVAL NOT NULL,
	PRIMARY KEY (check_id, ordinal)
);
