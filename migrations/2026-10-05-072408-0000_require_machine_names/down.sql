-- The names given on the way up are indistinguishable from operators' own, so
-- they stay.
ALTER TABLE machines
	DROP CONSTRAINT machines_name_not_blank,
	ALTER COLUMN name DROP NOT NULL;
