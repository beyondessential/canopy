-- ── Migration tests on request ──────────────────────────────────────────────
--
-- A migrating declaration tests on a schedule while its environment has a plan
-- open (weekly, and once in the day before the upgrade), or only when an
-- operator asks, for trying a version out or a group too large to test weekly.
ALTER TABLE restore_replicas
	ADD COLUMN migrates_on_request BOOLEAN NOT NULL DEFAULT FALSE;

-- An operator asking for a machine's data to be tested against a version. Held
-- until a verdict for the pair lands, and reinstates a pair already settled
-- against the latest snapshot.
CREATE TABLE migration_test_requests (
	machine_id UUID NOT NULL REFERENCES machines (id) ON DELETE CASCADE,
	version_id UUID NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
	requested_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
	requested_by TEXT,
	PRIMARY KEY (machine_id, version_id)
);
