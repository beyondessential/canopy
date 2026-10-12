ALTER TABLE denied_dns_names DROP CONSTRAINT denied_dns_names_machine_id_dns_name_kind_key;
DELETE FROM denied_dns_names a USING denied_dns_names b
WHERE a.machine_id = b.machine_id AND a.dns_name = b.dns_name
  AND (a.created_at, a.id) < (b.created_at, b.id);
ALTER TABLE denied_dns_names DROP COLUMN kind;
ALTER TABLE denied_dns_names ADD CONSTRAINT denied_dns_names_machine_id_dns_name_key
	UNIQUE (machine_id, dns_name);

ALTER TABLE undeclared_dns_names DROP CONSTRAINT undeclared_dns_names_machine_id_dns_name_kind_key;
DELETE FROM undeclared_dns_names a USING undeclared_dns_names b
WHERE a.machine_id = b.machine_id AND a.dns_name = b.dns_name
  AND (a.last_asked_at, a.id) < (b.last_asked_at, b.id);
ALTER TABLE undeclared_dns_names RENAME COLUMN kind TO asked_for;
ALTER TABLE undeclared_dns_names ADD CONSTRAINT undeclared_dns_names_machine_id_dns_name_key
	UNIQUE (machine_id, dns_name);

DROP TRIGGER application_certificate_names_one_holder ON application_certificate_names;
DROP TRIGGER application_names_one_holder ON application_names;
DROP FUNCTION enforce_one_holder_per_dns_name();

INSERT INTO application_names (application_id, name, created_at)
SELECT c.application_id, c.name, c.created_at
FROM application_certificate_names c
WHERE NOT EXISTS (SELECT 1 FROM application_names n WHERE n.name = c.name);

DROP TABLE application_certificate_names;
