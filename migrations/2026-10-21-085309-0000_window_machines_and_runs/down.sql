ALTER TABLE maintenance_window_moves DROP COLUMN carried_on;

DROP INDEX maintenance_windows_lease;
ALTER TABLE maintenance_windows DROP COLUMN lease_id;

-- A window over several machines has no place before this migration.
DELETE FROM maintenance_windows WHERE cardinality(machine_ids) > 0;

DROP INDEX maintenance_windows_machine_ids;
DROP INDEX maintenance_windows_one_open_per_environment;
CREATE UNIQUE INDEX maintenance_windows_one_open_per_environment
	ON maintenance_windows (server_group_id, rank)
	WHERE ended_at IS NULL AND server_group_id IS NOT NULL AND rank IS NOT NULL;

ALTER TABLE maintenance_windows
	DROP CONSTRAINT maintenance_windows_machines_shape,
	DROP COLUMN machine_ids;
