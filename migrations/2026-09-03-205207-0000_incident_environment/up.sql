-- An incident targets one of a group's environments: the group's applications
-- at one rank. A group's own checks, and members of a group with no ranked
-- application, target the group itself, which is rank IS NULL.
ALTER TABLE incidents
	ADD COLUMN rank TEXT,
	ADD CONSTRAINT incidents_rank_needs_group
		CHECK (rank IS NULL OR server_group_id IS NOT NULL),
	ADD CONSTRAINT incidents_rank_check
		CHECK (rank IS NULL OR rank IN ('production', 'clone', 'demo', 'test', 'dev'));

-- Each incident takes the environment its members already resolve to, so the
-- monitor's startup reconcile has nothing to move: left unranked, every one of
-- them closes and reopens, notifying twice for the same live trouble. The highest
-- rank among the live members wins, and an unrecognised spelling stays on the
-- group rather than failing the deploy.
WITH member_rank AS (
	SELECT ii.incident_id,
		(ii.left_at IS NULL) AS live,
		CASE lower(a.rank)
			WHEN 'production' THEN 'production'
			WHEN 'live' THEN 'production'
			WHEN 'prod' THEN 'production'
			WHEN 'clone' THEN 'clone'
			WHEN 'staging' THEN 'clone'
			WHEN 'demo' THEN 'demo'
			WHEN 'test' THEN 'test'
			WHEN 'dev' THEN 'dev'
		END AS rank
	FROM incident_issues ii
	JOIN issues i ON i.id = ii.issue_id
	JOIN applications a
		ON a.deleted_at IS NULL
		AND (a.id = i.application_id
			OR (i.application_id IS NULL AND a.machine_id = i.machine_id))
)
UPDATE incidents SET rank = (
	SELECT m.rank FROM member_rank m
	WHERE m.incident_id = incidents.id AND m.rank IS NOT NULL
	ORDER BY m.live DESC,
		array_position(ARRAY['production', 'clone', 'demo', 'test', 'dev'], m.rank)
	LIMIT 1
)
WHERE server_group_id IS NOT NULL;

-- One open incident per group, and one per environment of it.
DROP INDEX incidents_open_by_group;
CREATE UNIQUE INDEX incidents_open_by_group ON incidents (server_group_id)
	WHERE closed_at IS NULL AND server_group_id IS NOT NULL AND rank IS NULL;
CREATE UNIQUE INDEX incidents_open_by_environment ON incidents (server_group_id, rank)
	WHERE closed_at IS NULL AND server_group_id IS NOT NULL AND rank IS NOT NULL;

CREATE INDEX incidents_environment_opened ON incidents (server_group_id, rank, opened_at DESC);
