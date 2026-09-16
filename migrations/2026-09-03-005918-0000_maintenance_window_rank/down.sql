DROP INDEX maintenance_windows_one_open_per_environment;
DROP INDEX maintenance_windows_one_open_per_group;

-- An environment's window has no place in a group-only model, so it ends now.
UPDATE maintenance_windows SET ended_at = NOW(), updated_at = NOW()
	WHERE ended_at IS NULL AND rank IS NOT NULL;

CREATE UNIQUE INDEX maintenance_windows_one_open_per_group
	ON maintenance_windows (server_group_id)
	WHERE ended_at IS NULL AND server_group_id IS NOT NULL;

ALTER TABLE maintenance_windows
	DROP CONSTRAINT IF EXISTS maintenance_windows_rank_check,
	DROP CONSTRAINT IF EXISTS maintenance_windows_rank_needs_group,
	DROP COLUMN rank;
