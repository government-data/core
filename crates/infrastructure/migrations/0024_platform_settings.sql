-- The platform's settings (decision 0079): a Settings section `platform`
-- with the installation's time zone. The section check is widened, every
-- section allowed before still is.
ALTER TABLE public.system_settings
    DROP CONSTRAINT system_settings_section_check,
    ADD CONSTRAINT system_settings_section_check
        CHECK (section = ANY (ARRAY['ldap'::text, 'mail'::text, 'oauth'::text, 'platform'::text]));

-- The platform's time zone as saved, or NULL when it was never saved; the
-- one who reads it falls back to UTC when it is NULL or does not parse.
-- Read under the caller's row security, as the table itself.
CREATE FUNCTION public.platform_timezone() RETURNS text
    LANGUAGE sql STABLE SET search_path = pg_catalog, pg_temp AS $$
    SELECT nullif(btrim(data ->> 'timezone'), '')
      FROM public.system_settings
     WHERE section = 'platform'
$$;
