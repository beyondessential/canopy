-- A Kubernetes cluster is a check target, a fifth grain alongside application,
-- machine, group, and canopy-wide (spec CHK, "Targets"; K8S, "A cluster is a
-- check target").
--
-- A cluster's substrate checks are read on the cluster itself, which is the
-- grain they hold for: a cluster schedules the applications of many groups,
-- where a machine carries the few colocated on one box. So a cluster is its own
-- scope rather than being folded onto its applications the way a machine's
-- checks are.
--
-- This extends the machinery machine scope already added rather than inventing
-- any: each scope is a nullable FK column, with a CHECK that at most one is set
-- and a partial unique index keying find-or-create for that grain. Storage stays
-- nullable FK columns so Postgres keeps the ON DELETE CASCADE and uniqueness
-- that prevent orphaned check-states.
--
-- THE TRAP (carried forward from machine scope). The global-scope partial
-- unique index matches on every *other* scope column being null. A
-- cluster-scoped row has the application, machine and group columns all null, so
-- without widening the global index a cluster check would fall inside it and
-- collide with a canopy-wide issue on the same (source, ref). Both global
-- indexes are therefore recreated with `AND kubernetes_cluster_id IS NULL`.
--
-- Whoever adds the next grain has to do the same.

-- ── issues ──────────────────────────────────────────────────────────────────

ALTER TABLE issues
	ADD COLUMN kubernetes_cluster_id UUID
		REFERENCES kubernetes_clusters (id) ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE issues DROP CONSTRAINT issues_scope_at_most_one;
ALTER TABLE issues ADD CONSTRAINT issues_scope_at_most_one CHECK (
	(application_id IS NOT NULL)::int
	+ (machine_id IS NOT NULL)::int
	+ (server_group_id IS NOT NULL)::int
	+ (kubernetes_cluster_id IS NOT NULL)::int
	<= 1
);

-- Find-or-create keys per cluster, mirroring the per-application, per-machine
-- and per-group unique keys.
CREATE UNIQUE INDEX issues_kubernetes_cluster_source_ref
	ON issues (kubernetes_cluster_id, source, "ref")
	WHERE kubernetes_cluster_id IS NOT NULL;

CREATE INDEX issues_kubernetes_cluster_last_seen
	ON issues (kubernetes_cluster_id, last_seen DESC)
	WHERE kubernetes_cluster_id IS NOT NULL;

-- Recreated to exclude cluster-scoped rows (see THE TRAP above).
DROP INDEX issues_global_source_ref;
CREATE UNIQUE INDEX issues_global_source_ref
	ON issues (source, "ref")
	WHERE application_id IS NULL
		AND machine_id IS NULL
		AND server_group_id IS NULL
		AND kubernetes_cluster_id IS NULL;

-- ── scoped_check_policies ───────────────────────────────────────────────────

ALTER TABLE scoped_check_policies
	ADD COLUMN kubernetes_cluster_id UUID
		REFERENCES kubernetes_clusters (id) ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE scoped_check_policies DROP CONSTRAINT scoped_check_policies_check;
ALTER TABLE scoped_check_policies ADD CONSTRAINT scoped_check_policies_check CHECK (
	(application_id IS NOT NULL)::int
	+ (machine_id IS NOT NULL)::int
	+ (server_group_id IS NOT NULL)::int
	+ (kubernetes_cluster_id IS NOT NULL)::int
	<= 1
);

CREATE UNIQUE INDEX scoped_check_policies_kubernetes_cluster
	ON scoped_check_policies (kubernetes_cluster_id, source, check_name)
	WHERE kubernetes_cluster_id IS NOT NULL;

-- Recreated to exclude cluster-scoped rows (see THE TRAP above).
DROP INDEX scoped_check_policies_global;
CREATE UNIQUE INDEX scoped_check_policies_global
	ON scoped_check_policies (source, check_name)
	WHERE application_id IS NULL
		AND machine_id IS NULL
		AND server_group_id IS NULL
		AND kubernetes_cluster_id IS NULL;
