-- The artifacts a build registered, of which the schema is one. An array
-- rather than a side table: the list is short, only ever read whole, and has
-- no fields of its own to carry.
ALTER TABLE reporting_schema_builds ADD COLUMN artifact_ids UUID[] NOT NULL DEFAULT '{}';

-- A pair is settled against the artifacts the version had when it was built,
-- so a build carries when it happened without a join back to its report.
ALTER TABLE reporting_schema_builds
	ADD COLUMN built_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW();
