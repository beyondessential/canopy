-- ── Who may publish a group's reporting schema ──────────────────────────────
--
-- Publishing a group-scoped artifact is a privilege over every machine in the
-- group: they are offered what is registered and they run it. The intent
-- semantics a consumer advertises are the consumer's own claim, registered by
-- the device itself, so they shape dispatch but cannot be what grants this.
-- An operator sets this flag on the declaration through the admin API, and it
-- is the whole of the authorisation.
ALTER TABLE restore_replicas
	ADD COLUMN publishes_schemas BOOLEAN NOT NULL DEFAULT FALSE;
