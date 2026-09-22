-- Restore the machine-group derivation to its unguarded form.
CREATE OR REPLACE FUNCTION applications_take_machine_group() RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
	SELECT m.group_id INTO NEW.group_id FROM machines m WHERE m.id = NEW.machine_id;
	RETURN NEW;
END;
$$;

DROP INDEX applications_kubernetes_cluster_id;

ALTER TABLE applications DROP CONSTRAINT applications_one_host;

ALTER TABLE applications DROP COLUMN kubernetes_cluster_id;

ALTER TABLE applications
	ALTER COLUMN machine_id SET NOT NULL;
