-- Each group of a cut (decision 0031's `group_metrics`): per field holding
-- the `dimension` role and each of its values (`grp`, '' for the empty
-- ones), its records, those with a serious failure (blocking or major in a
-- critical or high field, until rules carry their cause) and their mean
-- ICD. Reported with the cut's results; replacing the cut's summary
-- replaces them. An older worker's report leaves none.

CREATE TABLE quality.group_metrics (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    dimension text NOT NULL,
    grp text NOT NULL,
    records bigint NOT NULL,
    serious bigint NOT NULL,
    mean_icd double precision,
    PRIMARY KEY (tenant_id, dataset, cut, dimension, grp),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.group_metrics ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.group_metrics FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.group_metrics
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
