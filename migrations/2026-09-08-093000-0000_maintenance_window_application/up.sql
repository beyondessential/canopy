-- A window can cover one application. A box running two products is worked on
-- one product at a time: stopping the Tamanu server on a host that also runs
-- mSupply left the only available declaration, the machine's, taking mSupply
-- out of alerting with it.
ALTER TABLE maintenance_windows
	ADD COLUMN application_id UUID REFERENCES applications (id) ON DELETE CASCADE;

ALTER TABLE maintenance_windows
	DROP CONSTRAINT maintenance_windows_one_target;
ALTER TABLE maintenance_windows
	ADD CONSTRAINT maintenance_windows_one_target
	CHECK (num_nonnulls(application_id, machine_id, server_group_id) = 1);

CREATE UNIQUE INDEX maintenance_windows_one_open_per_application
	ON maintenance_windows (application_id)
	WHERE ended_at IS NULL AND application_id IS NOT NULL;
CREATE INDEX maintenance_windows_application
	ON maintenance_windows (application_id, declared_at DESC);
