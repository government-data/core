-- The ICD of a cut (decision 0057): the IGCD and the entities per band in
-- its summary, each field's mean ICD in its profile, and each entity's
-- ICD, band and segment, the entity by the keyed hash of its normalized
-- key, never the key. Reported with the cut's results; replacing the
-- cut's summary replaces them. An older worker's report leaves them empty.

ALTER TABLE quality.run_summary ADD COLUMN igcd double precision;
ALTER TABLE quality.run_summary ADD COLUMN entities bigint;
ALTER TABLE quality.run_summary ADD COLUMN band_high bigint;
ALTER TABLE quality.run_summary ADD COLUMN band_medium bigint;
ALTER TABLE quality.run_summary ADD COLUMN band_low bigint;

ALTER TABLE quality.field_quality ADD COLUMN mean_icd double precision;

CREATE TABLE quality.entity_icd (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    entity text NOT NULL,
    icd double precision,
    band text CHECK (band IN ('high', 'medium', 'low')),
    segment text,
    PRIMARY KEY (tenant_id, dataset, cut, entity),
    FOREIGN KEY (tenant_id, dataset, cut)
        REFERENCES quality.run_summary (tenant_id, dataset, cut) ON DELETE CASCADE
);

ALTER TABLE quality.entity_icd ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.entity_icd FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.entity_icd
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
