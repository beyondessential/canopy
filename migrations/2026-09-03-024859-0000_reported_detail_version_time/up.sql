-- When a source last carried a version, apart from when it last pushed at all:
-- a push without one keeps the version it reported before, and the newest
-- version across sources is the one most recently carried, not the one whose
-- source spoke most recently.
ALTER TABLE application_reported_detail ADD COLUMN version_reported_at TIMESTAMPTZ;
UPDATE application_reported_detail SET version_reported_at = reported_at WHERE version IS NOT NULL;
