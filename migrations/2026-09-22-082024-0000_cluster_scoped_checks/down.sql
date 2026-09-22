-- ── scoped_check_policies ───────────────────────────────────────────────────

DROP INDEX scoped_check_policies_global;
CREATE UNIQUE INDEX scoped_check_policies_global
	ON scoped_check_policies (source, check_name)
	WHERE application_id IS NULL AND machine_id IS NULL AND server_group_id IS NULL;

DROP INDEX scoped_check_policies_kubernetes_cluster;

ALTER TABLE scoped_check_policies DROP CONSTRAINT scoped_check_policies_check;
ALTER TABLE scoped_check_policies ADD CONSTRAINT scoped_check_policies_check CHECK (
	(application_id IS NOT NULL)::int
	+ (machine_id IS NOT NULL)::int
	+ (server_group_id IS NOT NULL)::int
	<= 1
);

ALTER TABLE scoped_check_policies DROP COLUMN kubernetes_cluster_id;

-- ── issues ──────────────────────────────────────────────────────────────────

DROP INDEX issues_global_source_ref;
CREATE UNIQUE INDEX issues_global_source_ref
	ON issues (source, "ref")
	WHERE application_id IS NULL AND machine_id IS NULL AND server_group_id IS NULL;

DROP INDEX issues_kubernetes_cluster_last_seen;
DROP INDEX issues_kubernetes_cluster_source_ref;

ALTER TABLE issues DROP CONSTRAINT issues_scope_at_most_one;
ALTER TABLE issues ADD CONSTRAINT issues_scope_at_most_one CHECK (
	(application_id IS NOT NULL)::int
	+ (machine_id IS NOT NULL)::int
	+ (server_group_id IS NOT NULL)::int
	<= 1
);

ALTER TABLE issues DROP COLUMN kubernetes_cluster_id;
