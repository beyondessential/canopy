DROP INDEX incidents_environment_opened;
DROP INDEX incidents_open_by_environment;
DROP INDEX incidents_open_by_group;

-- An environment's incident has no place in a group-only model, and a group
-- can hold only one open incident once they collapse, so all but the earliest
-- open one closes.
UPDATE incidents SET closed_at = NOW(), updated_at = NOW()
	WHERE closed_at IS NULL AND server_group_id IS NOT NULL AND id NOT IN (
		SELECT DISTINCT ON (server_group_id) id FROM incidents
			WHERE closed_at IS NULL AND server_group_id IS NOT NULL
			ORDER BY server_group_id, opened_at
	);

UPDATE incident_issues SET left_at = NOW()
	WHERE left_at IS NULL AND incident_id IN (
		SELECT id FROM incidents WHERE closed_at IS NOT NULL
	);

CREATE UNIQUE INDEX incidents_open_by_group ON incidents (server_group_id)
	WHERE closed_at IS NULL;

ALTER TABLE incidents
	DROP CONSTRAINT IF EXISTS incidents_rank_check,
	DROP CONSTRAINT IF EXISTS incidents_rank_needs_group,
	DROP COLUMN rank;
