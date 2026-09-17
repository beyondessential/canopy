CREATE OR REPLACE FUNCTION update_server_group_effective_version()
RETURNS trigger
LANGUAGE plpgsql
AS $function$
BEGIN
    IF NEW.version IS NOT NULL THEN
        UPDATE server_groups
        SET effective_version = NEW.version, updated_at = now()
        WHERE version_application_id = NEW.server_id;
    END IF;
    RETURN NEW;
END;
$function$;
