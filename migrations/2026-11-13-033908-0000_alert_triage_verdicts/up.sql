-- Alert triage verdicts: what an agent decided about an incident notification,
-- and the incident's state at the revision it read, which is the baseline the
-- next triage of the same incident compares against. The verdict is kept apart
-- from the note that shows it, so editing or deleting the note leaves it.
CREATE TABLE alert_triage_verdicts (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	incident_id UUID NOT NULL REFERENCES incidents(id) ON DELETE CASCADE,
	verdict TEXT NOT NULL CHECK (verdict IN ('escalated', 'logged')),
	reasoning TEXT NOT NULL CHECK (char_length(reasoning) BETWEEN 1 AND 4000),
	revision TEXT NOT NULL,
	open BOOLEAN NOT NULL,
	worst_result TEXT CHECK (worst_result IN ('escalating_failure', 'failure', 'warning')),
	failing_checks JSONB NOT NULL,
	reasons JSONB NOT NULL,
	memory_entry_ids UUID[] NOT NULL DEFAULT '{}',
	note_id UUID REFERENCES incident_notes(id) ON DELETE SET NULL,
	report_link TEXT CHECK (char_length(report_link) BETWEEN 1 AND 2000),
	reported_at TIMESTAMPTZ,
	recorded_by TEXT NOT NULL,
	CHECK ((report_link IS NULL) = (reported_at IS NULL))
);

CREATE INDEX alert_triage_verdicts_incident
	ON alert_triage_verdicts (incident_id, created_at DESC);
CREATE INDEX alert_triage_verdicts_created ON alert_triage_verdicts (created_at);
