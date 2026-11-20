-- The run log's time per stage (decisions 0018, 0026 and 0031): how long
-- each stage of a cut's evaluation took (`load`, `rules`, `profile`, `icd`,
-- `recency`, `distributions`), in the order it ran (`position`). Reported
-- with the cut's results; replacing the cut's summary replaces them. An
-- older worker's report leaves none. Bytes, rows and spill per stage are
-- still to come.

CREATE TABLE quality.job_stats (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    stage text NOT NULL,
    position integer NOT NULL,
    ms bigint NOT NULL,
    PRIMARY KEY (tenant_id, dataset, cut, stage),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.job_stats ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.job_stats FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.job_stats
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
