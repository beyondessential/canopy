-- An application runs on exactly one host: a machine, or a Kubernetes cluster
-- where it is scheduled across the cluster rather than run on a box (spec
-- FLT, "Cardinality"; K8S, "A cluster is a host").
--
-- Until now an application required a machine (`machine_id` NOT NULL), because a
-- machine was the only host there was. A cluster is the second kind of host, so
-- `machine_id` becomes nullable, a `kubernetes_cluster_id` joins it, and a
-- CHECK holds exactly one of the two: an application always has a host, and
-- never two.
--
-- A cluster belongs to no group and carries applications of many groups at
-- once, so a cluster-hosted application takes its group from the namespace it is
-- deployed in rather than from its host. That is why the group derivation below
-- is guarded rather than dropped: a machine-hosted application still takes its
-- machine's group, and a cluster-hosted one keeps the group it was given.

ALTER TABLE applications
	ALTER COLUMN machine_id DROP NOT NULL;

ALTER TABLE applications
	ADD COLUMN kubernetes_cluster_id UUID
		REFERENCES kubernetes_clusters (id) ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE applications
	ADD CONSTRAINT applications_one_host
	CHECK (num_nonnulls(machine_id, kubernetes_cluster_id) = 1);

CREATE INDEX applications_kubernetes_cluster_id
	ON applications (kubernetes_cluster_id)
	WHERE kubernetes_cluster_id IS NOT NULL;

-- Guard the machine-group derivation so it leaves a cluster-hosted application's
-- group alone. Without the guard, updating a cluster application (which has no
-- machine) would run `SELECT m.group_id ... WHERE m.id = NULL`, find nothing,
-- and blank the group the namespace gave it. A machine-hosted application still
-- takes its machine's group exactly as before.
CREATE OR REPLACE FUNCTION applications_take_machine_group() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	IF NEW.machine_id IS NOT NULL THEN
		SELECT m.group_id INTO NEW.group_id FROM machines m WHERE m.id = NEW.machine_id;
	END IF;
	RETURN NEW;
END;
$$;
