-- Ranks inherited from a box's other applications and incidents moved to a
-- headline environment stay as they are: the model before this one reads them
-- as ordinary ranks and ordinary environment incidents.
ALTER TABLE incidents DROP CONSTRAINT incidents_group_has_rank;
CREATE UNIQUE INDEX incidents_open_by_group ON incidents (server_group_id)
	WHERE closed_at IS NULL AND server_group_id IS NOT NULL AND rank IS NULL;
ALTER TABLE applications DROP CONSTRAINT applications_one_rank_per_machine;
