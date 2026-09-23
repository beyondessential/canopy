-- How long a registered cluster may go unheard before it reads unreachable,
-- as a machine's does. Five minutes is five missed refiles at the relay's
-- one-minute cadence: enough to ride out the relay's pod being rescheduled or
-- its node drained in a cluster upgrade.
ALTER TABLE kubernetes_clusters
	ADD COLUMN alert_when_down_for INTERVAL NOT NULL DEFAULT '00:05:00'::interval,
	ADD CONSTRAINT kubernetes_clusters_alert_when_down_for_check
		CHECK (alert_when_down_for > '00:00:00'::interval);
