-- Whether a machine is managed by Ansible is set on the machine alone. A value
-- set on a group or an environment is carried onto each of its machines that
-- sets none of its own, the environment's over the group's, then dropped.
INSERT INTO ansible_variables (machine_id, name, value, is_secret, set_by)
SELECT DISTINCT ON (machines.id)
	machines.id, wide.name, wide.value, false, wide.set_by
FROM ansible_variables wide
JOIN machines
	ON machines.group_id = wide.server_group_id
	AND (wide.rank IS NULL OR machines.rank = wide.rank)
WHERE wide.name = 'managed_by_ansible'
	AND wide.machine_id IS NULL
	AND NOT wide.is_secret
	AND machines.deleted_at IS NULL
	AND NOT EXISTS (
		SELECT 1 FROM ansible_variables own
		WHERE own.machine_id = machines.id AND own.name = wide.name
	)
ORDER BY machines.id, wide.rank IS NULL;

DELETE FROM ansible_variables
WHERE name = 'managed_by_ansible' AND machine_id IS NULL;
