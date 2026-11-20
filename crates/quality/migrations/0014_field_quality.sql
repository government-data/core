-- Each field's profile in a dataset's cut (decision 0031's `field_quality`):
-- empties, distinct values, and the shape its values most often take (each
-- digit a 9, each letter an A), never a value. Reported with the cut's
-- results; replacing the cut's summary replaces them.

CREATE TABLE quality.field_quality (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    field text NOT NULL,
    empty bigint NOT NULL CHECK (empty >= 0),
    distinct_values bigint NOT NULL CHECK (distinct_values >= 0),
    pattern text,
    pattern_count bigint NOT NULL CHECK (pattern_count >= 0),
    PRIMARY KEY (tenant_id, dataset, cut, field),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.field_quality ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.field_quality FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.field_quality
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
