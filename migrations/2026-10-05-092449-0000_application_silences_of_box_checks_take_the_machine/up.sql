-- A check filed against an application is in that application type's
-- namespace, whatever it is called (see CHK, "Names"). A silence held at an
-- application in the machine namespace can therefore never match anything
-- again.
--
-- Every such row predates the machine grain. Before it, the box's checks
-- (`disk_free`, `time_sync` and their like) were filed against the application
-- on it, so silencing one there silenced the box's check. The check-namespace
-- migration re-keyed those rows to the machine namespace and left them where
-- they were. The box has since filed those checks itself, so the instruction is
-- carried to the box: the row moves to the application's machine.
--
-- A box already carrying the same silence keeps its own, and two applications
-- on one box carrying it become one row, the earliest. An application with no
-- machine has no box to carry it to, and its row goes: it silences nothing.
-- Nor does a deleted application's: its operator's instruction went with it,
-- and carrying it would silence a live box nobody asked about.
--
-- The check-states those silences quieted stay where they are, with no catalog
-- entry created for them. They are the box's checks as filed against the
-- application before the machine grain, and with no entry in the
-- application's namespace they are no longer live, so they drop out of every
-- rollup. Seeding entries for them would revive that history. An application
-- that reports such a name under itself gets its entry on its next push.

WITH stranded AS (
	SELECT
		silence.id,
		app.machine_id,
		row_number() OVER (
			PARTITION BY app.machine_id, silence.source, silence.check_name, silence.instance_key
			ORDER BY silence.created_at, silence.id
		) AS rank
	FROM scoped_check_policies silence
	JOIN applications app ON app.id = silence.application_id
	WHERE silence.subject = 'machine'
	  AND app.machine_id IS NOT NULL
	  AND app.deleted_at IS NULL
	  AND app.id <> '00000000-0000-0000-0000-000000000000'
	  AND NOT EXISTS (
		SELECT 1 FROM scoped_check_policies held
		WHERE held.machine_id = app.machine_id
		  AND held.source = silence.source
		  AND held.subject = 'machine'
		  AND held.check_name = silence.check_name
		  AND held.instance_key IS NOT DISTINCT FROM silence.instance_key
	  )
)
UPDATE scoped_check_policies silence
SET application_id = NULL, machine_id = stranded.machine_id, updated_at = now()
FROM stranded
WHERE silence.id = stranded.id
  AND stranded.rank = 1;

DELETE FROM scoped_check_policies
WHERE application_id IS NOT NULL
  AND subject = 'machine';
