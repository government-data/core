-- The capabilities the module of a client requires, from its manifest, as
-- the update agent registers its machine client (decision 0090, ruling
-- 109): the core takes its internal quality routes from the machine client
-- of a module that requires orchestration. An expansion: a nullable column,
-- empty for a client registered before it until its module registers again.
ALTER TABLE public.oidc_clients
    ADD COLUMN requires text[];
