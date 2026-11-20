-- Each remediation queue of a cut (decision 0059): per field holding the
-- `dimension` role and each of its values (`grp`, '' for the empty ones),
-- by the cut its findings opened at (`since`), those still open, those
-- resolved at this cut and the open ones' summed priority. Only counts:
-- the findings themselves are in the lake's gold layer. Reported with the
-- cut's results; replacing the cut's summary replaces them. An older
-- worker's report leaves none.

CREATE TABLE quality.queue_findings (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    dimension text NOT NULL,
    grp text NOT NULL,
    since date NOT NULL,
    open bigint NOT NULL CHECK (open >= 0),
    resolved bigint NOT NULL CHECK (resolved >= 0),
    priority double precision NOT NULL CHECK (priority >= 0),
    PRIMARY KEY (tenant_id, dataset, cut, dimension, grp, since),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE,
    CHECK (since <= cut)
);

ALTER TABLE quality.queue_findings ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.queue_findings FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.queue_findings
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
