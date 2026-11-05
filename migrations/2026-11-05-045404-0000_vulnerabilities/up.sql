-- OSV advisories, the packages they are matched against, and the triage
-- decisions taken on them.
--
-- Spec: VUL (security/vulnerabilities), VTR (security/vulnerability-triage),
-- PKG (public-server/packages).

-- ── Advisories ──────────────────────────────────────────────────────────────

-- One row per OSV id, caching the record. Fetched again only when OSV reports
-- a later `modified`.
CREATE TABLE vulnerabilities (
	id TEXT PRIMARY KEY,
	aliases TEXT[] NOT NULL DEFAULT '{}',
	summary TEXT NOT NULL DEFAULT '',
	details TEXT NOT NULL DEFAULT '',
	-- critical, high, medium, low, or unrated
	severity TEXT NOT NULL DEFAULT 'unrated'
		CONSTRAINT vulnerabilities_severity_known
			CHECK (severity IN ('critical', 'high', 'medium', 'low', 'unrated')),
	affected JSONB NOT NULL DEFAULT '[]'
		CONSTRAINT vulnerabilities_affected_array CHECK (jsonb_typeof(affected) = 'array'),
	"references" JSONB NOT NULL DEFAULT '[]',
	published_at TIMESTAMPTZ,
	modified_at TIMESTAMPTZ NOT NULL,
	fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Tamanu's dependencies ───────────────────────────────────────────────────

-- A version's lockfile is read once, since a release tag does not move. This
-- is the record that it has been, so a version with no dependencies is not read
-- again either.
CREATE TABLE version_lockfiles (
	version_id UUID PRIMARY KEY REFERENCES versions (id) ON DELETE CASCADE,
	read_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Each non-dev package of a version's lockfile.
CREATE TABLE version_packages (
	version_id UUID NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
	name TEXT NOT NULL,
	version TEXT NOT NULL,
	-- The workspaces that reach the package.
	workspaces TEXT[] NOT NULL DEFAULT '{}',
	-- Whether any workspace depends on it directly.
	direct BOOLEAN NOT NULL DEFAULT false,
	-- How each workspace reaches it: workspace name to the chain of packages.
	paths JSONB NOT NULL DEFAULT '{}'
		CONSTRAINT version_packages_paths_object CHECK (jsonb_typeof(paths) = 'object'),
	PRIMARY KEY (version_id, name, version)
);

CREATE INDEX version_packages_package ON version_packages (name, version);

-- ── A machine's packages ────────────────────────────────────────────────────

-- The last report a machine made: kept until the next, however long that is.
CREATE TABLE machine_package_reports (
	machine_id UUID PRIMARY KEY REFERENCES machines (id) ON DELETE CASCADE,
	distribution TEXT NOT NULL,
	release TEXT NOT NULL,
	-- The OSV ecosystem the report is placed in, NULL when Canopy cannot place it.
	ecosystem TEXT,
	-- Of the report's content, so an identical one changes nothing.
	digest TEXT NOT NULL,
	package_count INTEGER NOT NULL,
	reported_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Source packages, by name and source version.
CREATE TABLE machine_packages (
	machine_id UUID NOT NULL REFERENCES machine_package_reports (machine_id) ON DELETE CASCADE,
	name TEXT NOT NULL,
	version TEXT NOT NULL,
	PRIMARY KEY (machine_id, name, version)
);

CREATE INDEX machine_packages_package ON machine_packages (name, version);

-- ── Items and matches ───────────────────────────────────────────────────────

-- An item is one advisory within one source. The scan keeps whether anything
-- counts for it; exposure itself is worked out from the fleet when read.
CREATE TABLE vulnerability_items (
	vulnerability_id TEXT NOT NULL REFERENCES vulnerabilities (id) ON DELETE CASCADE,
	source TEXT NOT NULL CONSTRAINT vulnerability_items_source_known CHECK (source IN ('tamanu', 'linux')),
	counts BOOLEAN NOT NULL DEFAULT true,
	first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	-- When the model last looked at the item, so a reply it cannot give is not
	-- asked for again at once.
	model_attempted_at TIMESTAMPTZ,
	PRIMARY KEY (vulnerability_id, source)
);

-- Which advisory hits which package version, from the last OSV query.
CREATE TABLE vulnerability_matches (
	vulnerability_id TEXT NOT NULL,
	source TEXT NOT NULL,
	ecosystem TEXT NOT NULL,
	package TEXT NOT NULL,
	package_version TEXT NOT NULL,
	PRIMARY KEY (vulnerability_id, ecosystem, package, package_version),
	FOREIGN KEY (vulnerability_id, source)
		REFERENCES vulnerability_items (vulnerability_id, source) ON DELETE CASCADE
);

CREATE INDEX vulnerability_matches_package ON vulnerability_matches (ecosystem, package, package_version);

-- ── Triage ──────────────────────────────────────────────────────────────────

-- Append-only: the newest row for an item is its current decision.
CREATE TABLE vulnerability_decisions (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	vulnerability_id TEXT NOT NULL,
	source TEXT NOT NULL,
	outcome TEXT NOT NULL
		CONSTRAINT vulnerability_decisions_outcome_known
			CHECK (outcome IN ('not_affected', 'accepted_risk', 'will_fix')),
	reason TEXT,
	-- When an accepted risk is looked at again.
	review_by DATE,
	issue_url TEXT,
	-- The ground the model decided on, one of the fixed ones.
	ground TEXT,
	-- The audit actor that made it: an operator, a token, or Canopy's AI worker.
	actor JSONB NOT NULL,
	-- How it was made: the operator interface, the model, or the query interface.
	via TEXT NOT NULL CONSTRAINT vulnerability_decisions_via_known CHECK (via IN ('ui', 'model', 'mcp')),
	-- The advisory as it stood, to tell whether a revision changes exposure.
	severity_held TEXT NOT NULL,
	affected_held JSONB NOT NULL,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	FOREIGN KEY (vulnerability_id, source)
		REFERENCES vulnerability_items (vulnerability_id, source) ON DELETE CASCADE,
	CONSTRAINT vulnerability_decisions_reason_needed CHECK (
		outcome = 'will_fix' OR btrim(coalesce(reason, '')) <> ''
	),
	CONSTRAINT vulnerability_decisions_review_by_needed CHECK (
		(outcome = 'accepted_risk') = (review_by IS NOT NULL)
	)
);

CREATE INDEX vulnerability_decisions_item ON vulnerability_decisions (vulnerability_id, source, created_at DESC);

-- A decision that stopped standing: its review date passed, or the advisory
-- changed so that it no longer covers what the fleet runs.
CREATE TABLE vulnerability_reopenings (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	vulnerability_id TEXT NOT NULL,
	source TEXT NOT NULL,
	-- A decision reopens once.
	decision_id UUID NOT NULL UNIQUE REFERENCES vulnerability_decisions (id) ON DELETE CASCADE,
	reason JSONB NOT NULL,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	FOREIGN KEY (vulnerability_id, source)
		REFERENCES vulnerability_items (vulnerability_id, source) ON DELETE CASCADE
);

CREATE INDEX vulnerability_reopenings_item ON vulnerability_reopenings (vulnerability_id, source, created_at DESC);

-- The model's latest suggestion for an item. Not a decision: it leaves the
-- item open.
CREATE TABLE vulnerability_suggestions (
	vulnerability_id TEXT NOT NULL,
	source TEXT NOT NULL,
	outcome TEXT NOT NULL
		CONSTRAINT vulnerability_suggestions_outcome_known
			CHECK (outcome IN ('not_affected', 'accepted_risk', 'will_fix')),
	ground TEXT,
	reason TEXT NOT NULL,
	model TEXT NOT NULL,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (vulnerability_id, source),
	FOREIGN KEY (vulnerability_id, source)
		REFERENCES vulnerability_items (vulnerability_id, source) ON DELETE CASCADE
);

-- ── Query interface access ──────────────────────────────────────────────────

-- What a token may do beyond reading, chosen when it is minted.
ALTER TABLE mcp_tokens ADD COLUMN grants TEXT[] NOT NULL DEFAULT '{}'
	CONSTRAINT mcp_tokens_grants_known CHECK (grants <@ ARRAY['triage']);
