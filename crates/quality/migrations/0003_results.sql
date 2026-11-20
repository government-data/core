-- The results a worker reports for a dataset's cut (decision 0031): only
-- aggregates, never a record's values. PostgreSQL keeps the recent cuts the
-- dashboard reads; the lake keeps the history. Reporting the same cut again
-- replaces it (jobs are idempotent).

CREATE TABLE quality.run_summary (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    job_id uuid NOT NULL,
    records bigint NOT NULL CHECK (records >= 0),
    duration_ms bigint NOT NULL CHECK (duration_ms >= 0),
    rules_commit text NOT NULL,
    reported_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, dataset, cut)
);

CREATE TABLE quality.rule_results (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    rule text NOT NULL,
    applies bigint NOT NULL CHECK (applies >= 0),
    failures bigint NOT NULL CHECK (failures BETWEEN 0 AND applies),
    PRIMARY KEY (tenant_id, dataset, cut, rule),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.run_summary ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.run_summary FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.run_summary
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());

ALTER TABLE quality.rule_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.rule_results FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.rule_results
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
