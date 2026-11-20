-- A cut's distributions (decision 0031's `distribution_checks`): per drift
-- variable its percentiles 0 to 100 (`dimension` and `grp` ''), which the
-- PSI against the previous cut reads; and per outlier amount, dimension
-- and group (`grp` '' for the empty one), its positive amounts and those
-- whose robust z is below -3.5 (`low`) or above 3.5 (`high`). Reported with
-- the cut's results; replacing the cut's summary replaces them.

CREATE TABLE quality.distribution_checks (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    variable text NOT NULL,
    dimension text NOT NULL,
    grp text NOT NULL,
    percentiles double precision[],
    records bigint,
    low bigint,
    high bigint,
    PRIMARY KEY (tenant_id, dataset, cut, variable, dimension, grp),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.distribution_checks ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.distribution_checks FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.distribution_checks
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
