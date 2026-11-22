-- Whether the installation is in maintenance now (decision 0003): a module
-- that hands out work asks it and hands out none while it lasts, so what is
-- queued waits for the update instead of starting under it. As
-- `UpdatesService::active`: in force until its `until`. SECURITY DEFINER so
-- a role that may not read `orq_maintenance` can still ask; no one may run
-- it unless granted (a module grants its own queue role).
CREATE FUNCTION public.in_maintenance() RETURNS boolean
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS $$
    SELECT EXISTS (SELECT 1 FROM public.orq_maintenance WHERE until > now())
$$;

REVOKE EXECUTE ON FUNCTION public.in_maintenance() FROM PUBLIC;
