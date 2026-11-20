-- Schedules and cuts (decision 0035). A schedule fires a dataset's cut (its
-- extraction, then its evaluation) or a job that needs no cut; a cut is one
-- per dataset and date, and re-running a date blocks or replaces it. Every
-- row is one tenant's.

CREATE TABLE quality.schedules (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    -- Five fields, read in `timezone` (quality::schedule).
    cron text NOT NULL,
    timezone text DEFAULT 'America/Bogota' NOT NULL,
    fires text NOT NULL CHECK (fires IN ('cut', 'job')),
    dataset text,
    rerun text DEFAULT 'block' NOT NULL CHECK (rerun IN ('block', 'replace')),
    -- A job without a cut: its kind and parameters.
    kind text CHECK (kind IN ('map', 'maintain')),
    parameters jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    next_fire_at timestamp with time zone,
    last_fired_at timestamp with time zone,
    -- What its last fire did: queued, blocked, replaced, skipped (too late).
    last_outcome text CHECK (last_outcome IN ('queued', 'blocked', 'replaced', 'skipped')),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, id),
    CHECK (
        (fires = 'cut' AND dataset IS NOT NULL AND kind IS NULL)
        OR (fires = 'job' AND kind IS NOT NULL)
    )
);

CREATE INDEX schedules_due ON quality.schedules (next_fire_at) WHERE enabled;

CREATE TABLE quality.cuts (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    dataset text NOT NULL,
    cut date NOT NULL,
    -- Who asked: a schedule, or a person (_Run now_).
    schedule_id uuid,
    requested_by uuid,
    state text DEFAULT 'extracting' NOT NULL
        CHECK (state IN ('extracting', 'evaluating', 'done', 'failed')),
    -- 1, and one more each time it is replaced.
    run integer DEFAULT 1 NOT NULL CHECK (run >= 1),
    extract_job uuid,
    evaluate_job uuid,
    bronze_snapshot text,
    -- The instant it was due, and when it fired: the run log's lateness.
    due_at timestamp with time zone NOT NULL,
    fired_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (tenant_id, dataset, cut),
    -- A schedule with cuts is disabled, not deleted: its cuts keep where
    -- they came from.
    FOREIGN KEY (tenant_id, schedule_id) REFERENCES quality.schedules (tenant_id, id)
);

ALTER TABLE quality.schedules ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.schedules FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.schedules
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());

ALTER TABLE quality.cuts ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.cuts FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.cuts
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());

-- A job without a cut may have no dataset (`maintain` of the whole
-- warehouse); the kinds that read one still need it. The previous version
-- always writes one.
ALTER TABLE quality.jobs ALTER COLUMN dataset DROP NOT NULL;
ALTER TABLE quality.jobs ADD CONSTRAINT jobs_dataset_needed
    CHECK (dataset IS NOT NULL OR kind = 'maintain');
