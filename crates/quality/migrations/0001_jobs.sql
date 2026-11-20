-- The execution plane's queue (decisions 0018, 0023, 0027): a job is a row.
-- Workers take, renew and finish jobs; a job whose lease runs out goes back
-- to the queue, up to its attempts. Every row is one tenant's.
CREATE SCHEMA IF NOT EXISTS quality;

CREATE TABLE quality.jobs (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    kind text NOT NULL CHECK (kind IN ('map', 'extract', 'evaluate', 'test_rule', 'maintain')),
    dataset text NOT NULL,
    -- What the job needs that is not a secret: secrets travel per job
    -- through the API (decision 0023, point 2).
    parameters jsonb DEFAULT '{}'::jsonb NOT NULL,
    engine text DEFAULT 'duckdb' NOT NULL CHECK (engine IN ('duckdb', 'trino')),
    state text DEFAULT 'queued' NOT NULL CHECK (state IN ('queued', 'running', 'done', 'failed')),
    priority integer DEFAULT 0 NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    max_attempts integer DEFAULT 3 NOT NULL,
    -- Not taken before this time: the backoff between attempts.
    not_before timestamp with time zone DEFAULT now() NOT NULL,
    worker text,
    lease_until timestamp with time zone,
    error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    PRIMARY KEY (tenant_id, id)
);

CREATE INDEX jobs_ready ON quality.jobs (state, not_before, priority DESC);

ALTER TABLE quality.jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.jobs
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
