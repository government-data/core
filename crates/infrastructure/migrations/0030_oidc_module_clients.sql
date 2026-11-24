-- The clients a module registers at `module add` (quality-sdk decision
-- 0009): the module (null for one registered by hand, which a module never
-- takes over), when it was disabled (the module removed: no code, no
-- token), the secret it replaced and until when that one is still taken (a
-- rotation, at most an hour), the claim whose values come from the
-- person's actions, the capabilities its machine client consumes and where
-- a person may land after signing out. An expansion: nullable columns.
ALTER TABLE public.oidc_clients
    ADD COLUMN module text,
    ADD COLUMN disabled_at timestamp with time zone,
    ADD COLUMN previous_secret_hash text,
    ADD COLUMN previous_secret_until timestamp with time zone,
    ADD COLUMN role_claim jsonb,
    ADD COLUMN consumes text[],
    ADD COLUMN post_logout_redirect_uris text[];
