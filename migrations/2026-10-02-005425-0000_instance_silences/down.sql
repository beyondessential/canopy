-- Instance silences have no form without the key, and left in place they would
-- widen to silence the whole check, so they go.
DELETE FROM scoped_check_policies WHERE instance_key IS NOT NULL;

DROP INDEX scoped_check_policies_application;
DROP INDEX scoped_check_policies_machine;
DROP INDEX scoped_check_policies_group;
DROP INDEX scoped_check_policies_kubernetes_cluster;
DROP INDEX scoped_check_policies_global;

CREATE UNIQUE INDEX scoped_check_policies_application
	ON scoped_check_policies (application_id, source, subject, application_type, check_name)
	NULLS NOT DISTINCT
	WHERE application_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_machine
	ON scoped_check_policies (machine_id, source, subject, application_type, check_name)
	NULLS NOT DISTINCT
	WHERE machine_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_group
	ON scoped_check_policies (server_group_id, source, subject, application_type, check_name)
	NULLS NOT DISTINCT
	WHERE server_group_id IS NOT NULL;
-- The cluster and canopy-wide indexes keep the namespace columns the up
-- restored to them: rows that differ only by namespace may exist by now, and
-- keying by name alone again would refuse them.
CREATE UNIQUE INDEX scoped_check_policies_kubernetes_cluster
	ON scoped_check_policies (kubernetes_cluster_id, source, subject, application_type, check_name)
	NULLS NOT DISTINCT
	WHERE kubernetes_cluster_id IS NOT NULL;
CREATE UNIQUE INDEX scoped_check_policies_global
	ON scoped_check_policies (source, subject, application_type, check_name)
	NULLS NOT DISTINCT
	WHERE application_id IS NULL
		AND machine_id IS NULL
		AND server_group_id IS NULL
		AND kubernetes_cluster_id IS NULL;

ALTER TABLE scoped_check_policies DROP CONSTRAINT scoped_check_policies_instance_key_not_empty;
ALTER TABLE scoped_check_policies DROP COLUMN instance_key;
