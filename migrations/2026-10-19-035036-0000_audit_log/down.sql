DROP TABLE audit_entries;
DROP FUNCTION audit_entries_guard();

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
                    'CREATE TABLE %I (LIKE %I INCLUDING DEFAULTS INCLUDING CONSTRAINTS INCLUDING STORAGE)',
                    v_name, parent
                );
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
    WHERE p.relkind = 'p' AND n.nspname = 'public'
    GROUP BY p.relname;
$$;
