-- Trino's coordinator takes a new certificate only by restarting, and only
-- when Trino is free (decision 0066, point 9). Before it restarts, its agent
-- asks the API, which holds the jobs that may use Trino (no queued one is
-- handed out from then on; one already running goes on), counts the ones
-- running and asks Trino itself; the hold ends when the agent says Trino
-- answers again, or by itself at its expiry, so a dead agent cannot hold
-- the jobs for ever. A row per tenant, as every table: the API writes one in
-- each tenant of its cell.

-- Whether a job of `p_kind` may read through Trino: those that read a
-- source (decision 0062: `tables`, `map`, `records` and `extract`). Whether
-- its source is on Trino is known only once its details are asked (an
-- extraction's source is its mapping's, in the catalog), so all of them are
-- held, those of a source on DuckDB too.
CREATE FUNCTION quality.may_use_trino(p_kind text) RETURNS boolean
    LANGUAGE sql IMMUTABLE AS $$
    SELECT p_kind IN ('tables', 'map', 'records', 'extract')
$$;

CREATE TABLE quality.trino_holds (
    tenant_id uuid DEFAULT public.current_tenant() NOT NULL PRIMARY KEY,
    -- The coordinator's machine that asked.
    machine text NOT NULL,
    held_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

ALTER TABLE quality.trino_holds ENABLE ROW LEVEL SECURITY;
ALTER TABLE quality.trino_holds FORCE ROW LEVEL SECURITY;
CREATE POLICY isolation ON quality.trino_holds
    USING (tenant_id = public.current_tenant())
    WITH CHECK (tenant_id = public.current_tenant());
-- The queue reads every tenant's hold, as it reads every tenant's jobs.
GRANT SELECT ON quality.trino_holds TO qd_queue;
CREATE POLICY queue ON quality.trino_holds FOR SELECT TO qd_queue USING (true);
GRANT EXECUTE ON FUNCTION quality.may_use_trino(text) TO qd_queue;

-- The queue's claim, as in 0024_queue_machines.sql, but a job that may use
-- Trino is not taken while its tenant's hold is in force. A claim takes the
-- hold's lock shared, and the API takes it alone to hold and count in one
-- transaction: a claim that started before the hold is in force has
-- committed (its job running, so counted) before the count is taken, and one
-- after it sees the hold. 6_600_009: 0066, point 9.
CREATE OR REPLACE FUNCTION quality.claim_job(p_worker text, p_kinds text[], p_lease interval, p_backoff interval)
    RETURNS TABLE (tenant_id uuid, id uuid, kind text, dataset text, parameters jsonb, engine text, attempts integer)
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM quality.queue_check(true);
    PERFORM pg_catalog.pg_advisory_xact_lock_shared(6600009);
    PERFORM quality.release_expired(p_backoff);
    RETURN QUERY
    WITH next AS (
        SELECT j.tenant_id, j.id FROM quality.jobs j
        WHERE j.state = 'queued' AND j.not_before <= now() AND j.kind = ANY (p_kinds)
          AND NOT (quality.may_use_trino(j.kind) AND EXISTS (
              SELECT 1 FROM quality.trino_holds h
               WHERE h.tenant_id = j.tenant_id AND h.expires_at > now()))
        ORDER BY j.priority DESC, j.created_at
        FOR UPDATE OF j SKIP LOCKED
        LIMIT 1
    )
    UPDATE quality.jobs j
    SET state = 'running', worker = p_worker, lease_until = now() + p_lease,
        attempts = j.attempts + 1, started_at = now(), error = NULL
    FROM next
    WHERE j.tenant_id = next.tenant_id AND j.id = next.id
    RETURNING j.tenant_id, j.id, j.kind, j.dataset, j.parameters, j.engine, j.attempts;
END
$$;
