-- The variables a configuration run receives, at one of three scopes: a group,
-- an environment (a group at one rank), or a machine. A run's view merges the
-- three name-wise, a machine's over its environment's over its group's.
--
-- A secret's value lives in canopy's secret store and never here, so a name can
-- be listed without reading one.
CREATE TABLE inventory_variables (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	server_group_id UUID REFERENCES server_groups (id) ON DELETE CASCADE,
	rank TEXT,
	machine_id UUID REFERENCES machines (id) ON DELETE CASCADE,
	name TEXT NOT NULL,
	value JSONB,
	is_secret BOOLEAN NOT NULL DEFAULT FALSE,
	set_by TEXT,
	created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	CONSTRAINT inventory_variables_one_scope CHECK (
		(server_group_id IS NOT NULL AND machine_id IS NULL)
		OR (machine_id IS NOT NULL AND server_group_id IS NULL AND rank IS NULL)
	),
	CONSTRAINT inventory_variables_secret_value CHECK (is_secret = (value IS NULL)),
	-- A secret's name is the key its value is stored under.
	CONSTRAINT inventory_variables_name CHECK (name ~ '^[-._a-zA-Z0-9]+$')
);

CREATE UNIQUE INDEX inventory_variables_one_per_group
	ON inventory_variables (server_group_id, name)
	WHERE rank IS NULL AND machine_id IS NULL;
CREATE UNIQUE INDEX inventory_variables_one_per_environment
	ON inventory_variables (server_group_id, rank, name)
	WHERE rank IS NOT NULL;
CREATE UNIQUE INDEX inventory_variables_one_per_machine
	ON inventory_variables (machine_id, name)
	WHERE machine_id IS NOT NULL;

-- The lease a configuration run holds over an environment while it runs. The
-- inventory is served to the holder alone, so two runs never act on one
-- environment at once.
CREATE TABLE inventory_leases (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	server_group_id UUID NOT NULL REFERENCES server_groups (id) ON DELETE CASCADE,
	rank TEXT NOT NULL,
	intent TEXT NOT NULL,
	held_by TEXT,
	note TEXT,
	taken_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	expires_at TIMESTAMPTZ NOT NULL,
	released_at TIMESTAMPTZ,
	released_by TEXT,
	CONSTRAINT inventory_leases_intent CHECK (intent IN ('configure', 'upgrade'))
);

CREATE UNIQUE INDEX inventory_leases_one_open_per_environment
	ON inventory_leases (server_group_id, rank)
	WHERE released_at IS NULL;
