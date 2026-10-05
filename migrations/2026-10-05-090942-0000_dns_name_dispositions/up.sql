-- What happens to a DNS name a machine asks about that it cannot be resolved
-- for (CRT, "Undeclared requests" and "Denied DNS names").
--
-- Both are per machine rather than per application: an identity is the box's,
-- and a request that resolves to no application is by definition not any one
-- application's.

-- A request refused because it resolved to no single application on the
-- machine. One row per machine and DNS name, updated on every repeat ask; a row
-- not asked about for a day no longer counts and is pruned.
CREATE TABLE undeclared_dns_names (
	id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	machine_id     UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
	-- Normalised: lower case, no trailing dot.
	dns_name       TEXT NOT NULL,
	-- What the latest refused request was for.
	asked_for      TEXT NOT NULL CHECK (asked_for IN ('addresses', 'certificate')),
	first_asked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	last_asked_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
	UNIQUE (machine_id, dns_name)
);

CREATE INDEX undeclared_dns_names_last_asked ON undeclared_dns_names (last_asked_at);

-- An operator's decision that a machine is not to be served a DNS name. Stands
-- until lifted or until the DNS name is declared on one of the machine's
-- applications.
CREATE TABLE denied_dns_names (
	id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	machine_id UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
	-- Normalised: lower case, no trailing dot.
	dns_name   TEXT NOT NULL,
	denied_by  TEXT NOT NULL,
	note       TEXT,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	UNIQUE (machine_id, dns_name)
);
