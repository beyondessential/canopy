-- Pushes repeat the version already cached, and each write locks the group row
-- against anything else taking it.
CREATE OR REPLACE FUNCTION update_server_group_effective_version()
RETURNS trigger
LANGUAGE plpgsql
AS $function$
BEGIN
    IF NEW.version IS NOT NULL THEN
        UPDATE server_groups
        SET effective_version = NEW.version, updated_at = now()
        WHERE version_application_id = NEW.server_id
          AND effective_version IS DISTINCT FROM NEW.version;
    END IF;
    RETURN NEW;
END;
$function$;
