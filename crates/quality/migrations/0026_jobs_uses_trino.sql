-- Trino's hold is per job, not per kind (decision 0066, point 9, the user's
-- ruling of 2026-11-11): each job carries whether it uses Trino, set when it
-- is queued from what the one who queues it knows (a source's driver, an
-- extraction's mapping when at hand) and corrected when its details are
-- asked, where the API knows the source it reads. NULL is not known.
-- When in doubt, it uses Trino: a job held too long is the possible
-- error, never Trino restarted under a job of its own.
ALTER TABLE quality.jobs ADD COLUMN uses_trino boolean;

-- Whether a job of `p_kind` marked `p_uses` counts as using Trino: held
-- while its tenant's hold is in force, and counted as Trino busy while it
-- runs. A kind that never reads through Trino never does; one that may
-- does unless it is known not to.
CREATE FUNCTION quality.on_trino(p_kind text, p_uses boolean) RETURNS boolean
    LANGUAGE sql IMMUTABLE AS $$
    SELECT quality.may_use_trino(p_kind) AND p_uses IS DISTINCT FROM false
$$;
GRANT EXECUTE ON FUNCTION quality.on_trino(text, boolean) TO qd_queue;

-- The queue's claim, as in 0025_trino_hold.sql (the machine's state, the
-- hold's lock taken shared), but holding only the jobs on Trino.
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
          AND NOT (quality.on_trino(j.kind, j.uses_trino) AND EXISTS (
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
