-- A window over several machines of one environment, for work on some of an
-- environment's machines and not the rest, such as a run limited to two
-- facilities. It is one window row, so it has one declarer and one history, and
-- it names its machines here rather than in a join table so that every read of
-- a window carries them.
--
-- A set window has no `machine_id`: it sits at its group and rank, with the
-- machines it names in `machine_ids`. A window naming one machine is that
-- machine's window, so a set always names at least two.
ALTER TABLE maintenance_windows
	ADD COLUMN machine_ids UUID[] NOT NULL DEFAULT '{}',
	ADD CONSTRAINT maintenance_windows_machines_shape CHECK (
		cardinality(machine_ids) = 0
		OR (
			cardinality(machine_ids) >= 2
			AND array_position(machine_ids, NULL) IS NULL
			AND machine_id IS NULL
			AND application_id IS NULL
			AND server_group_id IS NOT NULL
			AND rank IS NOT NULL
		)
	);

-- A set is not its environment's own window: each environment still has at most
-- one of those. That a machine is named by at most one open window, alone or in
-- a set, cannot be an index over an array, so it is enforced where windows are
-- written, under the group's row lock.
DROP INDEX maintenance_windows_one_open_per_environment;
CREATE UNIQUE INDEX maintenance_windows_one_open_per_environment
	ON maintenance_windows (server_group_id, rank)
	WHERE ended_at IS NULL
		AND server_group_id IS NOT NULL
		AND rank IS NOT NULL
		AND cardinality(machine_ids) = 0;

CREATE INDEX maintenance_windows_machine_ids
	ON maintenance_windows USING gin (machine_ids);

-- What a run's start opens is bound to the run's lease and lasts the run: its
-- `expected_end` follows the lease's expiry, so the sweep that ends an expired
-- window ends it with the lease, and the binding is cleared when its declarer
-- holds it to an end of their own.
ALTER TABLE maintenance_windows
	ADD COLUMN lease_id UUID REFERENCES ansible_leases (id) ON DELETE SET NULL;
CREATE INDEX maintenance_windows_lease
	ON maintenance_windows (lease_id)
	WHERE lease_id IS NOT NULL;

-- A move off one machine of a set that carried on over others did not move the
-- window: it still covers the rest from when it began, so only moves without
-- this move where the window started covering what it covers now.
ALTER TABLE maintenance_window_moves
	ADD COLUMN carried_on BOOLEAN NOT NULL DEFAULT false;
