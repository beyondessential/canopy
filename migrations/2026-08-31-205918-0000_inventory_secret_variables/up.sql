-- Which secret variables an environment or an application carries, and who last
-- set each. The values are held in canopy's secret store and never here: this
-- table is the index that lets a name be listed, and a tag collision refused,
-- without reading a value.
CREATE TABLE inventory_secret_variables (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	server_group_id UUID REFERENCES server_groups (id) ON DELETE CASCADE,
	rank TEXT,
	application_id UUID REFERENCES applications (id) ON DELETE CASCADE,
	name TEXT NOT NULL,
	set_by TEXT,
	created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	-- An environment (a group at one rank) or a single application. A group
	-- without a rank is not a scope: a value its environments would then share
	-- is the one that most needs to differ between them.
	CONSTRAINT inventory_secret_variables_one_scope CHECK (
		(server_group_id IS NOT NULL AND rank IS NOT NULL AND application_id IS NULL)
		OR (application_id IS NOT NULL AND server_group_id IS NULL AND rank IS NULL)
	),
	-- The name is also the key the value is stored under.
	CONSTRAINT inventory_secret_variables_name CHECK (name ~ '^[-._a-zA-Z0-9]+$')
);

CREATE UNIQUE INDEX inventory_secret_variables_one_per_environment
	ON inventory_secret_variables (server_group_id, rank, name)
	WHERE application_id IS NULL;
CREATE UNIQUE INDEX inventory_secret_variables_one_per_application
	ON inventory_secret_variables (application_id, name)
	WHERE application_id IS NOT NULL;
