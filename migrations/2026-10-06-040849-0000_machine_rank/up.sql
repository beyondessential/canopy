-- A machine carries its own rank, which the applications on it share, so a box
-- can be ranked before anything on it has reported and what arrives on it is
-- ranked from the start.
ALTER TABLE machines
	ADD COLUMN rank TEXT
		CONSTRAINT machines_rank_canonical
		CHECK (rank IN ('production', 'clone', 'demo', 'test', 'dev'));

-- A box is ranked where its live applications are, which the exclusion
-- constraint on applications holds to one rank. A box whose applications are
-- all archived starts unranked, and restoring one of them ranks it again.
UPDATE machines SET rank = box.rank
FROM (
	SELECT machine_id, max(rank) AS rank
	FROM applications
	WHERE machine_id IS NOT NULL AND deleted_at IS NULL
		AND rank IN ('production', 'clone', 'demo', 'test', 'dev')
	GROUP BY machine_id
) AS box
WHERE machines.id = box.machine_id;

-- `applications.rank` stays as a denormalisation, so every query reading an
-- application's environment reads one column. These triggers keep it and the
-- box's rank together whichever side is written: `Machine::set_rank` writes the
-- box and does what a column write cannot, re-evaluating open issues against
-- the environment they now belong to, while the triggers cover every other
-- writer, such as raw SQL, a restore, or a backfill.
--
-- They are mutually recursive and terminate: each writes only where the rank
-- differs, so the write coming back the other way changes nothing.

-- An application stood up on a ranked box without a rank takes the box's. It
-- only ever fills a missing rank: a BEFORE trigger cannot see a machine created
-- earlier in the same statement, and filling nothing from nothing is harmless
-- where overwriting would not be.
CREATE FUNCTION applications_take_machine_rank() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	SELECT m.rank INTO NEW.rank FROM machines m WHERE m.id = NEW.machine_id;
	RETURN NEW;
END;
$$;

CREATE TRIGGER applications_take_machine_rank
	BEFORE INSERT ON applications
	FOR EACH ROW
	WHEN (NEW.rank IS NULL AND NEW.machine_id IS NOT NULL AND NEW.deleted_at IS NULL)
	EXECUTE FUNCTION applications_take_machine_rank();

-- A live application's rank is its box's, so ranking one ranks the box.
CREATE FUNCTION application_rank_ranks_machine() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	UPDATE machines SET rank = NEW.rank
	WHERE id = NEW.machine_id AND rank IS DISTINCT FROM NEW.rank;
	RETURN NULL;
END;
$$;

CREATE TRIGGER application_rank_ranks_machine
	AFTER INSERT OR UPDATE OF rank, machine_id, deleted_at ON applications
	FOR EACH ROW
	WHEN (NEW.rank IS NOT NULL AND NEW.machine_id IS NOT NULL AND NEW.deleted_at IS NULL)
	EXECUTE FUNCTION application_rank_ranks_machine();

-- Ranking a box ranks every live application on it. Archived ones keep the
-- rank they left at, and restoring one brings it to the box's.
CREATE FUNCTION machine_rank_propagates() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	UPDATE applications SET rank = NEW.rank
	WHERE machine_id = NEW.id AND deleted_at IS NULL
		AND rank IS DISTINCT FROM NEW.rank;
	RETURN NULL;
END;
$$;

CREATE TRIGGER machine_rank_propagates
	AFTER UPDATE OF rank ON machines
	FOR EACH ROW
	WHEN (NEW.rank IS NOT NULL AND NEW.rank IS DISTINCT FROM OLD.rank)
	EXECUTE FUNCTION machine_rank_propagates();
