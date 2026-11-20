-- RustFS's root keys of each lake the application installs (decision 0066,
-- step 11, the user's ruling of 2026-11-13): one pair per lake, the
-- replica's or a pool's, made by the application the first time a node asks
-- for them with its join token, and given to every node of that lake (a
-- pool's nodes must hold the same). The secret is sealed with the settings'
-- key (SETTINGS_ENCRYPTION_KEY or SETTINGS_KEY_FILE), as the global secrets
-- are; it is never answered by any other route nor logged.

CREATE TABLE public.lake_root_keys (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    -- `replica`, or `pool:<name>`.
    lake text NOT NULL CHECK (lake ~ '^(replica|pool:[a-z0-9-]{1,63})$'),
    access_key text NOT NULL,
    -- Sealed (SecretBox), never in clear.
    secret_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE UNIQUE INDEX lake_root_keys_lake ON public.lake_root_keys USING btree (tenant_id, lake);

ALTER TABLE public.lake_root_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.lake_root_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.lake_root_keys USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
