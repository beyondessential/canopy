-- Notes on issues and incidents can be written by an agent, edited and
-- deleted. An edit keeps the text it replaced, and a delete keeps the note.
ALTER TABLE issue_notes
	ADD COLUMN author_kind TEXT NOT NULL DEFAULT 'operator'
		CHECK (author_kind IN ('operator', 'agent')),
	ADD COLUMN revision BIGINT NOT NULL DEFAULT 1,
	ADD COLUMN deleted_at TIMESTAMPTZ,
	ADD COLUMN deleted_by TEXT;

ALTER TABLE incident_notes
	ADD COLUMN author_kind TEXT NOT NULL DEFAULT 'operator'
		CHECK (author_kind IN ('operator', 'agent')),
	ADD COLUMN revision BIGINT NOT NULL DEFAULT 1,
	ADD COLUMN deleted_at TIMESTAMPTZ,
	ADD COLUMN deleted_by TEXT;

-- The text a note held before an edit replaced it, with who edited it.
CREATE TABLE issue_note_revisions (
	note_id UUID NOT NULL REFERENCES issue_notes(id) ON DELETE CASCADE,
	revision BIGINT NOT NULL,
	body TEXT NOT NULL,
	edited_by TEXT NOT NULL,
	edited_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (note_id, revision)
);

CREATE TABLE incident_note_revisions (
	note_id UUID NOT NULL REFERENCES incident_notes(id) ON DELETE CASCADE,
	revision BIGINT NOT NULL,
	body TEXT NOT NULL,
	edited_by TEXT NOT NULL,
	edited_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (note_id, revision)
);
