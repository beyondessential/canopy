-- How many whole days past its open an incident has already sent a reminder
-- for. The reminder sweep compares it with the incident's age, so a reminder
-- is enqueued once per day however often the sweep runs.
ALTER TABLE incidents ADD COLUMN reminders_sent INTEGER NOT NULL DEFAULT 0;
