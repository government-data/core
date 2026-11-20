-- A lake's pools (decision 0029, points 5 and 6; 0066 step 11, the user's
-- rulings 1 to 3 of 2026-11-14): written in Settings → Lake before any of
-- the pool's nodes exists. One row per pool: its nodes' names in order,
-- each a host's name its peers resolve and its certificate carries, and the
-- drives of each node. The layout every node is given (RUSTFS_VOLUMES) is
-- derived from this row, by the application alone.

CREATE TABLE public.lake_pools (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (name ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
    -- In order; at least four (0029, point 5), at most sixteen.
    nodes text[] NOT NULL CHECK (cardinality(nodes) BETWEEN 4 AND 16),
    drives integer NOT NULL CHECK (drives BETWEEN 1 AND 16),
    -- The person who defined it last, and when.
    defined_by uuid NOT NULL,
    defined_at timestamp with time zone NOT NULL
);

CREATE UNIQUE INDEX lake_pools_name ON public.lake_pools USING btree (tenant_id, name);

ALTER TABLE public.lake_pools ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.lake_pools FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.lake_pools USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));

-- The node a lake pool's join token is for (ruling 1): joining refuses a
-- machine of any other name. Empty for every other role's token.
ALTER TABLE public.join_tokens ADD COLUMN node text CHECK (node ~ '^[a-z0-9-]{1,63}$');
