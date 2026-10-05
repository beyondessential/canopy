-- A machine always has a name. Name any that lack one from what the box is
-- known by: its reported hostname, its tailnet node, the first named
-- application on it, and failing all of those a plain placeholder.
UPDATE machines m
SET name = coalesce(
	(
		SELECT nullif(btrim(d.extra->>'hostname'), '')
		FROM machine_reported_detail d
		WHERE d.machine_id = m.id AND nullif(btrim(d.extra->>'hostname'), '') IS NOT NULL
		ORDER BY d.reported_at DESC
		LIMIT 1
	),
	(
		SELECT nullif(btrim(dv.tailscale_node_name), '')
		FROM devices dv
		WHERE dv.id = m.device_id
	),
	(
		SELECT nullif(btrim(a.name), '')
		FROM applications a
		WHERE a.machine_id = m.id AND nullif(btrim(a.name), '') IS NOT NULL
		ORDER BY a.created_at
		LIMIT 1
	),
	'Unnamed machine'
)
WHERE m.name IS NULL OR btrim(m.name) = '';

ALTER TABLE machines
	ALTER COLUMN name SET NOT NULL,
	ADD CONSTRAINT machines_name_not_blank CHECK (btrim(name) <> '');
