-- Each remediation queue's SLA in days (decision 0059): per dataset and
-- dimension (`grp` NULL, every group of it), and per group, which wins. A
-- queue with neither has 30 days, as the chart reads it. A finding is
-- overdue at a cut when its since plus its queue's SLA is before the cut;
-- the SLA is applied when reading, so changing it needs no evaluation.

CREATE TABLE quality.remediation_slas (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    dimension text NOT NULL,
    grp text,
    days integer NOT NULL CHECK (days > 0),
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    UNIQUE NULLS NOT DISTINCT (tenant_id, dataset, dimension, grp)
);

ALTER TABLE quality.remediation_slas ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.remediation_slas FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.remediation_slas
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
