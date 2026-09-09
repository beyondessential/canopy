-- A window can cover one of a group's environments: the machines serving the
-- group's applications at one rank. Declaring from an environment's upgrade
-- plan suspends that environment and leaves the group's other environments,
-- and the group's own checks, watched.
ALTER TABLE maintenance_windows
	ADD COLUMN rank TEXT,
	ADD CONSTRAINT maintenance_windows_rank_needs_group
		CHECK (rank IS NULL OR server_group_id IS NOT NULL),
	ADD CONSTRAINT maintenance_windows_rank_check
		CHECK (rank IS NULL OR rank IN ('production', 'clone', 'demo', 'test', 'dev'));

-- One open window per group, and one per environment of it.
DROP INDEX maintenance_windows_one_open_per_group;
CREATE UNIQUE INDEX maintenance_windows_one_open_per_group
	ON maintenance_windows (server_group_id)
	WHERE ended_at IS NULL AND server_group_id IS NOT NULL AND rank IS NULL;
CREATE UNIQUE INDEX maintenance_windows_one_open_per_environment
	ON maintenance_windows (server_group_id, rank)
	WHERE ended_at IS NULL AND server_group_id IS NOT NULL AND rank IS NOT NULL;
