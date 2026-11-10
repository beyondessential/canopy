DROP TABLE incident_note_revisions;
DROP TABLE issue_note_revisions;

DELETE FROM incident_notes WHERE deleted_at IS NOT NULL;
DELETE FROM issue_notes WHERE deleted_at IS NOT NULL;

ALTER TABLE incident_notes
	DROP COLUMN deleted_by,
	DROP COLUMN deleted_at,
	DROP COLUMN revision,
	DROP COLUMN author_kind;

ALTER TABLE issue_notes
	DROP COLUMN deleted_by,
	DROP COLUMN deleted_at,
	DROP COLUMN revision,
	DROP COLUMN author_kind;
