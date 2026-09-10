-- A registration asks whether a run has already reported for somebody else,
-- which counts the checks carrying that run. The table is an audit trail kept
-- indefinitely and nothing else indexes run_id, so the count reads every row
-- ever reported.
CREATE INDEX backup_restore_checks_run
	ON backup_restore_checks (run_id)
	WHERE run_id IS NOT NULL;
