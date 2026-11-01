-- DESTRUCTIVE: a request names an environment, and the key it goes back to
-- cannot tell two of them apart. Cleared rather than collapsed.
DELETE FROM reporting_schema_requests;

ALTER TABLE reporting_schema_requests DROP CONSTRAINT reporting_schema_requests_pkey;
ALTER TABLE reporting_schema_requests DROP COLUMN rank;
ALTER TABLE reporting_schema_requests ADD PRIMARY KEY (group_id, version_id);

ALTER TABLE reporting_schema_builds DROP COLUMN rank;

-- DESTRUCTIVE: where a group holds a schema for more than one environment of
-- the same version, only one can survive a key that cannot tell them apart.
-- The newest is the one a key without the rank would have left standing.
DELETE FROM artifacts a
USING artifacts b
WHERE a.artifact_type = b.artifact_type
	AND a.platform = b.platform
	AND a.version_id IS NOT DISTINCT FROM b.version_id
	AND a.version_range_pattern IS NOT DISTINCT FROM b.version_range_pattern
	AND a.group_id IS NOT DISTINCT FROM b.group_id
	AND (a.created_at, a.id) < (b.created_at, b.id);

DROP INDEX artifacts_identity;

CREATE UNIQUE INDEX artifacts_identity
	ON artifacts (artifact_type, platform, version_id, version_range_pattern, group_id)
	NULLS NOT DISTINCT;

DROP INDEX artifacts_group_rank;
ALTER TABLE artifacts DROP CONSTRAINT artifact_rank_within_group;
ALTER TABLE artifacts DROP COLUMN rank;
