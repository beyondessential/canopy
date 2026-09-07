DROP INDEX upgrade_plans_one_open_per_environment;

-- A group keeps one open plan: its highest-ranked environment's, newest first.
UPDATE upgrade_plans p
	SET superseded_at = NOW()
	WHERE p.met_at IS NULL AND p.superseded_at IS NULL AND p.withdrawn_at IS NULL
	AND p.id <> (
		SELECT q.id FROM upgrade_plans q
		WHERE q.group_id = p.group_id
			AND q.met_at IS NULL AND q.superseded_at IS NULL AND q.withdrawn_at IS NULL
		ORDER BY
			CASE q.rank
				WHEN 'production' THEN 0
				WHEN 'clone' THEN 1
				WHEN 'demo' THEN 2
				WHEN 'test' THEN 3
				ELSE 4
			END,
			q.created_at DESC
		LIMIT 1
	);

CREATE UNIQUE INDEX upgrade_plans_one_open_per_group
	ON upgrade_plans (group_id)
	WHERE met_at IS NULL AND superseded_at IS NULL AND withdrawn_at IS NULL;

ALTER TABLE upgrade_plans
	DROP CONSTRAINT IF EXISTS upgrade_plans_rank_check,
	DROP COLUMN rank;
