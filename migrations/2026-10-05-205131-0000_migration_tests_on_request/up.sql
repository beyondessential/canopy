-- ── Migration tests on request ──────────────────────────────────────────────
--
-- A migrating declaration either tests every new snapshot while its
-- environment has a plan open, or only when an operator asks. A test costs a
-- full restore and migrate, so asking is the cheaper default; the column's own
-- default keeps a declaration that does not say on the original behaviour.
ALTER TABLE restore_replicas
	ADD COLUMN migrates_on_request BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE restore_replicas SET migrates_on_request = TRUE;

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
