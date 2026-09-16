DROP INDEX maintenance_windows_application;
DROP INDEX maintenance_windows_one_open_per_application;

ALTER TABLE maintenance_windows
	DROP CONSTRAINT maintenance_windows_one_target;

-- An application's window has no place in a machine-only model, so it moves to
-- the application's box and ends now.
UPDATE maintenance_windows w
SET machine_id = a.machine_id,
	ended_at = coalesce(w.ended_at, NOW()),
	updated_at = CASE WHEN w.ended_at IS NULL THEN NOW() ELSE w.updated_at END
FROM applications a
WHERE a.id = w.application_id;

ALTER TABLE maintenance_windows
	DROP COLUMN application_id;
ALTER TABLE maintenance_windows
	ADD CONSTRAINT maintenance_windows_one_target
	CHECK (num_nonnulls(machine_id, server_group_id) = 1);
