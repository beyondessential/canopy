-- A machine carries its own rank, which the applications on it share, so a box
-- can be ranked before anything on it has reported and what arrives on it is
-- ranked from the start.

-- The one place SQL spells out what a rank is: each spelling `ServerRank` reads,
-- mapped to the one it writes, and anything else to NULL. A test holds this to
-- `ServerRank::SPELLINGS`, the table its own parsing reads.
CREATE FUNCTION rank_canonical(rank TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
	SELECT CASE lower(rank)
		WHEN 'production' THEN 'production'
		WHEN 'live' THEN 'production'
		WHEN 'prod' THEN 'production'
		WHEN 'clone' THEN 'clone'
		WHEN 'staging' THEN 'clone'
		WHEN 'demo' THEN 'demo'
		WHEN 'test' THEN 'test'
		WHEN 'dev' THEN 'dev'
	END
$$;

ALTER TABLE machines
	ADD COLUMN rank TEXT
		-- IS NOT DISTINCT FROM, so a spelling no one recognises, whose canonical
		-- form is NULL, is refused rather than passing as an unknown.
		CONSTRAINT machines_rank_canonical
			CHECK (rank_canonical(rank) IS NOT DISTINCT FROM rank);

-- A box is ranked where its live applications are, which the exclusion
-- constraint on applications holds to one rank. A box whose applications are
-- all archived starts unranked, and restoring one of them ranks it again.
UPDATE machines SET rank = box.rank
FROM (
	SELECT machine_id, max(rank_canonical(rank)) AS rank
	FROM applications
	WHERE machine_id IS NOT NULL AND deleted_at IS NULL
	GROUP BY machine_id
) AS box
WHERE machines.id = box.machine_id AND box.rank IS NOT NULL;

-- `applications.rank` stays as a denormalisation, so every query reading an
-- application's environment reads one column. These triggers keep it and the
-- box's rank together whichever side is ranked: `Machine::set_rank` writes the
-- box and does what a column write cannot, re-evaluating open issues against
-- the environment they now belong to, while the triggers cover every other
-- writer, such as raw SQL, a restore, or a backfill. A rank is never cleared,
-- so neither side propagates a NULL.
--
-- They are mutually recursive and terminate: each writes only where the rank
-- differs, so the write coming back the other way changes nothing.

-- An application joining a ranked box takes the box's rank, whatever rank it
-- was written with, as its group is the box's. An application stood up on a
-- box is joining it, and so is one moved onto the box or brought back from the
-- archive; one keeping its rank where it stands is not, and ranking it ranks
-- the box (below).
--
-- A BEFORE trigger cannot see a machine created earlier in the same statement,
-- so where the lookup finds nothing the application keeps the rank it was
-- given, and that ranks the new box in turn.
CREATE FUNCTION applications_take_machine_rank() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	NEW.rank := COALESCE((SELECT m.rank FROM machines m WHERE m.id = NEW.machine_id), NEW.rank);
	RETURN NEW;
END;
$$;

CREATE TRIGGER applications_take_machine_rank_on_insert
	BEFORE INSERT ON applications
	FOR EACH ROW
	WHEN (NEW.machine_id IS NOT NULL AND NEW.deleted_at IS NULL)
	EXECUTE FUNCTION applications_take_machine_rank();

CREATE TRIGGER applications_take_machine_rank_on_join
	BEFORE UPDATE OF machine_id, deleted_at ON applications
	FOR EACH ROW
	WHEN (NEW.machine_id IS NOT NULL AND NEW.deleted_at IS NULL
		AND (NEW.machine_id IS DISTINCT FROM OLD.machine_id OR OLD.deleted_at IS NOT NULL))
	EXECUTE FUNCTION applications_take_machine_rank();

-- A live application's rank is its box's, so ranking one ranks the box. By the
-- time this runs, an application joining a ranked box has taken its rank, so
-- this only ever ranks a box that had none or re-ranks one from an application
-- already on it. Where it was the box that ranked the application, the box
-- already carries the rank and this writes nothing. `applications.rank` is read leniently, so an older spelling
-- ranks the box as the rank it names, and a spelling no one recognises, which
-- reads as unranked, leaves the box alone.
CREATE FUNCTION application_rank_ranks_machine() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
	canonical TEXT := rank_canonical(NEW.rank);
BEGIN
	IF canonical IS NOT NULL THEN
		UPDATE machines SET rank = canonical
		WHERE id = NEW.machine_id AND rank IS DISTINCT FROM canonical;
	END IF;
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
