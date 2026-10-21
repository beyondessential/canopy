-- The variable catalogue infra publishes, and what Canopy last made of reading
-- it. One row. The document is the last copy that read as a catalogue, so a
-- restart while the repository is unreachable still has one; a problem says
-- why the latest attempt did not replace it.
CREATE TABLE ansible_catalogue (
	id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
	document JSONB,
	digest TEXT,
	source_commit TEXT,
	read_at TIMESTAMPTZ,
	first_checked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	checked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	problem TEXT,
	problem_detail TEXT,
	CONSTRAINT ansible_catalogue_read CHECK (
		(document IS NULL) = (digest IS NULL) AND (document IS NULL) = (read_at IS NULL)
	),
	CONSTRAINT ansible_catalogue_problem CHECK (problem IN ('unreachable', 'invalid'))
);
