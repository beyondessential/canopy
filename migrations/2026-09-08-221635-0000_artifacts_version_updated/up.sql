-- Whether a pair is settled asks when a version's artifacts last changed, once
-- per pair on every worklist poll. Without an index leading on version_id that
-- is a sequential scan of artifacts each time: artifacts_identity leads with
-- artifact_type, and artifacts_group_id with group_id. The partial predicate
-- matches the query, which counts the unscoped artifacts alone.
CREATE INDEX artifacts_version_updated
	ON artifacts (version_id, updated_at)
	WHERE group_id IS NULL;
