DROP TABLE machine_reported_timezone;
DROP TABLE backup_schedule_history;
DROP TABLE machine_backup_schedule;

ALTER TABLE server_group_backup_schedule
	DROP CONSTRAINT server_group_backup_schedule_shape,
	DROP COLUMN has_schedule,
	DROP COLUMN schedule_zone,
	DROP COLUMN expected_cron;

ALTER TABLE backup_type_defaults
	DROP CONSTRAINT backup_type_defaults_schedule_shape,
	DROP COLUMN default_zone,
	DROP COLUMN default_cron;
