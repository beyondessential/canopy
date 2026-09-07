-- A plan is for one of a group's environments: its applications at one rank. A
-- site's clone can move ahead of its production, so each environment goes its
-- own place next.
ALTER TABLE upgrade_plans ADD COLUMN rank TEXT;

-- Existing plans were about the environment the group's headline version comes
-- from, which is the rank of its canonical member, falling back to the group's
-- highest-ranked application where it has no canonical member at all.
-- Applications still carry the older spellings of production and clone, which
-- the plan must not.
WITH ranked AS (
	SELECT
		g.id AS group_id,
		CASE lower(COALESCE(canonical.rank, highest.rank))
			WHEN 'live' THEN 'production'
			WHEN 'prod' THEN 'production'
			WHEN 'staging' THEN 'clone'
			ELSE lower(COALESCE(canonical.rank, highest.rank))
		END AS rank
	FROM server_groups g
	LEFT JOIN applications canonical ON canonical.id = g.version_application_id
	LEFT JOIN LATERAL (
		SELECT a.rank FROM applications a
		WHERE a.group_id = g.id AND a.deleted_at IS NULL AND a.rank IS NOT NULL
		ORDER BY CASE lower(a.rank)
			WHEN 'live' THEN 0
			WHEN 'prod' THEN 0
			WHEN 'production' THEN 0
			WHEN 'staging' THEN 1
			WHEN 'clone' THEN 1
			WHEN 'demo' THEN 2
			WHEN 'test' THEN 3
			ELSE 4
		END
		LIMIT 1
	) highest ON TRUE
)
UPDATE upgrade_plans p SET rank = ranked.rank
	FROM ranked WHERE ranked.group_id = p.group_id;
UPDATE upgrade_plans SET rank = 'production' WHERE rank IS NULL;

ALTER TABLE upgrade_plans
	ALTER COLUMN rank SET NOT NULL,
	ADD CONSTRAINT upgrade_plans_rank_check
		CHECK (rank IN ('production', 'clone', 'demo', 'test', 'dev'));

-- At most one open plan per environment.
DROP INDEX upgrade_plans_one_open_per_group;

CREATE UNIQUE INDEX upgrade_plans_one_open_per_environment
	ON upgrade_plans (group_id, rank)
	WHERE met_at IS NULL AND superseded_at IS NULL AND withdrawn_at IS NULL;
