-- A source's tables (decision 0042): a `tables` job lists them, with no
-- dataset, and the source keeps its last listing. Each check is replaced
-- in one statement, only widened: the previous version writes no `tables`
-- job, and everything it writes still holds.
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_kind_check,
    ADD CONSTRAINT jobs_kind_check
        CHECK (kind IN ('map', 'extract', 'evaluate', 'test_rule', 'maintain', 'tables'));
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_dataset_needed,
    ADD CONSTRAINT jobs_dataset_needed
        CHECK (dataset IS NOT NULL OR kind IN ('maintain', 'tables'));

-- Names and columns (name and type) only, never a row; the system's schemas are left
-- out by the job. A listing goes with its source.
CREATE TABLE quality.source_tables (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    source text NOT NULL,
    job uuid NOT NULL,
    -- When the job that listed them was queued: a late report of an older
    -- job never replaces a newer listing.
    asked_at timestamp with time zone NOT NULL,
    listed_at timestamp with time zone DEFAULT now() NOT NULL,
    tables jsonb NOT NULL,
    truncated boolean DEFAULT false NOT NULL,
    PRIMARY KEY (tenant_id, source),
    FOREIGN KEY (tenant_id, source) REFERENCES quality.sources (tenant_id, name) ON DELETE CASCADE
);

ALTER TABLE quality.source_tables ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.source_tables FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.source_tables
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
