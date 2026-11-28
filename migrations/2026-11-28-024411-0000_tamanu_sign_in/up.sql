-- When each key last authenticated over mutual TLS, so a sign-in token is
-- encrypted to the key the box's agent holds rather than to one provisioned to
-- be carried elsewhere.
ALTER TABLE device_keys ADD COLUMN last_authenticated_at TIMESTAMPTZ;

-- What each source declares an application and its reporter accept, replaced
-- with every push from that source.
ALTER TABLE application_reported_detail
	ADD COLUMN capabilities TEXT[] NOT NULL DEFAULT '{}';

-- Each key's sign-in tokens, newest first, read beside its identity tokens for
-- the settings page's figures and the keys' states.
CREATE INDEX audit_entries_tamanu_sign_in_by_kid
	ON audit_entries ((detail ->> 'kid'), recorded_at DESC)
	WHERE action = 'tamanu_sign_in_issued' AND (outcome IS NULL OR outcome = 'done');

-- Sign-ins per person over the rate window.
CREATE INDEX audit_entries_tamanu_sign_in_by_actor
	ON audit_entries (actor_id, recorded_at DESC)
	WHERE action = 'tamanu_sign_in_issued';
