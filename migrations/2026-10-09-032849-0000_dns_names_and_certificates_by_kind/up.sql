-- DNS addresses and TLS certificates are separate features that share
-- infrastructure (DNS, "Declared DNS names"): each has its own declarations,
-- its own undeclared records, and its own denials.

-- The names an application holds for certificates. `application_names` is the
-- address side only from here on.
--
-- Not the same as `application_certificates`, which holds orders and chains:
-- a declaration exists before any order and outlives its release.
CREATE TABLE application_certificate_names (
	id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	application_id UUID NOT NULL REFERENCES applications(id),
	-- Normalised: lower case, no trailing dot.
	name           TEXT NOT NULL,
	created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One application holds a name for certificates. That it is also the one
-- holding it for addresses is the trigger's business, below.
CREATE UNIQUE INDEX application_certificate_names_name ON application_certificate_names (name);
CREATE INDEX application_certificate_names_application ON application_certificate_names (application_id);

-- Canopy DNS is not in use, so every existing declaration was made for a
-- certificate.
INSERT INTO application_certificate_names (application_id, name, created_at)
SELECT application_id, name, created_at FROM application_names;

-- A row that registered addresses, or is still withdrawing records already
-- published, is address-side data and stays. The rest was only a declaration.
DELETE FROM application_names
WHERE cardinality(addresses) = 0
  AND cardinality(published_addresses) = 0;

-- A name is held by one application across both kinds, so application A cannot
-- have the addresses while B has the certificate. Each table's unique index
-- covers its own kind; this covers the other. The advisory lock serialises two
-- declarations of the same name, which would otherwise each see the other's
-- table empty and both win.
CREATE FUNCTION enforce_one_holder_per_dns_name() RETURNS trigger AS $$
DECLARE
	holder UUID;
BEGIN
	PERFORM pg_advisory_xact_lock(hashtextextended('dns-name-holder:' || NEW.name, 0));
	IF TG_TABLE_NAME = 'application_names' THEN
		SELECT application_id INTO holder FROM application_certificate_names
		WHERE name = NEW.name AND application_id <> NEW.application_id
		LIMIT 1;
	ELSE
		SELECT application_id INTO holder FROM application_names
		WHERE name = NEW.name AND application_id <> NEW.application_id
		LIMIT 1;
	END IF;
	IF holder IS NOT NULL THEN
		RAISE EXCEPTION 'DNS name % is held by another application', NEW.name
			USING ERRCODE = 'unique_violation';
	END IF;
	RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER application_names_one_holder
	BEFORE INSERT OR UPDATE OF name, application_id ON application_names
	FOR EACH ROW EXECUTE FUNCTION enforce_one_holder_per_dns_name();
CREATE TRIGGER application_certificate_names_one_holder
	BEFORE INSERT OR UPDATE OF name, application_id ON application_certificate_names
	FOR EACH ROW EXECUTE FUNCTION enforce_one_holder_per_dns_name();

-- Undeclared requests are one per machine, DNS name and kind.
ALTER TABLE undeclared_dns_names RENAME COLUMN asked_for TO kind;
ALTER TABLE undeclared_dns_names DROP CONSTRAINT undeclared_dns_names_machine_id_dns_name_key;
ALTER TABLE undeclared_dns_names ADD CONSTRAINT undeclared_dns_names_machine_id_dns_name_kind_key
	UNIQUE (machine_id, dns_name, kind);

-- Denials are too. Every existing denial was of a certificate.
ALTER TABLE denied_dns_names
	ADD COLUMN kind TEXT NOT NULL DEFAULT 'certificate'
	CHECK (kind IN ('addresses', 'certificate'));
ALTER TABLE denied_dns_names ALTER COLUMN kind DROP DEFAULT;
ALTER TABLE denied_dns_names DROP CONSTRAINT denied_dns_names_machine_id_dns_name_key;
ALTER TABLE denied_dns_names ADD CONSTRAINT denied_dns_names_machine_id_dns_name_kind_key
	UNIQUE (machine_id, dns_name, kind);
