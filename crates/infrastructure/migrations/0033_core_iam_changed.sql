-- `core.iam.changed` (quality-sdk decision 0017, step 4; decision 0002's
-- step 11): the core publishes one event of its own topic at version 1 for
-- every statement that changes a policy, a group, a group's members or an
-- attachment, in the same transaction, so it is stored exactly when the
-- change commits and once whichever replica made it. A module that reads it
-- drops what it cached of `/v1/iam/effective`. `data` names only what kind
-- of thing changed: `{"kind": "policy" | "group" | "membership" |
-- "attachment"}`, never who or what. The notice wakes the reads that wait.
-- An expansion: a function and four statement triggers.
CREATE FUNCTION public.publish_core_iam_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO public.module_events (id, topic, version, publisher, occurred_at, data)
    VALUES (gen_random_uuid(), 'core.iam.changed', 1, 'core', now(),
            jsonb_build_object('kind', TG_ARGV[0]));
    PERFORM pg_notify('module_events', 'core.iam.changed');
    RETURN NULL;
END;
$$;

CREATE TRIGGER publish_core_iam_changed
    AFTER INSERT OR UPDATE OR DELETE ON public.iam_policies
    FOR EACH STATEMENT EXECUTE FUNCTION public.publish_core_iam_changed('policy');
CREATE TRIGGER publish_core_iam_changed
    AFTER INSERT OR UPDATE OR DELETE ON public.iam_groups
    FOR EACH STATEMENT EXECUTE FUNCTION public.publish_core_iam_changed('group');
CREATE TRIGGER publish_core_iam_changed
    AFTER INSERT OR UPDATE OR DELETE ON public.iam_group_members
    FOR EACH STATEMENT EXECUTE FUNCTION public.publish_core_iam_changed('membership');
CREATE TRIGGER publish_core_iam_changed
    AFTER INSERT OR UPDATE OR DELETE ON public.iam_attachments
    FOR EACH STATEMENT EXECUTE FUNCTION public.publish_core_iam_changed('attachment');
