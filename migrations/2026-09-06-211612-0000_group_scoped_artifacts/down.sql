DROP INDEX artifacts_identity;

DELETE FROM artifacts WHERE group_id IS NOT NULL;

ALTER TABLE artifacts ADD CONSTRAINT artifacts_type_platform_version_id UNIQUE (
	artifact_type, platform, version_id
);

DROP INDEX artifacts_group_id;

ALTER TABLE artifacts DROP CONSTRAINT artifact_rests_by_scope;

ALTER TABLE artifacts
	DROP COLUMN group_id,
	DROP COLUMN content,
	DROP COLUMN content_type,
	DROP COLUMN digest,
	DROP COLUMN run_id;

ALTER TABLE artifacts ALTER COLUMN download_url SET NOT NULL;
