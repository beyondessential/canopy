-- The project a group serves, the PM in charge of it, and its contact on the
-- ground. An empty detail is null, never an empty string.
ALTER TABLE server_groups
	ADD COLUMN project TEXT CHECK (project <> ''),
	ADD COLUMN pm_in_charge TEXT CHECK (pm_in_charge <> ''),
	ADD COLUMN ground_contact TEXT CHECK (ground_contact <> '');
