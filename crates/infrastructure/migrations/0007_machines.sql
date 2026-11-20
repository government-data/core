-- The installation's machines and the tokens they join with (decisions
-- 0029 and 0066). A join token is shown once: only its SHA-256 is kept. It
-- is used once, within its hour, and the machine it made is recorded on it.
-- A machine keeps the serial of the certificate it uses: the API accepts
-- that one and no other (0066, point 9).

CREATE TABLE public.machines (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    -- As the certificate's CN: lowercase letters, digits and hyphens.
    name text NOT NULL CHECK (name ~ '^[a-z0-9-]{1,63}$'),
    role text NOT NULL CHECK (role = ANY (ARRAY['worker'::text, 'trino-coordinator'::text, 'trino-worker'::text, 'lake'::text, 'lake-replica'::text])),
    pool text CHECK (pool ~ '^[a-z0-9-]{1,63}$'),
    state text DEFAULT 'pending'::text NOT NULL CHECK (state = ANY (ARRAY['pending'::text, 'approved'::text, 'draining'::text, 'retired'::text])),
    version text,
    -- The certificate in use (lowercase hex) and when it ends.
    cert_serial text CHECK (cert_serial ~ '^[0-9a-f]{1,64}$'),
    cert_not_after timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    -- Who approved and who retired it, as the audit log names them.
    approved_at timestamp with time zone,
    approved_by uuid,
    retired_at timestamp with time zone,
    retired_by uuid
);

-- A name belongs to one machine that is not retired: a retired machine's
-- name can join again.
CREATE UNIQUE INDEX machines_live_name ON public.machines USING btree (tenant_id, name) WHERE (state <> 'retired'::text);

CREATE TABLE public.join_tokens (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL REFERENCES public.tenants(id) ON DELETE RESTRICT,
    -- SHA-256 of the token, hex; the token itself is never stored.
    token_hash text NOT NULL UNIQUE,
    role text NOT NULL CHECK (role = ANY (ARRAY['worker'::text, 'trino-coordinator'::text, 'trino-worker'::text, 'lake'::text, 'lake-replica'::text])),
    pool text CHECK (pool ~ '^[a-z0-9-]{1,63}$'),
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    machine_id uuid REFERENCES public.machines(id) ON DELETE RESTRICT
);

ALTER TABLE public.machines ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.machines FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.machines USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
ALTER TABLE public.join_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.join_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.join_tokens USING ((tenant_id = public.current_tenant())) WITH CHECK ((tenant_id = public.current_tenant()));
