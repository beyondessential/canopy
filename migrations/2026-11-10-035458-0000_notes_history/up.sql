-- Every change to the notes of a group, machine or application, as the text
-- the notes held after it. `via` is null for the revision a record's existing
-- notes were given when this log began, which has no author either.
CREATE TABLE notes_revisions (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	server_group_id UUID REFERENCES server_groups(id) ON DELETE CASCADE,
	machine_id UUID REFERENCES machines(id) ON DELETE CASCADE,
	application_id UUID REFERENCES applications(id) ON DELETE CASCADE,
	revision BIGINT NOT NULL CHECK (revision >= 1),
	notes TEXT NOT NULL,
	author TEXT,
	via TEXT CHECK (via IN ('operator', 'ansible_import', 'token')),
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	CHECK (num_nonnulls(server_group_id, machine_id, application_id) = 1)
);

CREATE UNIQUE INDEX notes_revisions_group ON notes_revisions (server_group_id, revision)
	WHERE server_group_id IS NOT NULL;
CREATE UNIQUE INDEX notes_revisions_machine ON notes_revisions (machine_id, revision)
	WHERE machine_id IS NOT NULL;
CREATE UNIQUE INDEX notes_revisions_application ON notes_revisions (application_id, revision)
	WHERE application_id IS NOT NULL;

INSERT INTO notes_revisions (server_group_id, revision, notes, created_at)
	SELECT id, 1, notes, created_at FROM server_groups WHERE notes <> '';
INSERT INTO notes_revisions (machine_id, revision, notes, created_at)
	SELECT id, 1, notes, created_at FROM machines WHERE notes <> '';
INSERT INTO notes_revisions (application_id, revision, notes, created_at)
	SELECT id, 1, notes, created_at FROM applications WHERE notes <> '';
