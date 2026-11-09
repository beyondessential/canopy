-- A reason is a sentence or two. NOT VALID holds what is written from now on
-- to the limit and leaves the longer reasons already kept as they are.
ALTER TABLE vulnerability_decision_entries
	ADD CONSTRAINT vulnerability_decision_entries_reason_short
	CHECK (char_length(reason) <= 500) NOT VALID;

ALTER TABLE vulnerability_decision_withdrawals
	ADD CONSTRAINT vulnerability_decision_withdrawals_reason_short
	CHECK (char_length(reason) <= 500) NOT VALID;

ALTER TABLE vulnerability_decision_edits
	ADD CONSTRAINT vulnerability_decision_edits_reason_short
	CHECK (char_length(reason) <= 500) NOT VALID;
