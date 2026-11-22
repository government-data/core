-- Who asked for an update now when no administrator did (decision 0086,
-- `POST /updater/v1/request`): the name of the update agent's machine, for
-- a person on the server. An expansion: a nullable column on the updates'
-- single row.
ALTER TABLE public.orq_updater
    ADD COLUMN requested_by_machine text;
