-- ── An artifact may belong to a group ───────────────────────────────────────
--
-- An artifact belonging to no group is for every group. One that belongs to a
-- group is for that group alone, and Canopy holds its bytes rather than a
-- location, so the boundary is enforced on the read instead of resting on a
-- location being hard to guess.

ALTER TABLE artifacts
	ADD COLUMN group_id UUID REFERENCES server_groups(id) ON DELETE CASCADE,
	ADD COLUMN content BYTEA,
	ADD COLUMN content_type TEXT,
	ADD COLUMN digest TEXT,
	ADD COLUMN run_id UUID;

ALTER TABLE artifacts ALTER COLUMN download_url DROP NOT NULL;

-- An unscoped artifact rests at a location Canopy records and does not hold; a
-- group-scoped one rests in Canopy and always carries a digest, which the read
-- verifies the bytes against.
ALTER TABLE artifacts ADD CONSTRAINT artifact_rests_by_scope CHECK (
	(group_id IS NULL
		AND download_url IS NOT NULL
		AND content IS NULL
		AND content_type IS NULL)
	OR
	(group_id IS NOT NULL
		AND download_url IS NULL
		AND content IS NOT NULL
		AND digest IS NOT NULL)
);

CREATE INDEX artifacts_group_id ON artifacts (group_id);

-- ── Identity ────────────────────────────────────────────────────────────────
--
-- A registration replaces whatever is already registered for the same version
-- or range, type, platform, and group, so that tuple has to be a key to upsert
-- on. The old constraint keyed on version_id alone, which left range artifacts
-- with no uniqueness at all: version_id is NULL for every one of them, and the
-- default treatment of NULL makes those rows all distinct from each other.
-- NULLS NOT DISTINCT is what lets one index cover the exact and range shapes
-- and the grouped and ungrouped ones together.

ALTER TABLE artifacts DROP CONSTRAINT artifacts_type_platform_version_id;

CREATE UNIQUE INDEX artifacts_identity
	ON artifacts (artifact_type, platform, version_id, version_range_pattern, group_id)
	NULLS NOT DISTINCT;
