-- What the replication of the main lake into the replica left of each
-- tenant's warehouse (decision 0066 step 11, the user's rulings of
-- 2026-11-13): configured, not on the source yet, a lake's keys not kept,
-- or the step that failed and what happened, in a few words, never a key.
-- One row per warehouse, replaced at each run; when it was first found
-- configured is kept through later failures. Shown in Settings → Lake.

CREATE TABLE public.lake_replication (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    warehouse text NOT NULL CHECK (warehouse ~ '^qd-tenant-[a-z0-9-]{1,53}$'),
    state text NOT NULL CHECK (state IN ('configured', 'no_warehouse', 'no_keys', 'failed')),
    -- With `failed`: the step; with `no_keys`: the lake (`replica`, `pool:<name>`).
    step text,
    detail text,
    configured_at timestamp with time zone,
    checked_at timestamp with time zone NOT NULL
);

CREATE UNIQUE INDEX lake_replication_warehouse ON public.lake_replication USING btree (tenant_id, warehouse);

ALTER TABLE public.lake_replication ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.lake_replication FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.lake_replication USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
