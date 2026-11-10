-- Short titled entries carrying what past incidents taught, each linked to at
-- most one group, check or incident; none applies to the fleet as a whole.
-- A link restricts deletion rather than clearing, so an entry never silently
-- turns fleet-wide.
CREATE TABLE memory_entries (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 512),
	body TEXT NOT NULL CHECK (char_length(body) <= 16384),
	server_group_id UUID REFERENCES server_groups(id) ON DELETE RESTRICT,
	incident_id UUID REFERENCES incidents(id) ON DELETE RESTRICT,
	check_source TEXT,
	check_name TEXT,
	revision BIGINT NOT NULL DEFAULT 1,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	created_by TEXT NOT NULL,
	updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	updated_by TEXT NOT NULL,
	retired_at TIMESTAMPTZ,
	retired_by TEXT,
	retired_reason TEXT,
	search tsvector GENERATED ALWAYS AS (
		to_tsvector('english', title || ' ' || body)
	) STORED,
	CHECK ((check_source IS NULL) = (check_name IS NULL)),
	CHECK (
		(server_group_id IS NOT NULL)::int
		+ (incident_id IS NOT NULL)::int
		+ (check_source IS NOT NULL)::int <= 1
	),
	CHECK ((retired_at IS NULL) = (retired_by IS NULL))
);

CREATE INDEX memory_entries_search ON memory_entries USING gin (search);
CREATE INDEX memory_entries_updated ON memory_entries (updated_at DESC);
CREATE INDEX memory_entries_group ON memory_entries (server_group_id)
	WHERE server_group_id IS NOT NULL;
CREATE INDEX memory_entries_incident ON memory_entries (incident_id)
	WHERE incident_id IS NOT NULL;
CREATE INDEX memory_entries_check ON memory_entries (check_source, check_name)
	WHERE check_source IS NOT NULL;

-- Every version an entry has had, retirement and restoration included. The
-- version's content is what the entry held after it.
CREATE TABLE memory_entry_revisions (
	entry_id UUID NOT NULL REFERENCES memory_entries(id) ON DELETE CASCADE,
	revision BIGINT NOT NULL,
	kind TEXT NOT NULL CHECK (kind IN ('recorded', 'amended', 'retired', 'restored')),
	title TEXT NOT NULL,
	body TEXT NOT NULL,
	server_group_id UUID,
	incident_id UUID,
	check_source TEXT,
	check_name TEXT,
	actor TEXT NOT NULL,
	via TEXT NOT NULL CHECK (via IN ('operator', 'token')),
	reason TEXT,
	restored_from BIGINT,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (entry_id, revision)
);
