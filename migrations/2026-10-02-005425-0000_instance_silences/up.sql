-- A silence can name one instance of a check (spec CHK, "Silencing one
-- instance"): the instance with that key is quieted on the target, and the
-- check's other instances are graded as before.
--
-- An instance silence is an ordinary scoped silence that also carries the key,
-- so it is set, recorded and scoped exactly as one. A row with no key applies
-- to every instance of the check, which is what every row before this one
-- meant; a row with a key applies to that instance alone.
--
-- The empty key is the instance a check without instances is graded as, so a
-- row naming it would be a whole-check silence spelled a second way. It is
-- refused here rather than left for the readers to reconcile.

ALTER TABLE scoped_check_policies ADD COLUMN instance_key TEXT;

ALTER TABLE scoped_check_policies ADD CONSTRAINT scoped_check_policies_instance_key_not_empty
	CHECK (instance_key IS NULL OR instance_key <> '');

-- One transform per (scope, source, namespace, check, instance), so a
-- whole-check silence and silences on several of the check's instances sit
-- side by side at one scope. NULLS NOT DISTINCT keeps the keyless row unique
-- per check, as the namespace columns already rely on.
--
-- The cluster and canopy-wide indexes are brought back to the namespace
-- columns too: the cluster migration recreated them keyed by name alone.

DROP INDEX scoped_check_policies_application;
DROP INDEX scoped_check_policies_machine;
DROP INDEX scoped_check_policies_group;
DROP INDEX scoped_check_policies_kubernetes_cluster;
DROP INDEX scoped_check_policies_global;

CREATE UNIQUE INDEX scoped_check_policies_application
	ON scoped_check_policies (application_id, source, subject, application_type, check_name, instance_key)
	NULLS NOT DISTINCT
	WHERE application_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_machine
	ON scoped_check_policies (machine_id, source, subject, application_type, check_name, instance_key)
	NULLS NOT DISTINCT
	WHERE machine_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_group
	ON scoped_check_policies (server_group_id, source, subject, application_type, check_name, instance_key)
	NULLS NOT DISTINCT
	WHERE server_group_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_kubernetes_cluster
	ON scoped_check_policies (kubernetes_cluster_id, source, subject, application_type, check_name, instance_key)
	NULLS NOT DISTINCT
	WHERE kubernetes_cluster_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_global
	ON scoped_check_policies (source, subject, application_type, check_name, instance_key)
	NULLS NOT DISTINCT
	WHERE application_id IS NULL
		AND machine_id IS NULL
		AND server_group_id IS NULL
		AND kubernetes_cluster_id IS NULL;

-- ── issues: a check state's instances ──────────────────────────────────────
--
-- A check with instances keeps them in a column of their own (spec CHK,
-- "Checks with instances"), so whether a state has instances is a fact of the
-- row rather than something read off the shape of its detail: `detail` holds a
-- plain check's fields, or the fields an instanced check shares, and a plain
-- check reporting a field named `instances` is still a plain check.
--
-- `instances` is the check's instances by key, each with its label, observed
-- and effective result, and own fields. `grading_context` is what the filing
-- that graded them gave its rules beyond the instance (the report's fields and
-- the target's tags), so a re-grade after an instance silence replays exactly
-- that rather than reconstructing it. The two are set together or not at all.
--
-- `title` is the headline the last filing gave the check, kept whatever the
-- state's result, where `description` is the headline only while it is
-- degraded. A re-grade that brings a state back into trouble presents it.

ALTER TABLE issues
	ADD COLUMN title TEXT,
	ADD COLUMN instances JSONB,
	ADD COLUMN grading_context JSONB;

ALTER TABLE issues ADD CONSTRAINT issues_instances_graded_together
	CHECK ((instances IS NULL) = (grading_context IS NULL));
