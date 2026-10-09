-- A change to a check's policy re-grades every state of the check from what
-- its most recent report observed (spec CHK, "Policy"), so a state keeps the
-- inputs its rules read beyond the check itself (the report's fields and the
-- target's tags) whether or not the check has instances. A state with
-- instances still always has them; a plain one may have them too.

ALTER TABLE issues DROP CONSTRAINT issues_instances_graded_together;

ALTER TABLE issues ADD CONSTRAINT issues_instances_graded
	CHECK (instances IS NULL OR grading_context IS NOT NULL);
