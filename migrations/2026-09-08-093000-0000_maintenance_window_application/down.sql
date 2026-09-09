-- Back to a window over the box. An application's window has no target in a
-- machine-only model, and a row with neither a machine nor a group fails the
-- constraint restored below, so it goes rather than widening to the box and
-- quieting the workloads beside it that nobody declared over.
DROP INDEX maintenance_windows_application;
DROP INDEX maintenance_windows_one_open_per_application;

DELETE FROM maintenance_windows WHERE application_id IS NOT NULL;

ALTER TABLE maintenance_windows
	DROP CONSTRAINT maintenance_windows_one_target,
	DROP COLUMN application_id;
ALTER TABLE maintenance_windows
	ADD CONSTRAINT maintenance_windows_one_target
	CHECK (num_nonnulls(machine_id, server_group_id) = 1);
