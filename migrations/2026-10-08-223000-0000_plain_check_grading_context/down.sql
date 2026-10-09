UPDATE issues SET grading_context = NULL WHERE instances IS NULL;

ALTER TABLE issues DROP CONSTRAINT issues_instances_graded;

ALTER TABLE issues ADD CONSTRAINT issues_instances_graded_together
	CHECK ((instances IS NULL) = (grading_context IS NULL));
