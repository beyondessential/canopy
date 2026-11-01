-- A group-scoped artifact may be for one of the group's environments rather
-- than for all of them. A reporting schema follows the configuration of the
-- database it was built from, and a group's production, clone and demo hold
-- different configuration, so one schema per group and version is the wrong
-- one for every environment but the one it was built against.
--
-- An artifact with no rank is still for every environment in its group, which
-- is what every artifact registered before this is.

ALTER TABLE artifacts
	ADD COLUMN rank TEXT
		-- IS NOT DISTINCT FROM, so a spelling no one recognises, whose canonical
		-- form is NULL, is refused rather than passing as an unknown.
		CONSTRAINT artifacts_rank_canonical
			CHECK (rank_canonical(rank) IS NOT DISTINCT FROM rank);

-- An environment is a group's members at one rank, so a rank outside a group
-- names nothing. Without this an unscoped artifact could carry a rank and be
-- offered to that rank of every group at once.
ALTER TABLE artifacts ADD CONSTRAINT artifact_rank_within_group CHECK (
	rank IS NULL OR group_id IS NOT NULL
);

CREATE INDEX artifacts_group_rank ON artifacts (group_id, rank);

-- ── Identity ────────────────────────────────────────────────────────────────
--
-- A registration replaces what is registered for the same version or range,
-- type, platform and scope, and the scope now carries the rank. Without the
-- rank in the key, a second environment's schema for a version would replace
-- the first's instead of sitting beside it, which is the whole of what this
-- migration is for.

DROP INDEX artifacts_identity;

CREATE UNIQUE INDEX artifacts_identity
	ON artifacts (artifact_type, platform, version_id, version_range_pattern, group_id, rank)
	NULLS NOT DISTINCT;

-- ── Pairs ───────────────────────────────────────────────────────────────────
--
-- A pair is a group's environment and a version, so what is recorded against
-- one carries the environment too.

-- Nullable, and not backfilled: a build recorded before this names no
-- environment, and guessing one for it would have a schema built against some
-- other environment's configuration read as this one's. A pair looks its build
-- up by environment, so those rows match none and every environment is on the
-- worklist until it has a build of its own. The rows stay for the audit trail.
ALTER TABLE reporting_schema_builds
	ADD COLUMN rank TEXT
		CONSTRAINT reporting_schema_builds_rank_canonical
			CHECK (rank_canonical(rank) IS NOT DISTINCT FROM rank);

-- A request is an operator asking for a build that has not happened yet, so
-- there is no history in one. Cleared rather than backfilled, since the key
-- gains a column that cannot be null and an operator asking again is one click.
DELETE FROM reporting_schema_requests;

ALTER TABLE reporting_schema_requests
	DROP CONSTRAINT reporting_schema_requests_pkey;

ALTER TABLE reporting_schema_requests
	ADD COLUMN rank TEXT NOT NULL
		CONSTRAINT reporting_schema_requests_rank_canonical
			CHECK (rank_canonical(rank) IS NOT DISTINCT FROM rank);

ALTER TABLE reporting_schema_requests
	ADD PRIMARY KEY (group_id, rank, version_id);
