-- Sources (decision 0036): a connection of a driver. What is not secret is
-- kept in the clear, to show and edit; the secret fields only sealed by the
-- application, as one JSON object, in two versions (current and previous,
-- decision 0030), so a password can be rotated without breaking a run.
-- Every row is one tenant's.

CREATE TABLE quality.sources (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    -- What a mapping's `from` and a Trino catalog call it.
    name text NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    driver text NOT NULL,
    fields jsonb DEFAULT '{}'::jsonb NOT NULL,
    secrets text NOT NULL,
    secrets_set_at timestamp with time zone DEFAULT now() NOT NULL,
    previous_secrets text,
    previous_set_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, name)
);

ALTER TABLE quality.sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.sources FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.sources
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
