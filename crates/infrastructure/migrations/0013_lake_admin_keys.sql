-- The platform's admin user's keys of each lake (decision 0028, point 2;
-- 0066 step 11, the user's ruling of 2026-11-13): join.sh makes the user on
-- the node when it brings the lake up and shows its keys there once; a
-- person pastes them in Settings → Lake. One pair per lake, replaced when
-- pasted again. The secret is sealed with the settings' key
-- (SETTINGS_ENCRYPTION_KEY or SETTINGS_KEY_FILE), as the root keys are; it
-- is never answered by any route, logged or audited.

CREATE TABLE public.lake_admin_keys (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    -- `replica`, or `pool:<name>`.
    lake text NOT NULL CHECK (lake ~ '^(replica|pool:[a-z0-9-]{1,63})$'),
    access_key text NOT NULL,
    -- Sealed (SecretBox), never in clear.
    secret_key text NOT NULL,
    -- The person who pasted them last, and when.
    set_by uuid NOT NULL,
    set_at timestamp with time zone NOT NULL
);

CREATE UNIQUE INDEX lake_admin_keys_lake ON public.lake_admin_keys USING btree (tenant_id, lake);

ALTER TABLE public.lake_admin_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.lake_admin_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.lake_admin_keys USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
