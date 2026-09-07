-- A reporting-schema build's result, hanging off the restore-health report that
-- carries its common fields, the way a migration test's does. Its own table
-- rather than nullable columns on the report, because the pair a build is for
-- is the whole point of it and a plain restore report has nothing to put there.
--
-- A row here is what settles a pair: a build that failed settles it as firmly
-- as one that produced a schema, since a build against a fixed version and
-- configuration fails the same way every time.
CREATE TABLE reporting_schema_builds (
	check_id BIGINT PRIMARY KEY REFERENCES backup_restore_checks (id) ON DELETE CASCADE,
	group_id UUID NOT NULL REFERENCES server_groups (id) ON DELETE CASCADE,
	version_id UUID NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
	application_id UUID REFERENCES applications (id) ON DELETE SET NULL,
	built BOOLEAN NOT NULL,
	error TEXT
);

-- Whether a pair is settled is the question the worklist asks on every pass.
CREATE INDEX reporting_schema_builds_pair ON reporting_schema_builds (group_id, version_id);

-- An operator asking for a pair's build, which is how a schema is refreshed
-- after the group's configuration changes and how a settled pair is reinstated.
-- Keyed on the pair rather than the machine, because the pair is what is built.
CREATE TABLE reporting_schema_requests (
	group_id UUID NOT NULL REFERENCES server_groups (id) ON DELETE CASCADE,
	version_id UUID NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
	requested_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
	requested_by TEXT,
	PRIMARY KEY (group_id, version_id)
);
