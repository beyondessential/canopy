-- ── One publisher per group ─────────────────────────────────────────────────
--
-- What a builder registers is offered to every machine in the group, and a
-- registration replaces whatever is already registered for the same version,
-- type, platform and group. Two enabled declarations publishing for one group
-- are therefore both dispatched the same pairs and each overwrite the other's
-- schema, with which one a machine ends up on decided by whichever reported
-- last. The mark is the operator's, so the operator holds it to one.
CREATE UNIQUE INDEX restore_replicas_one_schema_publisher
	ON restore_replicas (group_id)
	WHERE publishes_schemas AND enabled;
