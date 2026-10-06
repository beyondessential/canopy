DROP TRIGGER machine_rank_propagates ON machines;
DROP FUNCTION machine_rank_propagates();
DROP TRIGGER application_rank_ranks_machine ON applications;
DROP FUNCTION application_rank_ranks_machine();
DROP TRIGGER applications_take_machine_rank ON applications;
DROP FUNCTION applications_take_machine_rank();
ALTER TABLE machines DROP COLUMN rank;
