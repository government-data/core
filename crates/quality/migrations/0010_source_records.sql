-- A page of a source table's records (decision 0049, point 3): what its
-- `records` job read, kept a short time for whoever asked it, then purged.
-- It holds values: served only behind quality:ViewPersonalData and masked
-- otherwise, every read audited.
-- Each check is replaced in one statement, only widened: the previous
-- version writes no `records` job, and everything it writes still holds.
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_kind_check,
    ADD CONSTRAINT jobs_kind_check
        CHECK (kind IN ('map', 'extract', 'evaluate', 'test_rule', 'maintain', 'tables',
                        'lake_tables', 'records'));
ALTER TABLE quality.jobs
    DROP CONSTRAINT jobs_dataset_needed,
    ADD CONSTRAINT jobs_dataset_needed
        CHECK (dataset IS NOT NULL OR kind IN ('maintain', 'tables', 'lake_tables', 'records'));

CREATE TABLE quality.source_records (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    job uuid NOT NULL,
    page jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, job),
    FOREIGN KEY (tenant_id, job) REFERENCES quality.jobs (tenant_id, id) ON DELETE CASCADE
);

ALTER TABLE quality.source_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.source_records FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.source_records
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
