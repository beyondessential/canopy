-- When the environment was first seen at or past the plan's target. A version
-- appearing is the upgrade starting, so the plan closes once that has stood
-- for a settle period rather than on the first report carrying it.
ALTER TABLE upgrade_plans
	ADD COLUMN target_reached_at TIMESTAMPTZ;

-- Plans already met were met on the report that closed them; the mark stands
-- where met_at does so the history reads the same.
UPDATE upgrade_plans SET target_reached_at = met_at WHERE met_at IS NOT NULL;
