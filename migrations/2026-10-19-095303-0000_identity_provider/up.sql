-- Canopy as an OpenID Connect issuer (IDP, IDT): the relying parties tokens are
-- issued for, which machines and applications may obtain them, and what Canopy
-- has observed of its signing keys.
--
-- Who registered, granted or revoked what, and every token issued, are the
-- audit log's (AUD), so none of these tables carries who/when columns of its
-- own.

CREATE TABLE relying_parties (
	id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	name       TEXT NOT NULL CHECK (btrim(name) <> ''),
	-- What a token carries as `aud` and what a machine names when it asks for
	-- one, so two relying parties never share it. Fixed once registered.
	audience   TEXT NOT NULL UNIQUE CHECK (btrim(audience) <> ''),
	token_lifetime_minutes INTEGER NOT NULL DEFAULT 5
		CHECK (token_lifetime_minutes BETWEEN 1 AND 60),
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

SELECT diesel_manage_updated_at('relying_parties');

CREATE FUNCTION relying_parties_audience_fixed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
	IF NEW.audience IS DISTINCT FROM OLD.audience THEN
		RAISE EXCEPTION 'a relying party''s audience is fixed once registered';
	END IF;
	RETURN NEW;
END;
$$;

CREATE TRIGGER relying_parties_audience_fixed
	BEFORE UPDATE ON relying_parties
	FOR EACH ROW EXECUTE FUNCTION relying_parties_audience_fixed();

-- A grant lets one machine, or one application, obtain tokens for one relying
-- party. A machine's grant covers only tokens naming the machine, and an
-- application's only tokens naming that application.
CREATE TABLE identity_grants (
	id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	relying_party_id UUID NOT NULL REFERENCES relying_parties (id) ON DELETE CASCADE,
	machine_id       UUID REFERENCES machines (id) ON DELETE CASCADE,
	application_id   UUID REFERENCES applications (id) ON DELETE CASCADE,
	CONSTRAINT identity_grants_one_subject
		CHECK (num_nonnulls(machine_id, application_id) = 1)
);

CREATE UNIQUE INDEX identity_grants_machine
	ON identity_grants (machine_id, relying_party_id) WHERE machine_id IS NOT NULL;
CREATE UNIQUE INDEX identity_grants_application
	ON identity_grants (application_id, relying_party_id) WHERE application_id IS NOT NULL;
CREATE INDEX identity_grants_relying_party ON identity_grants (relying_party_id);

-- What Canopy has seen of each signing key, by its `kid`. Each moment is
-- written once and stands: when the key was first observed served at the
-- issuer, which is what it matures from, and when it first signed, which is what
-- keeps signing from ever moving back to an older key.
--
-- A reference can be an alias that is later pointed at another key, so several
-- rows can carry the same `key_ref`; the one known most recently is the key the
-- reference names now.
CREATE TABLE identity_keys (
	kid             TEXT PRIMARY KEY,
	key_ref         TEXT NOT NULL,
	known_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
	first_served_at TIMESTAMPTZ,
	first_signed_at TIMESTAMPTZ
);

CREATE FUNCTION identity_keys_written_once() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
	IF OLD.first_served_at IS NOT NULL
		AND NEW.first_served_at IS DISTINCT FROM OLD.first_served_at THEN
		RAISE EXCEPTION 'a key''s first served moment is written once';
	END IF;
	IF OLD.first_signed_at IS NOT NULL
		AND NEW.first_signed_at IS DISTINCT FROM OLD.first_signed_at THEN
		RAISE EXCEPTION 'a key''s first signed moment is written once';
	END IF;
	RETURN NEW;
END;
$$;

CREATE TRIGGER identity_keys_written_once
	BEFORE UPDATE ON identity_keys
	FOR EACH ROW EXECUTE FUNCTION identity_keys_written_once();
