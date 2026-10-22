-- A secret variable's earlier value, held in the secret store for a while after
-- the entry that replaced or removed it, so the change can be undone. One row
-- per value held; the value itself is in the scope's `-held` Secret, keyed by
-- the entry. The scope columns carry no foreign keys, so a value whose scope
-- has gone is still found and forgotten when its hold runs out.
CREATE TABLE ansible_secret_values_held (
	entry_id UUID PRIMARY KEY,
	server_group_id UUID,
	rank TEXT,
	machine_id UUID,
	name TEXT NOT NULL,
	held_until TIMESTAMPTZ NOT NULL,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	CONSTRAINT ansible_secret_values_held_one_scope CHECK (
		(machine_id IS NOT NULL AND server_group_id IS NULL AND rank IS NULL)
		OR (machine_id IS NULL AND server_group_id IS NOT NULL)
	)
);

CREATE INDEX ansible_secret_values_held_until ON ansible_secret_values_held (held_until);
