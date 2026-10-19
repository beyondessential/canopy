-- The audit log: one entry per action an administrator, device, token, or job
-- takes, written as a condition of the action it records.
--
-- spec: AUD, HST
--
-- Entries are held in weekly ranges like the other histories, and each week is
-- sub-partitioned by retention class, so entries leave by dropping a partition
-- rather than by deleting rows. The week's ranges are provisioned by the same
-- `ensure_weekly_partitions` as every history, which gains the sub-partitioning
-- below, and `partition_runway` keeps reporting one row per history, not one per
-- week.

-- `ensure_weekly_partitions`, unchanged except that a history declared as
-- sub-partitioned has each week built as a list-partitioned table whose
-- sub-partitions exist before the week is attached. Building them detached, as
-- the week itself is, keeps provisioning from taking a lock on the history.
CREATE OR REPLACE FUNCTION ensure_weekly_partitions(
    parent TEXT,
    weeks_ahead INTEGER DEFAULT 4
)
RETURNS TABLE(partition_name TEXT, week_start DATE, week_end DATE, action TEXT)
LANGUAGE plpgsql
AS $$
DECLARE
    c_lock_key CONSTANT BIGINT := 6819230114770001;
    v_week_start DATE;
    v_week_end DATE;
    v_name TEXT;
    v_i INT;
    v_attached BOOLEAN;
    v_exists BOOLEAN;
    -- The histories whose weeks are list-partitioned on `retention` beneath the
    -- range. Naming one here is what makes its weeks come out that way.
    v_sub_values TEXT[] := CASE parent WHEN 'audit_entries' THEN ARRAY['standard', 'year'] END;
    v_sub TEXT;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relname = parent AND n.nspname = 'public' AND c.relkind = 'p'
    ) THEN
        RAISE EXCEPTION 'ensure_weekly_partitions: % is not a partitioned table', parent;
    END IF;

    IF NOT pg_try_advisory_xact_lock(c_lock_key) THEN
        RETURN;
    END IF;

    SET LOCAL lock_timeout = '5s';

    FOR v_i IN 0..weeks_ahead LOOP
        v_week_start := DATE_TRUNC('week', CURRENT_DATE + (v_i * INTERVAL '1 week'))::DATE;
        v_week_end := v_week_start + INTERVAL '7 days';
        v_name := FORMAT(
            '%s_%sw%s',
            parent,
            EXTRACT(ISOYEAR FROM v_week_start),
            LPAD(EXTRACT(WEEK FROM v_week_start)::TEXT, 2, '0')
        );

        SELECT EXISTS (
            SELECT 1
            FROM pg_inherits i
            JOIN pg_class c ON c.oid = i.inhrelid
            JOIN pg_class p ON p.oid = i.inhparent
            WHERE p.relname = parent AND c.relname = v_name
        ) INTO v_attached;

        partition_name := v_name;
        week_start := v_week_start;
        week_end := v_week_end;

        IF v_attached THEN
            action := 'already_exists';
            RETURN NEXT;
            CONTINUE;
        END IF;

        SELECT EXISTS (
            SELECT 1 FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relname = v_name AND n.nspname = 'public'
        ) INTO v_exists;

        BEGIN
            IF NOT v_exists THEN
                EXECUTE FORMAT(
                    'CREATE TABLE %I (LIKE %I INCLUDING DEFAULTS INCLUDING CONSTRAINTS INCLUDING STORAGE)%s',
                    v_name, parent,
                    CASE WHEN v_sub_values IS NULL THEN '' ELSE ' PARTITION BY LIST (retention)' END
                );
            END IF;

            IF v_sub_values IS NOT NULL THEN
                FOREACH v_sub IN ARRAY v_sub_values LOOP
                    EXECUTE FORMAT(
                        'CREATE TABLE IF NOT EXISTS %I PARTITION OF %I FOR VALUES IN (%L)',
                        v_name || '_' || v_sub, v_name, v_sub
                    );
                END LOOP;
            END IF;

            EXECUTE FORMAT(
                'ALTER TABLE %I ATTACH PARTITION %I FOR VALUES FROM (%L) TO (%L)',
                parent, v_name, v_week_start, v_week_end
            );

            action := CASE WHEN v_exists THEN 'attached' ELSE 'created' END;
        EXCEPTION WHEN OTHERS THEN
            action := FORMAT('failed: %s', SQLERRM);
        END;

        RETURN NEXT;
    END LOOP;
END;
$$;

-- `partition_runway`, unchanged except that a partition which is itself
-- partitioned (a sub-partitioned week) is not a history of its own.
CREATE OR REPLACE FUNCTION partition_runway()
RETURNS TABLE(parent TEXT, partitions BIGINT, covered_to DATE, days_remaining INTEGER)
LANGUAGE sql
STABLE
AS $$
    SELECT
        p.relname::TEXT,
        COUNT(c.oid)::BIGINT,
        MAX(SUBSTRING(pg_get_expr(c.relpartbound, c.oid) FROM 'TO \(''(\d{4}-\d{2}-\d{2})'))::DATE,
        COALESCE(
            MAX(SUBSTRING(pg_get_expr(c.relpartbound, c.oid) FROM 'TO \(''(\d{4}-\d{2}-\d{2})'))::DATE
                - CURRENT_DATE,
            -1
        )::INTEGER
    FROM pg_class p
    JOIN pg_namespace n ON n.oid = p.relnamespace
    LEFT JOIN pg_inherits i ON i.inhparent = p.oid
    LEFT JOIN pg_class c ON c.oid = i.inhrelid
    WHERE p.relkind = 'p' AND n.nspname = 'public' AND NOT p.relispartition
    GROUP BY p.relname;
