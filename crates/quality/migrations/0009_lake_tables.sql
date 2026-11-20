-- The lake's tables (decision 0043, point 1): a `lake_tables` job lists
-- the tenant's warehouse, with no dataset, and the tenant keeps its last
-- listing. Each check is replaced in one statement, only widened: the
-- previous version writes no `lake_tables` job, and everything it writes
-- still holds.
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_kind_check,
    ADD CONSTRAINT jobs_kind_check
        CHECK (kind IN ('map', 'extract', 'evaluate', 'test_rule', 'maintain', 'tables',
                        'lake_tables'));
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_dataset_needed,
    ADD CONSTRAINT jobs_dataset_needed
        CHECK (dataset IS NOT NULL OR kind IN ('maintain', 'tables', 'lake_tables'));

-- Namespaces, tables and columns (name and type) only, from the tables'
-- metadata, never a row. One a tenant.
CREATE TABLE quality.lake_tables (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    job uuid NOT NULL,
    -- When the job that listed them was queued: a late report of an older
    -- job never replaces a newer listing.
    asked_at timestamp with time zone NOT NULL,
    listed_at timestamp with time zone DEFAULT now() NOT NULL,
    tables jsonb NOT NULL,
    truncated boolean DEFAULT false NOT NULL,
    PRIMARY KEY (tenant_id)
);

ALTER TABLE quality.lake_tables ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.lake_tables FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.lake_tables
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
