-- A window can be moved to another grain on its target's line of descent, so a
-- window declared too wide or too narrow is corrected without lifting it. The
-- window row keeps the target it covers now; each move records the target it
-- left, over the span it covered there.
--
-- What a move leaves uncovered settles as though the window had ended over it,
-- so a move row is read as settling until its settle period has elapsed, and is
-- the old target's maintenance history thereafter.
CREATE TABLE maintenance_window_moves (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	window_id UUID NOT NULL REFERENCES maintenance_windows (id) ON DELETE CASCADE,
	application_id UUID REFERENCES applications (id) ON DELETE CASCADE,
	machine_id UUID REFERENCES machines (id) ON DELETE CASCADE,
	server_group_id UUID REFERENCES server_groups (id) ON DELETE CASCADE,
	rank TEXT,
	-- When the window started covering this target: its declaration, or the
	-- move that brought it here.
	covered_from TIMESTAMPTZ NOT NULL,
	moved_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	moved_by TEXT,
	-- Stamped once the settle period after the move has elapsed and this
	-- target's issues have been re-evaluated, so that happens exactly once.
	settled_at TIMESTAMPTZ,
	CONSTRAINT maintenance_window_moves_one_target
		CHECK (num_nonnulls(application_id, machine_id, server_group_id) = 1),
	CONSTRAINT maintenance_window_moves_rank_needs_group
		CHECK (rank IS NULL OR server_group_id IS NOT NULL),
	CONSTRAINT maintenance_window_moves_rank_check
		CHECK (rank IS NULL OR rank IN ('production', 'clone', 'demo', 'test', 'dev'))
);

CREATE INDEX maintenance_window_moves_window
	ON maintenance_window_moves (window_id, moved_at DESC);
CREATE INDEX maintenance_window_moves_unsettled
	ON maintenance_window_moves (moved_at)
	WHERE settled_at IS NULL;
CREATE INDEX maintenance_window_moves_application
	ON maintenance_window_moves (application_id, moved_at DESC);
CREATE INDEX maintenance_window_moves_machine
	ON maintenance_window_moves (machine_id, moved_at DESC);
CREATE INDEX maintenance_window_moves_group
	ON maintenance_window_moves (server_group_id, moved_at DESC);

-- A window declared from an upgrade plan's offer is that plan's window, and
-- stays over the plan's environment so the plan it holds open stays held.
ALTER TABLE maintenance_windows
	ADD COLUMN upgrade_plan_id UUID REFERENCES upgrade_plans (id) ON DELETE SET NULL;
