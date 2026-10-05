-- Resolve check-states left behind at a grain their check is no longer filed
-- at, and retire the last of the broken-thread rows as check-states.
--
-- Canopy's own checks moved grains over time: the backup and restore checks
-- now file against the machine or the group, but their earlier states against
-- applications were never closed. Canopy's checks are not reports, so no later
-- report omits them and nothing ever recovers them; an active one counted
-- against its application's health for good. The checks Canopy still files
-- against an application are listed here; any other `canopy` state on an
-- application is one of these leftovers.
--
-- `merge_broken_thread` resolved only the broken-thread rows that were active
-- at the time, and left `check_name` set on all of them, so each still read as
-- a second state for its check. Clearing `check_name` takes them out of every
-- check-state read while leaving them in the issue and incident history.
--
-- spec: CHK#each-check-is-held-at-one-grain

-- 1. Canopy's own states at an abandoned grain.
UPDATE issues
SET active = false,
	resolved_at = NOW(),
	resolved_by = 'migration:2026-10-05-resolve_abandoned_grain_check_states',
	resolved_reason = 'expected',
	updated_at = NOW()
WHERE source = 'canopy'
	AND application_id IS NOT NULL
	AND check_name IS NOT NULL
	AND resolved_at IS NULL
	AND check_name NOT IN (
		'reachability',
		'certificate-expiry',
		'certificate-issuance',
		'dns-records',
		'migration-test',
		'reporting-schema'
	);

-- 2. Broken-thread rows still unresolved.
UPDATE issues
SET active = false,
	resolved_at = NOW(),
	resolved_by = 'migration:2026-10-05-resolve_abandoned_grain_check_states',
	resolved_reason = 'expected',
	updated_at = NOW()
WHERE ref LIKE 'health-broken/%'
	AND resolved_at IS NULL;

UPDATE issues
SET check_name = NULL
WHERE ref LIKE 'health-broken/%'
	AND check_name IS NOT NULL;

-- 3. Mark incident links as left for the states just resolved, so the
--    orphan-close below sees an accurate count of remaining contributors.
UPDATE incident_issues
SET left_at = NOW()
WHERE left_at IS NULL
	AND issue_id IN (
		SELECT id FROM issues
		WHERE resolved_by = 'migration:2026-10-05-resolve_abandoned_grain_check_states'
	);

-- 4. Close incidents this migration just orphaned. No Slack resolve: this is
--    a cleanup of states nothing could recover, not an operational recovery.
UPDATE incidents
SET closed_at = NOW(), updated_at = NOW()
WHERE closed_at IS NULL
	AND EXISTS (
		SELECT 1 FROM incident_issues ii
		JOIN issues i ON i.id = ii.issue_id
		WHERE ii.incident_id = incidents.id
			AND i.resolved_by = 'migration:2026-10-05-resolve_abandoned_grain_check_states'
	)
	AND NOT EXISTS (
		SELECT 1 FROM incident_issues ii
		WHERE ii.incident_id = incidents.id
			AND ii.left_at IS NULL
	);

-- 5. Cancel pending Slack opens for incidents just closed.
UPDATE slack_outbox
SET gave_up_at = NOW(),
	last_error = 'cancelled: incident closed by abandoned-grain cleanup migration'
WHERE gave_up_at IS NULL
	AND delivered_at IS NULL
	AND kind = 'incident_open'
	AND incident_id IN (
		SELECT id FROM incidents
		WHERE closed_at IS NOT NULL
			AND EXISTS (
				SELECT 1 FROM incident_issues ii
				JOIN issues i ON i.id = ii.issue_id
				WHERE ii.incident_id = incidents.id
					AND i.resolved_by = 'migration:2026-10-05-resolve_abandoned_grain_check_states'
			)
	);
