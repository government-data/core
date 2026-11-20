-- A rule's test (decisions 0019 point 10, 0043 point 3): what its job
-- found, kept a short time for whoever asked it, then purged. It holds a
-- read of records with their values: served only behind
-- quality:ViewPersonalData and masked otherwise, every read audited.
CREATE TABLE quality.rule_tests (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    job uuid NOT NULL,
    report jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, job),
    FOREIGN KEY (tenant_id, job) REFERENCES quality.jobs (tenant_id, id) ON DELETE CASCADE
);

ALTER TABLE quality.rule_tests ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.rule_tests FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.rule_tests
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
