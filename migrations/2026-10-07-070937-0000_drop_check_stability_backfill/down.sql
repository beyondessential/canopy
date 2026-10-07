-- Recreated empty: an older binary rolled back onto this would see no marker
-- and replay the backfill, which converges on rows that already exist.
CREATE TABLE check_stability_backfill (
	done_at TIMESTAMPTZ NOT NULL DEFAULT NOW() PRIMARY KEY
);
