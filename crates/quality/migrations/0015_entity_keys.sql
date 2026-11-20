-- The tenant's entity key (decision 0057): the HMAC key that turns a
-- client's normalized document into the hash `entity_icd` keeps, so no
-- result holds a document. Sealed with the sources' key; generated once,
-- by the first evaluation that asks for it.

CREATE TABLE quality.entity_keys (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL PRIMARY KEY,
    sealed text NOT NULL,
    created_at timestamptz DEFAULT now() NOT NULL
);

ALTER TABLE quality.entity_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.entity_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.entity_keys
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
