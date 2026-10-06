-- ── Migration tests on request ──────────────────────────────────────────────
--
-- A migrating declaration tests on a schedule while its environment has a plan
-- open (weekly, and once in the day before the upgrade), or only when an
-- operator asks, for trying a version out or a group too large to test weekly.
ALTER TABLE restore_replicas
	ADD COLUMN migrates_on_request BOOLEAN NOT NULL DEFAULT FALSE;

-- An operator asking for a machine's data to be tested against its
-- environment's plan. Held until a test that began after it answers it, and
-- only while the plan is the one the environment is going by: a closed plan's
-- asks match nothing.
CREATE TABLE migration_test_requests (
	machine_id UUID NOT NULL REFERENCES machines (id) ON DELETE CASCADE,
	plan_id UUID NOT NULL REFERENCES upgrade_plans (id) ON DELETE CASCADE,
	requested_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
	requested_by TEXT,
	PRIMARY KEY (machine_id, plan_id)
);

-- The fleet view reads every open plan's asks at once.
CREATE INDEX migration_test_requests_plan ON migration_test_requests (plan_id);
