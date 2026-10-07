-- Cron schedules for backups, per machine, with the timezone they are read in.
--
-- Until now a type's schedule was manual-only or an interval, set per group
-- over a fleet-wide default. A schedule is now one of manual-only, an interval,
-- or a cron expression with an optional zone, and a machine can hold its own
-- over its group's. See [BKO](.workhorse/specs/private-server/backup.md).
--
-- Every layer stores a schedule as the same three columns: an interval, a cron
-- expression, a zone. At most one of interval and cron is set, neither means
-- manual-only, and a zone only goes with a cron expression.
--
-- Columns are appended rather than reordered: every model here loads
-- positionally.

-- The shortest interval is an hour. The UI never offered less; the data is
-- brought into line so the floor holds there too.
UPDATE backup_type_defaults SET default_interval = INTERVAL '1 hour'
	WHERE default_interval < INTERVAL '1 hour';
UPDATE server_group_backup_schedule SET expected_interval = INTERVAL '1 hour'
	WHERE expected_interval < INTERVAL '1 hour';

ALTER TABLE backup_type_defaults
	ADD COLUMN default_cron TEXT,
	ADD COLUMN default_zone TEXT,
	ADD CONSTRAINT backup_type_defaults_schedule_shape CHECK (
		(default_interval IS NULL OR default_cron IS NULL)
		AND (default_zone IS NULL OR default_cron IS NOT NULL)
	);

-- A group's row carries a schedule override and a retention override apart:
-- `has_schedule` says whether the schedule columns override anything, so the
-- schedule can be cleared while a retention override stays (and the reverse).
-- Rows that existed decided their schedule alone, so they have one.
ALTER TABLE server_group_backup_schedule
	ADD COLUMN expected_cron TEXT,
	ADD COLUMN schedule_zone TEXT,
	ADD COLUMN has_schedule BOOLEAN NOT NULL DEFAULT true,
	ADD CONSTRAINT server_group_backup_schedule_shape CHECK (
		(expected_interval IS NULL OR expected_cron IS NULL)
		AND (schedule_zone IS NULL OR expected_cron IS NOT NULL)
		AND (has_schedule OR (
			expected_interval IS NULL AND expected_cron IS NULL AND schedule_zone IS NULL
		))
	);

-- A machine's own override. Keyed by machine rather than capability or group,
-- so it survives a group move and a type being disabled and re-enabled.
CREATE TABLE machine_backup_schedule (
	machine_id        UUID NOT NULL REFERENCES machines (id) ON DELETE CASCADE,
	type              TEXT NOT NULL,
	expected_interval INTERVAL,
	expected_cron     TEXT,
	schedule_zone     TEXT,
	created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
	updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (machine_id, type),
	CONSTRAINT machine_backup_schedule_shape CHECK (
		(expected_interval IS NULL OR expected_cron IS NULL)
		AND (schedule_zone IS NULL OR expected_cron IS NOT NULL)
	)
);
SELECT diesel_manage_updated_at('machine_backup_schedule');

-- Every set and clear of a layer, newest last. The layer's current row is the
-- latest entry; the rest is the history shown beside each editor, and what
-- works out when a machine's resolved schedule last changed.
--
-- `kind` is what the layer held after the change: manual, interval or cron, or
-- null when the change cleared the layer so it no longer overrides anything.
CREATE TABLE backup_schedule_history (
	id          BIGSERIAL PRIMARY KEY,
	layer       TEXT NOT NULL CHECK (layer IN ('machine', 'group', 'fleet')),
	type        TEXT NOT NULL,
	group_id    UUID REFERENCES server_groups (id) ON DELETE CASCADE,
	machine_id  UUID REFERENCES machines (id) ON DELETE CASCADE,
	kind        TEXT CHECK (kind IN ('manual', 'interval', 'cron')),
	interval    INTERVAL,
	cron        TEXT,
	zone        TEXT,
	changed_by  TEXT,
	changed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
	CONSTRAINT backup_schedule_history_target CHECK (
		(layer = 'machine' AND machine_id IS NOT NULL AND group_id IS NULL)
		OR (layer = 'group' AND group_id IS NOT NULL AND machine_id IS NULL)
		OR (layer = 'fleet' AND group_id IS NULL AND machine_id IS NULL)
	)
);
CREATE INDEX backup_schedule_history_group_idx
	ON backup_schedule_history (group_id, type, changed_at) WHERE group_id IS NOT NULL;
CREATE INDEX backup_schedule_history_machine_idx
	ON backup_schedule_history (machine_id, type, changed_at) WHERE machine_id IS NOT NULL;
CREATE INDEX backup_schedule_history_fleet_idx
	ON backup_schedule_history (type, changed_at) WHERE layer = 'fleet';

-- What stands now was set at some point nobody recorded; seed each layer's
-- history with it so the replay that works out when a schedule last changed
-- starts from the right place. The epoch stands for "always".
INSERT INTO backup_schedule_history (layer, type, kind, interval, changed_at)
SELECT 'fleet', type,
	CASE WHEN default_interval IS NULL THEN 'manual' ELSE 'interval' END,
	default_interval,
	TIMESTAMPTZ 'epoch'
FROM backup_type_defaults;

INSERT INTO backup_schedule_history (layer, type, group_id, kind, interval, changed_at)
SELECT 'group', type, group_id,
	CASE WHEN expected_interval IS NULL THEN 'manual' ELSE 'interval' END,
	expected_interval,
	updated_at
FROM server_group_backup_schedule;

-- The operating system timezone each machine reports, and when that last
-- changed. A schedule that reads in the machine's zone takes effect anew when
-- the machine reports a different one, so the moment is recorded as the report
-- comes in rather than read back out of status history.
CREATE TABLE machine_reported_timezone (
	machine_id UUID PRIMARY KEY REFERENCES machines (id) ON DELETE CASCADE,
	timezone   TEXT NOT NULL,
	changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO machine_reported_timezone (machine_id, timezone, changed_at)
SELECT DISTINCT ON (machine_id) machine_id, extra ->> 'osTimezone', reported_at
FROM machine_reported_detail
WHERE jsonb_typeof(extra -> 'osTimezone') = 'string'
	AND extra ->> 'osTimezone' <> ''
ORDER BY machine_id, reported_at DESC;
