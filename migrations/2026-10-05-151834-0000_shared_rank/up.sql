-- A box's applications share one rank, so a machine is in the environment that
-- rank names, and an application is unranked only while it is pending: new on a
-- box where nothing is ranked yet.

-- Ranks are written canonical, so the constraint below compares spellings that
-- mean the same thing as equal. A spelling no one recognises is not a rank the
-- application can be read with, so it becomes unranked here.
UPDATE applications SET rank = CASE lower(rank)
	WHEN 'production' THEN 'production'
	WHEN 'live' THEN 'production'
	WHEN 'prod' THEN 'production'
	WHEN 'clone' THEN 'clone'
	WHEN 'staging' THEN 'clone'
	WHEN 'demo' THEN 'demo'
	WHEN 'test' THEN 'test'
	WHEN 'dev' THEN 'dev'
END
WHERE rank IS NOT NULL;

-- Every live application takes the highest rank among those on its box: what the
-- box's environment has always been derived as, so no incident or window changes
-- environment under anyone, and an unranked sidecar beside a ranked central joins
-- the central's environment.
WITH box AS (
	SELECT machine_id,
		(array_agg(rank ORDER BY array_position(
			ARRAY['production', 'clone', 'demo', 'test', 'dev'], rank)))[1] AS rank
	FROM applications
	WHERE machine_id IS NOT NULL AND deleted_at IS NULL AND rank IS NOT NULL
	GROUP BY machine_id
)
UPDATE applications SET rank = box.rank
FROM box
WHERE applications.machine_id = box.machine_id
	AND applications.deleted_at IS NULL
	AND applications.rank IS DISTINCT FROM box.rank;

-- Held in the schema, so no write can leave a box carrying two ranks or a
-- ranked application beside a pending one. Deferrable so that ranking a whole
-- box in one statement is checked once the statement is done rather than as
-- each of its rows is changed.
CREATE EXTENSION IF NOT EXISTS btree_gist;
ALTER TABLE applications
	ADD CONSTRAINT applications_one_rank_per_machine
	EXCLUDE USING gist (machine_id WITH =, (COALESCE(rank, '')) WITH <>)
	WHERE (machine_id IS NOT NULL AND deleted_at IS NULL)
	DEFERRABLE INITIALLY IMMEDIATE;

-- A group's own checks belong to its headline environment, the highest rank
-- among its live applications, so no incident targets a group itself any more.
CREATE TEMPORARY TABLE group_headline ON COMMIT DROP AS
SELECT group_id, (array_agg(rank ORDER BY array_position(
		ARRAY['production', 'clone', 'demo', 'test', 'dev'], rank)))[1] AS rank
FROM applications
WHERE group_id IS NOT NULL AND deleted_at IS NULL AND rank IS NOT NULL
GROUP BY group_id;

-- An open one that cannot move closes, leaving its issues to join whichever
-- incident they now belong to when the monitor reconciles on startup: either
-- there is no headline environment to move it to, or that environment has an
-- incident open already, and at most one is open per environment.
CREATE TEMPORARY TABLE group_incidents_closing ON COMMIT DROP AS
SELECT i.id
FROM incidents i
LEFT JOIN group_headline h ON h.group_id = i.server_group_id
WHERE i.server_group_id IS NOT NULL AND i.rank IS NULL AND i.closed_at IS NULL
	AND (h.rank IS NULL OR EXISTS (
		SELECT 1 FROM incidents o
		WHERE o.server_group_id = i.server_group_id AND o.rank = h.rank
			AND o.closed_at IS NULL));

UPDATE incident_issues SET left_at = NOW()
WHERE left_at IS NULL AND incident_id IN (SELECT id FROM group_incidents_closing);

-- Slack hears of the close the way it would of any other: an open still waiting
-- to be delivered is cancelled, as is a reminder, and an incident whose open was
-- delivered gets its resolve, naming the group as the incident always did.
UPDATE slack_outbox
SET gave_up_at = NOW(),
	last_error = 'cancelled: group-target incident closed by the shared rank migration'
WHERE gave_up_at IS NULL
	AND delivered_at IS NULL
	AND kind IN ('incident_open', 'incident_reminder')
	AND incident_id IN (SELECT id FROM group_incidents_closing);

INSERT INTO slack_outbox (kind, incident_id, payload, deliver_after)
SELECT 'incident_resolve', i.id,
	jsonb_build_object(
		'server', g.name,
		'by', 'Canopy, which no longer opens an incident on a group itself'),
	NOW()
FROM incidents i
JOIN server_groups g ON g.id = i.server_group_id
WHERE i.id IN (SELECT id FROM group_incidents_closing)
	AND EXISTS (
		SELECT 1 FROM slack_outbox o
		WHERE o.incident_id = i.id
			AND o.kind = 'incident_open'
			AND o.delivered_at IS NOT NULL);

UPDATE incidents SET closed_at = NOW(), closing_at = NULL, updated_at = NOW()
WHERE id IN (SELECT id FROM group_incidents_closing);

-- The rest move to the headline environment, keeping their timeline. A closed
-- incident of a group that never had a ranked application is placed at
-- production, the environment such a group was given for its plans.
UPDATE incidents SET rank = COALESCE(
	(SELECT h.rank FROM group_headline h WHERE h.group_id = incidents.server_group_id),
	'production')
WHERE server_group_id IS NOT NULL AND rank IS NULL;

DROP INDEX incidents_open_by_group;
ALTER TABLE incidents
	ADD CONSTRAINT incidents_group_has_rank
		CHECK (server_group_id IS NULL OR rank IS NOT NULL);