$$;

-- `records` is every record the action acted on, each as
-- `{"kind": ..., "id": ..., "name": ...}` with the name as it was at the time.
-- None of it is a foreign key: an entry outlives what it names, and naming a
-- record must never stop it being deleted.
--
-- `outcome` is null for an entry written together with its action. An entry
-- committed ahead of its action starts `pending` and is settled once, `done` or
-- `failed`.
--
-- The key carries both partition columns because a primary key on a
-- partitioned table has to.
CREATE TABLE audit_entries (
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    recorded_at timestamptz NOT NULL DEFAULT now(),
    retention text NOT NULL DEFAULT 'standard' CHECK (retention IN ('standard', 'year')),
    actor_kind text NOT NULL CHECK (actor_kind IN ('operator', 'device', 'token', 'canopy')),
    actor_id text NOT NULL,
    actor_name text,
    actor_role text,
    actor_application_id uuid,
    action text NOT NULL,
    records jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(records) = 'array'),
    detail jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(detail) = 'object'),
    outcome text CHECK (outcome IN ('pending', 'done', 'failed')),
    settled_at timestamptz,
    CHECK (
        (outcome IS NULL OR outcome = 'pending') = (settled_at IS NULL)
    ),
    -- A device and a token are named by their identifier, a device carries its
    -- role, and a token its name, so an actor always reads back whole.
    CHECK (
        actor_kind NOT IN ('device', 'token')
        OR actor_id ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    ),
    CHECK (actor_kind <> 'device' OR actor_role IS NOT NULL),
    CHECK (actor_kind <> 'token' OR actor_name IS NOT NULL),
    PRIMARY KEY (id, recorded_at, retention)
) PARTITION BY RANGE (recorded_at);

CREATE INDEX audit_entries_recorded_at_idx ON audit_entries (recorded_at DESC);
CREATE INDEX audit_entries_action_idx ON audit_entries (action, recorded_at DESC);
CREATE INDEX audit_entries_actor_idx ON audit_entries (actor_kind, actor_id, recorded_at DESC);
CREATE INDEX audit_entries_records_idx ON audit_entries USING gin (records jsonb_path_ops);
CREATE INDEX audit_entries_detail_idx ON audit_entries USING gin (detail jsonb_path_ops);

-- Searching by part of a record's name, as the log page and the fleet query
-- interface do, reads the names of an entry's records as one text, a name to a
-- line, under a trigram index.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE FUNCTION audit_record_names(records jsonb) RETURNS text
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
RETURN (
    SELECT coalesce(string_agg(acted ->> 'name', E'\n'), '')
    FROM jsonb_array_elements(records) AS acted
);

CREATE INDEX audit_entries_record_names_idx ON audit_entries
    USING gin (audit_record_names(records) gin_trgm_ops);

-- The only change an entry ever takes is being settled: a pending entry becomes
-- done or failed, once, within an hour of being committed by the database's own
-- clock, with its detail only added to. The hour is read off the clock as the
-- settle runs, not when its transaction began, so a long transaction cannot
-- settle an entry that has lapsed; from the hour on, the entry is presented as
-- unconfirmed. Deletes are refused; entries leave by
-- dropping a partition, which fires no row trigger.
CREATE FUNCTION audit_entries_guard() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'audit entries are never deleted; they leave by dropping a partition'
            USING ERRCODE = 'restrict_violation';
    END IF;

    IF OLD.outcome IS DISTINCT FROM 'pending' THEN
        RAISE EXCEPTION 'only a pending audit entry can be settled'
            USING ERRCODE = 'restrict_violation';
    END IF;

    IF NEW.outcome IS NULL OR NEW.outcome NOT IN ('done', 'failed') THEN
        RAISE EXCEPTION 'an audit entry is settled as done or failed'
            USING ERRCODE = 'restrict_violation';
    END IF;

    IF clock_timestamp() >= OLD.recorded_at + interval '1 hour' THEN
        RAISE EXCEPTION 'this audit entry lapsed unsettled and can no longer be settled'
            USING ERRCODE = 'restrict_violation';
    END IF;

    IF (NEW.id, NEW.recorded_at, NEW.retention, NEW.actor_kind, NEW.actor_id, NEW.actor_name,
        NEW.actor_role, NEW.actor_application_id, NEW.action, NEW.records)
       IS DISTINCT FROM
       (OLD.id, OLD.recorded_at, OLD.retention, OLD.actor_kind, OLD.actor_id, OLD.actor_name,
        OLD.actor_role, OLD.actor_application_id, OLD.action, OLD.records)
    THEN
        RAISE EXCEPTION 'settling an audit entry changes only its outcome and its detail'
            USING ERRCODE = 'restrict_violation';
    END IF;

    IF EXISTS (
        SELECT 1 FROM jsonb_each(OLD.detail) AS kept
        WHERE NEW.detail -> kept.key IS DISTINCT FROM kept.value
    ) THEN
        RAISE EXCEPTION 'settling an audit entry only adds to its detail'
            USING ERRCODE = 'restrict_violation';
    END IF;

    NEW.settled_at := now();
    RETURN NEW;
END;
$$;

CREATE TRIGGER audit_entries_guard
    BEFORE UPDATE OR DELETE ON audit_entries
    FOR EACH ROW EXECUTE FUNCTION audit_entries_guard();

-- A write only lands in a week that has a range, so the log is provisioned
-- here as well as by the monitor loop.
SELECT ensure_weekly_partitions('audit_entries', 8);
