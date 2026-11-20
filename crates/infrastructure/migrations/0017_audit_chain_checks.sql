-- Each verification of a tenant's audit chain (decision 0070, point 12):
-- when, how many rows and which sequence numbers were checked, the result,
-- and the first break by its number and kind, never a value of the row.
-- Written by `server audit verify` (and by the daily run, step 4); read by
-- the audit dashboard's panel (step 6). The anchors' kinds are step 4's.

CREATE TABLE public.audit_chain_checks (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL
        REFERENCES public.tenants(id) ON DELETE RESTRICT,
    checked_at timestamp with time zone NOT NULL,
    rows_checked bigint NOT NULL,
    first_seq bigint,
    last_seq bigint,
    result text NOT NULL CHECK (result IN ('ok', 'break')),
    break_seq bigint,
    break_kind text
        CHECK (break_kind IN ('gap', 'hash', 'anchor_mismatch', 'behind_anchor', 'head')),
    CHECK ((result = 'break') = (break_seq IS NOT NULL AND break_kind IS NOT NULL))
);

CREATE INDEX audit_chain_checks_tenant_idx
    ON public.audit_chain_checks (tenant_id, checked_at DESC);

ALTER TABLE public.audit_chain_checks ENABLE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.audit_chain_checks FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON public.audit_chain_checks
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
