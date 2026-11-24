-- Each module's actions and managed policies as the update agent last told
-- them (quality-sdk decision 0009, `PUT /updater/v1/modules`): an object by
-- module name. A module the list no longer names keeps its entry (its
-- policies and their assignments stay while it is removed); only a purge
-- deletes it. An expansion: a nullable column on the updates' single row.
ALTER TABLE public.orq_updater
    ADD COLUMN module_permissions jsonb;

-- What someone can do changes with the modules' policies: whoever keeps
-- Actors in memory is told, for every tenant (an empty payload).
CREATE FUNCTION public.notify_module_permissions() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_notify('permissions', '');
    RETURN NULL;
END;
$$;

CREATE TRIGGER notify_module_permissions
    AFTER UPDATE OF modules, module_permissions ON public.orq_updater
    FOR EACH ROW EXECUTE FUNCTION public.notify_module_permissions();
