-- A lease a run holds also records what the run is: the playbook, what it is
-- limited to, the tags it runs and skips, and the application type assigned to
-- each machine with nothing on it. Further arguments are never stored, since
-- they are typed freely and can carry a secret.
--
-- A lease is pending until the run first reads its environment under it, which
-- stamps `started_at`. Canopy releases a lease that no run has read in time.
ALTER TABLE ansible_leases
	ADD COLUMN playbook TEXT,
	ADD COLUMN limit_pattern TEXT,
	-- The machines the limit matched, null where the run acts on the whole
	-- environment.
	ADD COLUMN limited_to UUID[],
	ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}',
	ADD COLUMN skip_tags TEXT[] NOT NULL DEFAULT '{}',
	-- Machine identifier to the application type it is assigned for this run.
	ADD COLUMN assignments JSONB NOT NULL DEFAULT '{}',
	ADD COLUMN started_at TIMESTAMPTZ,
	-- Why the lease stopped holding, where it has: the holder released it,
	-- another operator took it over, Canopy released it unread, or it expired.
	ADD COLUMN release_reason TEXT,
	ADD CONSTRAINT ansible_leases_release_reason CHECK (
		release_reason IS NULL
		OR release_reason IN ('released', 'taken_over', 'unstarted', 'expired')
	),
	ADD CONSTRAINT ansible_leases_reason_needs_release CHECK (
		release_reason IS NULL OR released_at IS NOT NULL
	),
	ADD CONSTRAINT ansible_leases_assignments_object CHECK (
		jsonb_typeof(assignments) = 'object'
	);

-- A lease already held was taken by a run that is under way.
UPDATE ansible_leases SET started_at = taken_at WHERE released_at IS NULL;

-- Leases already released were released by whoever the row says; the reason is
-- what can be told from it.
UPDATE ansible_leases
SET release_reason = CASE
	WHEN released_by IS NULL THEN 'expired'
	WHEN released_by IS DISTINCT FROM held_by THEN 'taken_over'
	ELSE 'released'
END
WHERE released_at IS NOT NULL;

-- The sweeps: a lease still held that has gone unread or expired.
CREATE INDEX ansible_leases_open_by_taken
	ON ansible_leases (taken_at)
	WHERE released_at IS NULL;
