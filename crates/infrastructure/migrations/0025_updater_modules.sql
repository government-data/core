-- The installed modules as the update agent tells them (decision 0003,
-- `PUT /updater/v1/modules`): the whole list, replaced each time, each with
-- its name, version and drain path (or null). The drain asks the modules
-- that have a drain path. An expansion: a nullable column on the updates'
-- single row.
ALTER TABLE public.orq_updater
    ADD COLUMN modules jsonb,
    ADD COLUMN modules_at timestamp with time zone;
