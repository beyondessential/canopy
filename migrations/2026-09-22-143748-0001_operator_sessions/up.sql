-- A client session on the administrative surface and the safety mode it is in
-- (see the SAFE spec). The server holds the mode so a client reaches no further
-- than its session's mode however it describes itself; holding it here rather
-- than in a signed token is what lets a raise be ended before its expiry and
-- lets the live sessions be enumerated. The mode survives a restart because it
-- is a row.
CREATE TABLE operator_sessions (
	-- The session identifier the client carries in a request header. Minted by
	-- the server, so a client cannot forge one belonging to another login.
	id UUID PRIMARY KEY,
	-- The login this session belongs to. A session is usable only by its login.
	login TEXT NOT NULL,
	-- The session's current mode: 'read-only', 'write', or 'danger'. Every
	-- session begins read-only.
	mode TEXT NOT NULL DEFAULT 'read-only',
	-- When the current raise lapses, ten minutes after it was made. NULL while
	-- the session is read-only. The mode returns to read-only once this passes,
	-- whatever the operator has been doing in the meantime.
	raise_expires_at TIMESTAMPTZ,
	-- Idle liveness, separate from the raise: bumped on every request, swept
	-- when it falls too far behind so stale sessions do not accumulate.
	last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The idle sweep retires sessions ordered by how long since they were seen.
CREATE INDEX operator_sessions_last_seen_at ON operator_sessions (last_seen_at);
-- Enumerating the sessions a login holds (one operator can hold several).
CREATE INDEX operator_sessions_login ON operator_sessions (login);
