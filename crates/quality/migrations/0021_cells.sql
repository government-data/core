-- Each SFC cell of a cut (decision 0058, 0031's `cells`): per field holding
-- the `sfc` role, the active records (`total`), those where it is right
-- (`ok`: filled, and no rule that reads it fails), whether it passes (at
-- least 90 %), and the right records it lacks to pass (`missing`) or can
-- lose and still pass (`margin`). Reported with the cut's results;
-- replacing the cut's summary replaces them. An older worker's report
-- leaves none.

CREATE TABLE quality.cells (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    cell text NOT NULL,
    ok bigint NOT NULL,
    total bigint NOT NULL,
    passes boolean NOT NULL,
    missing bigint NOT NULL,
    margin bigint NOT NULL,
    PRIMARY KEY (tenant_id, dataset, cut, cell),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.cells ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.cells FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.cells
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
